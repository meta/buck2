/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::bzl::ImportPath;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::name::CellName;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_hash::IntentionallyStdHashSet;
use buck2_interpreter::file_type::StarlarkFileType;
use buck2_interpreter::import_paths::HasImportPaths;
use buck2_interpreter::load_module::INTERPRETER_CALCULATION_IMPL;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::prelude_path::PreludePath;
use dice::DiceComputations;
use dice::DiceTransaction;
use dupe::Dupe;
use starlark::environment::FrozenModule;
use starlark::environment::Globals;
use starlark::environment::GlobalsBuilder;
use starlark::typing::Ty;
use starlark::values::AllocFrozenValue;
use starlark::values::FrozenHeap;
use starlark::values::OwnedFrozenRef;
use starlark::values::Value;
use starlark::values::type_repr::StarlarkTypeRepr;

/// The environment in which a Starlark file is evaluated.
pub(crate) struct Environment {
    /// The globals that are driven from Rust.
    pub(crate) globals: Globals,
    /// The path to the prelude, if the prelude is loaded in this file.
    /// Note that in a BUCK file the `native` value is also exploded into the top-level.
    prelude: Option<PreludePath>,
    /// A path that is implicitly loaded as additional globals.
    preload: Option<ImportPath>,
}

impl Environment {
    pub(crate) async fn new(
        cell: CellName,
        path_type: StarlarkFileType,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<Environment> {
        // Find the information from the globals
        let globals = INTERPRETER_CALCULATION_IMPL.get()?.global_env(dice).await?;

        // Next grab the prelude, unless we are in the prelude cell and not a build file
        let prelude = match INTERPRETER_CALCULATION_IMPL
            .get()?
            .prelude_import(dice)
            .await?
        {
            Some(prelude)
                if path_type == StarlarkFileType::Buck || prelude.import_path().cell() != cell =>
            {
                Some(prelude)
            }
            _ => None,
        };

        // Now grab the pre-load things
        let preload = dice
            .import_paths_for_cell(BuildFileCell::new(cell))
            .await?
            .root_import()
            .cloned();

        Ok(Environment {
            globals,
            prelude,
            preload,
        })
    }

    /// The globals plus everything the interpreter injects into the module
    /// scope before evaluation: the prelude's public symbols, and for BUCK
    /// files the members of the prelude's `native` struct and the root
    /// implicit import. The typechecker derives types from the actual values,
    /// so rules get their attribute-based signatures.
    pub(crate) async fn globals_with_implicit_symbols(
        &self,
        path_type: StarlarkFileType,
        dice: &DiceTransaction,
    ) -> buck2_error::Result<Globals> {
        if self.prelude.is_none() && self.preload.is_none() {
            return Ok(self.globals.dupe());
        }
        let mut dice = dice.ctx();

        let mut builder = GlobalsBuilder::new();
        for (name, value) in self.globals.iter() {
            builder.set(name, Imported(value));
        }

        if let Some(prelude) = &self.prelude {
            let m = dice
                .get_loaded_module_from_import_path(prelude.import_path())
                .await?;
            add_module_symbols(&mut builder, m.env())?;
            if path_type == StarlarkFileType::Buck
                && let Some(native) = m.native_globals_for_buck_files()?
            {
                for (i, (name, _)) in native.value().iter().enumerate() {
                    // Re-derive the field from the owner to keep it tied to its heap.
                    let value = native
                        .try_map(|native| native.iter().nth(i).map(|(_, v)| v).ok_or(()))
                        .map_err(|()| internal_error!("struct field disappeared"))?;
                    builder.set(name.as_str(), Imported(value));
                }
            }
        }

        // The root implicit import is only injected into BUCK files.
        if path_type == StarlarkFileType::Buck
            && let Some(preload) = &self.preload
        {
            let m = dice.get_loaded_module_from_import_path(preload).await?;
            add_module_symbols(&mut builder, m.env())?;
        }

        Ok(builder.build())
    }

    pub(crate) async fn get_names(
        &self,
        path_type: StarlarkFileType,
        dice: &DiceTransaction,
    ) -> buck2_error::Result<IntentionallyStdHashSet<String>> {
        let mut dice = dice.ctx();
        let mut names = IntentionallyStdHashSet::new();

        for x in self.globals.names() {
            names.insert(x.to_owned());
        }

        if let Some(prelude) = &self.prelude {
            let m = dice
                .get_loaded_module_from_import_path(prelude.import_path())
                .await?;
            for x in m.env().names() {
                names.insert(x.to_owned());
            }
            if path_type == StarlarkFileType::Buck {
                if let Some(native) = m.native_globals_for_buck_files()? {
                    for (name, _value) in native.value().iter() {
                        names.insert(name.as_str().to_owned());
                    }
                }
            }
        }

        if let Some(preload) = &self.preload {
            let m = dice.get_loaded_module_from_import_path(preload).await?;
            for x in m.env().names() {
                names.insert(x.to_owned());
            }
        }

        Ok(names)
    }
}

/// Add a module's public symbols to the globals, the same way
/// `Module::import_public_symbols` injects them before evaluation.
fn add_module_symbols(builder: &mut GlobalsBuilder, env: &FrozenModule) -> buck2_error::Result<()> {
    for name in env.names() {
        let Some(value) = env
            .get_option_ref(name)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Interpreter))?
        else {
            continue;
        };
        builder.set(name, Imported(value));
    }
    Ok(())
}

/// A value from another frozen heap. Allocating it references that heap instead of copying.
struct Imported<'a>(OwnedFrozenRef<'a, Value<'static>>);

impl StarlarkTypeRepr for Imported<'_> {
    type Canonical = Self;

    fn starlark_type_repr() -> Ty {
        Ty::any()
    }
}

impl<'fv> AllocFrozenValue<'fv> for Imported<'_> {
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        self.0.add_to_frozen_heap(heap)
    }
}
