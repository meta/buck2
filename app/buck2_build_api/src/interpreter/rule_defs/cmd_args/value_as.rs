/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;

use buck2_interpreter::types::cell_path::StarlarkCellPath;
use buck2_interpreter::types::cell_root::CellRoot;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_interpreter::types::project_root::StarlarkProjectRoot;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use dupe::Dupe;
use starlark::any::IsStaticType;
use starlark::any::ProvidesStaticType;
use starlark::typing::Ty;
use starlark::values::UnpackValue;
use starlark::values::UnpackValueStable;
use starlark::values::Value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_ty;
use crate::interpreter::rule_defs::provider::builtin::run_info::RunInfo;

pub struct ValueAsCommandLineLike<'v>(pub &'v dyn CommandLineArgLike<'v>);

// SAFETY: the static type is the same type with the heap lifetime erased.
unsafe impl<'v> ProvidesStaticType<'v> for ValueAsCommandLineLike<'v> {
    type StaticType = ValueAsCommandLineLike<'static>;
}

impl IsStaticType for ValueAsCommandLineLike<'static> {
    type Reinfect<'lt> = ValueAsCommandLineLike<'lt>;
}

impl<'v> StarlarkTypeRepr for ValueAsCommandLineLike<'v> {
    type Canonical = ValueAsCommandLineLike<'static>;

    fn starlark_type_repr() -> Ty {
        command_line_arg_like_ty().dupe()
    }
}

impl<'v> ValueAsCommandLineLike<'v> {
    pub(crate) fn unpack(value: Value<'v>) -> Option<Self> {
        if let Some(x) = value.unpack_starlark_str() {
            return Some(ValueAsCommandLineLike(x as &dyn CommandLineArgLike));
        }

        macro_rules! check {
            ($t:ty) => {
                if let Some(v) = value.downcast_ref::<$t>() {
                    return Some(ValueAsCommandLineLike(v as &dyn CommandLineArgLike));
                }
            };
        }

        // Typically downcasting is provided by implementing `StarlarkValue::provide`.
        // These are exceptions:
        // * either providers, where `StarlarkValue` is generated,
        //   and plugging in `provide` is tricky
        // * or live outside of `buck2_build_api` crate,
        //   so `impl StarlarkValue` cannot provide `CommandLineArgLike`
        check!(RunInfo);
        check!(StarlarkCellPath);
        check!(StarlarkTargetLabel);
        check!(StarlarkConfiguredProvidersLabel);
        check!(CellRoot);
        check!(StarlarkProjectRoot);

        Some(ValueAsCommandLineLike(value.request_value()?))
    }
}

/// Whether a value provides `CommandLineArgLike` is a property of its type.
impl<'v> UnpackValueStable<'v> for ValueAsCommandLineLike<'v> {}

impl<'v> UnpackValue<'v> for ValueAsCommandLineLike<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(Self::unpack(value))
    }
}
