/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::fmt;
use std::fmt::Display;
use std::sync::Arc;

use allocative::Allocative;
use buck2_build_api::bxl::types::BxlFunctionLabel;
use buck2_core::bxl::BxlFilePath;
use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_interpreter::build_context::starlark_path_from_build_context;
use cli_args::CliArgs;
use derive_more::Display;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::starlark_value;
use starlark::values::typing::StarlarkCallable;

use crate::bxl::eval::CliResolutionCtx;
use crate::bxl::starlark_defs::cli_args;
use crate::bxl::starlark_defs::cli_args::ArgAccessor;
use crate::bxl::starlark_defs::cli_args::CliArgError;
use crate::bxl::starlark_defs::cli_args::CliArgValue;

#[starlark_module]
pub(crate) fn register_bxl_prefixed_main_function(builder: &mut GlobalsBuilder) {
    fn bxl_main<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallable<'v>,
        #[starlark(require = named)] cli_args: ValueOf<'v, DictType<&'v str, &'v CliArgs>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        bxl_impl(r#impl, cli_args, doc, eval)
    }
}

#[starlark_module]
pub(crate) fn register_bxl_main_function(builder: &mut GlobalsBuilder) {
    fn main<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallable<'v>,
        #[starlark(require = named)] cli_args: ValueOf<'v, DictType<&'v str, &'v CliArgs>>,
        #[starlark(require = named, default = "")] doc: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        bxl_impl(r#impl, cli_args, doc, eval)
    }
}

fn bxl_impl<'v>(
    r#impl: StarlarkCallable<'v>,
    cli_args: ValueOf<'v, DictType<&'v str, &'v CliArgs>>,
    doc: &str,
    eval: &mut Evaluator<'v, '_, '_>,
) -> starlark::Result<Value<'v>> {
    let implementation = r#impl.0;

    let bxl_path = starlark_path_from_build_context(eval)?
        .unpack_bxl_file()
        .ok_or_else(|| {
            buck2_error!(
                buck2_error::ErrorTag::Input,
                "`bxl` can only be declared in bxl files"
            )
        })?
        .clone();

    let mut unresolved_cli_args = BuckMutInsertionMap::new();
    let mut short_args = BuckMutInsertionSet::new();

    for entry in cli_args.try_items() {
        let (arg, def) = entry?;
        if let Some(short) = def.short {
            if short_args.contains(&short) {
                let buck2_error: buck2_error::Error =
                    CliArgError::DuplicateShort(short.to_owned()).into();
                return Err(buck2_error.into());
            } else {
                short_args.insert(short.to_owned());
            }
        }
        unresolved_cli_args.insert(arg.to_owned(), def.clone());
    }

    Ok(eval.heap().alloc(BxlFunction {
        bxl_path,
        id: RefCell::new(None),
        implementation,
        cli_args: unresolved_cli_args,
        docs: Some(doc.to_owned()),
    }))
}

/// Errors around rule declaration, instantiation, validation, etc
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum BxlError {
    #[error("Bxl defined in `{0}` must be assigned to a variable, e.g. `my_bxl = bxl_main(...)`")]
    BxlNotAssigned(String),
}

/// The callable created by `bxl()`
#[derive(Debug, Clone, ProvidesStaticType, Trace, NoSerialize, Allocative)]
pub(crate) struct BxlFunction<'v> {
    // The bxl path that contains the bxl() call; stored here so we can retrieve extra
    /// information during `export_as()`
    bxl_path: BxlFilePath,
    /// Once exported, the `import_path` and `name` of the callable. Used in DICE to retrieve bxl
    /// implementations
    id: RefCell<Option<BxlFunctionLabel>>,
    /// The implementation function for this bxl. Must be callable and take a ctx
    implementation: Value<'v>,
    /// the cli args to this bxl function
    cli_args: BuckMutInsertionMap<String, CliArgs>,
    docs: Option<String>,
}

impl<'v> Display for BxlFunction<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.id.borrow() {
            Some(id) => write!(f, "{}()", id.name),
            None => write!(f, "<unbound bxl>"),
        }
    }
}

impl<'v> AllocValue<'v> for BxlFunction<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

/// A bxl function in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum BxlFunctionView<'v> {
    Unfrozen(ValueTyped<'v, BxlFunction<'v>>),
    Frozen(ValueTyped<'v, FrozenBxlFunction<'v>>),
}

