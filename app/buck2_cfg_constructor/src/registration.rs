/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_core::cells::paths::CellRelativePath;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_CFG_CONSTRUCTOR_GLOBALS;
use buck2_interpreter_for_build::interpreter::build_context::BuildContext;
use buck2_interpreter_for_build::interpreter::build_context::PerFileTypeContext;
use buck2_interpreter_for_build::interpreter::package_file_extra::MAKE_CFG_CONSTRUCTOR;
use buck2_interpreter_for_build::interpreter::package_file_extra::PackageFileExtra;
use buck2_node::cfg_constructor::CfgConstructorImpl;
use buck2_node::metadata::key::MetadataKeyRef;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;
use starlark::values::starlark_value;

use crate::CfgConstructor;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum RegisterCfgConstructorError {
    #[error("`set_cfg_constructor()` can only be called from the root `PACKAGE` file of a cell")]
    NotCellRoot,
    #[error("`set_cfg_constructor()` can only be called at most once")]
    AlreadyRegistered,
}

/// Value stored in `PackageFileExtra.cfg_constructor` field.
/// Not a real Starlark value.
#[derive(
    Debug,
    derive_more::Display,
    Trace,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable,
    Freeze
)]
#[display("{:?}", self)]
struct StarlarkCfgConstructor<'v> {
    stage0: Value<'v>,
    stage1: Value<'v>,
    key: String,
    aliases: Option<Value<'v>>,
    extra_data: Option<Value<'v>>,
}

#[starlark_value(type = "StarlarkCfgConstructor", frozen_vtable, alloc = complex)]
impl<'v> StarlarkValue<'v> for StarlarkCfgConstructor<'v> {}

fn make_cfg_constructor(
    cfg_constructor: OwnedFrozen<Value<'static>>,
) -> buck2_error::Result<Arc<dyn CfgConstructorImpl>> {
    cfg_constructor.by_ref_with_reconstructor(|value, reconstructor| {
        let starlark = value.downcast_ref_err::<StarlarkCfgConstructor>()?;
        let field = |v| reconstructor.reconstruct::<Value<'static>>(v);
        let cfg_constructor: Arc<dyn CfgConstructorImpl> = Arc::new(CfgConstructor {
            cfg_constructor_pre_constraint_analysis: field(starlark.stage0),
            cfg_constructor_post_constraint_analysis: field(starlark.stage1),
            key: MetadataKeyRef::new(&starlark.key)?.to_owned(),
            aliases: starlark.aliases.map(field),
            extra_data: starlark.extra_data.map(field),
        });
        Ok(cfg_constructor)
    })
}

#[starlark_module]
pub(crate) fn register_set_cfg_constructor(globals: &mut GlobalsBuilder) {
    /// Register global cfg constructor.
    ///
    /// This function can only be called from the root `PACKAGE` file of a cell.
    /// When called from a non-root cell (e.g., an external cell), the call is
    /// silently ignored. This allows repositories to include `set_cfg_constructor`
    /// in their PACKAGE/BUCK_TREE files while still being consumable as external
    /// cells by other repositories.
    ///
    /// Parameters:
    ///   * `stage0`: The first cfg constructor that will be invoked before configuration rules are analyzed.
    ///   * `stage1`: The second cfg constructor that will be invoked after configuration rules are analyzed.
    ///   * `key`: The key for cfg modifiers on PACKAGE values and metadata.
    ///   * `aliases`: The aliases map to use for input modifiers.
    ///   * `extra_data`: Some extra data that may be used by `set_cfg_constructor` implementation that is
    ///     custom to our implementation and may not be used in other context like open-source.
    fn set_cfg_constructor<'v>(
        #[starlark(require=named)] stage0: Value<'v>,
        #[starlark(require=named)] stage1: Value<'v>,
        #[starlark(require=named)] key: &str,
        #[starlark(require = named, default = NoneOr::None)] aliases: NoneOr<Value<'v>>,
        #[starlark(require = named, default = NoneOr::None)] extra_data: NoneOr<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let build_context = BuildContext::from_context(eval)?;
        let ctx = match &build_context.additional {
            PerFileTypeContext::Package(ctx) => ctx,
            _ => {
                return Err(
                    buck2_error::Error::from(RegisterCfgConstructorError::NotCellRoot).into(),
                );
            }
        };

        // Check if this is being called from the root of the current cell
        let current_cell = ctx.path.dir().cell();
        let is_cell_root = ctx.path.dir().path() == CellRelativePath::empty();

        if !is_cell_root {
            // Not at cell root - this is an error
            return Err(
                buck2_error::Error::from(RegisterCfgConstructorError::NotCellRoot).into(),
            );
        }

        // Check if this cell is the root cell
        let is_root_cell = current_cell == build_context.cell_info.cell_resolver().root_cell();

        if !is_root_cell {
            // Called from an external cell's root PACKAGE - silently ignore.
            // This allows repos to have set_cfg_constructor in their PACKAGE/BUCK_TREE
            // while still being usable as external cells.
            return Ok(NoneType);
        }

        // This is the root cell's root PACKAGE - register the cfg constructor
        let package_file_extra: &PackageFileExtra = PackageFileExtra::get_or_init(eval)?;
        if package_file_extra.cfg_constructor.get().is_some() {
            return Err(
                buck2_error::Error::from(RegisterCfgConstructorError::AlreadyRegistered).into(),
            );
        }
        package_file_extra.cfg_constructor.get_or_init(|| {
            eval.heap()
                .alloc(StarlarkCfgConstructor {
                    stage0,
                    stage1,
                    key: key.to_owned(),
                    aliases: aliases.into_option(),
                    extra_data: extra_data.into_option(),
                })
                .to_value()
        });
        Ok(NoneType)
    }
}

pub(crate) fn init_registration() {
    MAKE_CFG_CONSTRUCTOR.init(make_cfg_constructor);
    REGISTER_BUCK2_CFG_CONSTRUCTOR_GLOBALS.init(register_set_cfg_constructor);
}
