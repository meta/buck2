/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::provider::ty::abstract_provider::AbstractProvider;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutInsertionMap;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Arguments;
use starlark::eval::Evaluator;
use starlark::eval::ParametersSpec;
use starlark::eval::ParametersSpecParam;
use starlark::starlark_module;
use starlark::typing::ParamIsRequired;
use starlark::typing::ParamSpec;
use starlark::typing::Ty;
use starlark::util::ArcStr;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark::values::typing::StarlarkCallable;
use starlark::values::typing::StarlarkCallableParamSpec;

use crate::attrs::DynamicAttrType;
use crate::attrs::DynamicAttrValues;
use crate::dynamic_actions::StarlarkDynamicActions;
use crate::dynamic_actions::StarlarkDynamicActionsData;

pub struct DynamicActionsCallbackParamSpec;

pub struct DynamicActionsCallbackParam {
    pub name: pagable::StaticStr,
    pub ty: LazyLock<Ty>,
}

pagable::static_str!(P_ACTIONS_NAME = "actions");

pub static P_ACTIONS: DynamicActionsCallbackParam = DynamicActionsCallbackParam {
    name: P_ACTIONS_NAME,
    ty: LazyLock::new(AnalysisActions::starlark_type_repr),
};

impl StarlarkCallableParamSpec for DynamicActionsCallbackParamSpec {
    fn params() -> ParamSpec {
        ParamSpec::new_parts(
            [],
            [],
            None,
            [(
                ArcStr::new_static(P_ACTIONS.name),
                ParamIsRequired::Yes,
                P_ACTIONS.ty.dupe(),
            )],
            Some(Ty::any()),
        )
        .unwrap()
    }
}

pub type DynamicActionsCallbackReturnType = ListType<AbstractProvider>;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum DynamicActionCallableError {
    #[error("DynamicActionCallable can be called only if frozen")]
    NotFrozen,
    #[error("DynamicActionCallable must be exported (assigned to global variable)")]
    NotExported,
}

#[derive(
    Debug,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    derive_more::Display,
    Trace
)]
#[display(
    "DynamicActionCallable[{}]",
    self.name.get().map_or("(unbound)", |s| s.as_str())
)]
pub struct DynamicActionsCallable<'v> {
    pub self_ty: Ty,
    pub implementation:
        StarlarkCallable<'v, DynamicActionsCallbackParamSpec, DynamicActionsCallbackReturnType>,
    pub attrs: BuckMutInsertionMap<String, DynamicAttrType<'v>>,
    pub name: OnceCell<String>,
}

#[derive(
    Debug,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    derive_more::Display,
    StarlarkPagable
)]
#[display("DynamicActionsCallable[{}]", name)]
pub struct FrozenStarlarkDynamicActionsCallable<'v> {
    #[starlark_pagable(pagable)]
    pub self_ty: Ty,
    pub implementation:
        StarlarkCallable<'v, DynamicActionsCallbackParamSpec, DynamicActionsCallbackReturnType>,
    pub attrs: BuckMutInsertionMap<String, DynamicAttrType<'v>>,
    name: String,
    signature: ParametersSpec<Value<'v>>,
}

starlark::methods_static!(DYNAMIC_ACTION_CALLABLE_METHODS = dynamic_action_callable_methods);

/// A dynamic actions callable in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum DynamicActionsCallableView<'v> {
    Frozen(ValueTyped<'v, FrozenStarlarkDynamicActionsCallable<'v>>),
    Unfrozen(ValueTyped<'v, DynamicActionsCallable<'v>>),
}

#[starlark_value(type = "DynamicActionCallable")]
impl<'v> StarlarkType<'v> for DynamicActionsCallableView<'v> {
    fn export_as(
        &self,
        variable_name: &str,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<()> {
        if let DynamicActionsCallableView::Unfrozen(this) = self {
            // First wins.
            this.name.get_or_init(|| variable_name.to_owned());
        }
        Ok(())
    }

    fn invoke(
        &self,
        me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let this = match self {
            DynamicActionsCallableView::Frozen(this) => this,
            DynamicActionsCallableView::Unfrozen(_) => {
                return Err(buck2_error::Error::from(DynamicActionCallableError::NotFrozen).into());
            }
        };
        let me = FrozenValueTyped::new_err(me)?;
        let attr_values: DynamicAttrValues<'v> =
            this.signature.parser(args, eval, |parser, _eval| {
                let mut attr_values = Vec::with_capacity(this.attrs.len());
                for (name, attr_ty) in &this.attrs {
                    let value = attr_ty
                        .coerce(parser.next()?)
                        .with_buck_error_context(|| format!("Error coercing attribute `{name}`"))?;
                    attr_values.push(value);
                }
                Ok(DynamicAttrValues {
                    values: attr_values.into(),
                })
            })?;
        Ok(eval.heap().alloc(StarlarkDynamicActions {
            data: RefCell::new(Some(StarlarkDynamicActionsData {
                callable: me,
                attr_values,
            })),
        }))
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        match self {
            DynamicActionsCallableView::Frozen(this) => Some(this.self_ty.dupe()),
            DynamicActionsCallableView::Unfrozen(this) => Some(this.self_ty.dupe()),
        }
    }

    // used for docs of `DynamicActionCallable`
    fn get_methods() -> Option<&'static Methods> {
        Some(DYNAMIC_ACTION_CALLABLE_METHODS.methods())
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for DynamicActionsCallable<'v> {
    type Type = DynamicActionsCallableView<'v>;
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenStarlarkDynamicActionsCallable<'v> {
    type Type = DynamicActionsCallableView<'v>;
}

impl<'v> AllocValue<'v> for DynamicActionsCallable<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

impl<'v> Freeze<'v> for DynamicActionsCallable<'v> {
    type Frozen<'fv> = FrozenStarlarkDynamicActionsCallable<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let DynamicActionsCallable {
            self_ty,
            implementation,
            name,
            attrs,
        } = self;

        let name = name
            .into_inner()
            .ok_or(DynamicActionCallableError::NotExported)
            .map_err(|e| FreezeError::new(e.to_string()))?;

        let signature = ParametersSpec::new_named_only(
            &name,
            attrs
                .keys()
                .map(|s| (s.as_str(), ParametersSpecParam::Required)),
        );

        Ok(FrozenStarlarkDynamicActionsCallable {
            self_ty,
            implementation: implementation.freeze(freezer)?,
            name,
            // `Freeze::freeze` by name: the map's inherent `freeze` would be picked otherwise.
            attrs: Freeze::freeze(attrs, freezer)?,
            signature,
        })
    }
}

/// A factory function that creates `DynamicActions` instances.
///
/// `DynamicActionsCallable` is returned by calling `dynamic_actions()` with an implementation
/// function and attribute definitions. When invoked with concrete artifact values, it produces
/// a `DynamicActions` instance ready to be executed by `ctx.actions.dynamic_output_new()`.
///
/// This type must be assigned to a global variable before it can be called
///
/// # Usage
///
/// ```python
/// # Create a DynamicActionsCallable
/// _my_action = dynamic_actions(
///     impl = _my_impl,
///     attrs = {
///         "config": dynattrs.artifact_value(),
///         "out": dynattrs.output(),
///     },
/// )
///
/// # Later, call it with concrete values to create a DynamicActions
/// dynamic_action = _my_action(
///     config = config_file,
///     out = output.as_output(),
/// )
/// ```
///
/// See `dynamic_actions()` and `ctx.actions.dynamic_output_new()` for complete workflow.
#[starlark_module]
fn dynamic_action_callable_methods(builder: &mut MethodsBuilder) {}