#[starlark_value(type = "bxl")]
impl<'v> StarlarkType<'v> for BxlFunctionView<'v> {
    fn export_as(
        &self,
        variable_name: &str,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<()> {
        if let BxlFunctionView::Unfrozen(this) = self {
            *this.id.borrow_mut() = Some(BxlFunctionLabel {
                bxl_path: this.bxl_path.clone(),
                name: variable_name.to_owned(),
            });
        }
        Ok(())
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for BxlFunction<'v> {
    type Type = BxlFunctionView<'v>;
}

impl<'v> Freeze<'v> for BxlFunction<'v> {
    type Frozen<'fv> = FrozenBxlFunction<'fv>;
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let frozen_impl = self.implementation.freeze(freezer)?;
        let docs = self.docs;
        let id = match self.id.into_inner() {
            Some(x) => x,
            None => {
                return Err(FreezeError::new(
                    BxlError::BxlNotAssigned(self.bxl_path.to_string()).to_string(),
                ));
            }
        };
        let bxl_id = Arc::new(id);

        Ok(FrozenBxlFunction {
            implementation: frozen_impl,
            cli_args: self.cli_args,
            bxl_id,
            docs,
        })
    }
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}()", bxl_id.name)]
pub(crate) struct FrozenBxlFunction<'v> {
    implementation: Value<'v>,
    cli_args: BuckMutInsertionMap<String, CliArgs>,
    #[starlark_pagable(pagable)]
    bxl_id: Arc<BxlFunctionLabel>,
    docs: Option<String>,
}

/// A bxl function kept alive by its owning frozen heap; usable across threads and awaits.
pub(crate) type OwnedBxlFunction = OwnedFrozen<ValueTyped<'static, FrozenBxlFunction<'static>>>;

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenBxlFunction<'v> {
    type Type = BxlFunctionView<'v>;
}

impl<'v> FrozenBxlFunction<'v> {
    pub(crate) fn implementation(&self) -> Value<'v> {
        self.implementation
    }

    pub(crate) fn cli_spec(&self) -> BxlCliSpec<'_> {
        BxlCliSpec {
            cli_args: &self.cli_args,
            docs: self.docs.as_deref(),
        }
    }
}

/// The command line interface declared by a bxl function: its `cli_args` and its docstring.
///
/// Split out from the function because resolving cli args is async, and this holds no
/// heap-branded values, so unlike a borrow of the function itself it can cross an await.
pub(crate) struct BxlCliSpec<'a> {
    cli_args: &'a BuckMutInsertionMap<String, CliArgs>,
    docs: Option<&'a str>,
}

impl<'a> BxlCliSpec<'a> {
    pub(crate) fn to_clap(&self, mut clap: clap::Command) -> clap::Command {
        if let Some(docs) = self.docs {
            clap = clap.about(docs.to_owned())
        }

        for (arg, def) in self.cli_args.iter() {
            clap = clap.arg(def.to_clap(clap::Arg::new(arg.clone()).long(arg.clone())))
        }

        clap
    }

    /// Parses the cli args as defined by this bxl function. Automatically changes the CLI args
    /// to snakecase when accessed from the bxl context.
    pub(crate) async fn parse_clap(
        &self,
        clap: clap::ArgMatches,
        ctx: &CliResolutionCtx<'_>,
    ) -> buck2_error::Result<BuckFixedInsertionMap<String, CliArgValue>> {
        let mut res = BuckMutInsertionMap::with_capacity(self.cli_args.len());

        for (arg, cli) in self.cli_args.iter() {
            let snake_case_args = arg.replace('-', "_");
            if res.contains_key(&snake_case_args) {
                return Err(CliArgError::DefinedBothKebabAndSnakeCase(arg.clone()).into());
            }
            res.insert(
                snake_case_args,
                cli.parse_clap(ArgAccessor::Clap { clap: &clap, arg }, ctx)
                    .await
                    .with_buck_error_context(|| {
                        format!("Error parsing cli flag `{arg}` for bxl function")
                    })?,
            );
        }

        Ok(res.freeze())
    }
}

starlark::__starlark_pagable_only! {
    #[cfg(test)]
    mod tests {
        use pagable::PagableDeserialize;
        use pagable::PagableSerialize;
        use starlark::values::FrozenHeapName;

        use super::*;

        #[test]
        fn frozen_bxl_function_round_trips() -> pagable::Result<()> {
            let expected_label = BxlFunctionLabel {
                bxl_path: BxlFilePath::testing_new("cell", "dir/test.bxl"),
                name: "main".to_owned(),
            };
            let owned: OwnedBxlFunction = OwnedFrozen::build(
                FrozenHeapName::user("frozen_bxl_function_round_trips"),
                |heap| {
                    heap.alloc_simple_typed(FrozenBxlFunction {
                        implementation: heap.alloc("implementation"),
                        cli_args: BuckMutInsertionMap::new(),
                        bxl_id: Arc::new(expected_label.clone()),
                        docs: Some("test docs".to_owned()),
                    })
                },
            );

            let mut serializer = pagable::testing::TestingSerializer::new();
            owned.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
            let restored = OwnedBxlFunction::pagable_deserialize(&mut deserializer)?;

            restored.by_ref(|f| {
                let f = f.as_ref();
                assert_eq!(f.implementation.unpack_str(), Some("implementation"));
                assert!(f.cli_args.is_empty());
                assert_eq!(f.bxl_id.as_ref(), &expected_label);
                assert_eq!(f.docs.as_deref(), Some("test docs"));
            });
            Ok(())
        }
    }
}
