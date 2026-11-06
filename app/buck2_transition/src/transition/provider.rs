/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::typing::ParamIsRequired;
use starlark::typing::ParamSpec;
use starlark::util::ArcStr;
use starlark::values::Freeze;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::typing::StarlarkCallableChecked;
use starlark::values::typing::StarlarkCallableParamSpec;

use crate::transition::starlark::IMPL_ATTRS_PARAM;
use crate::transition::starlark::IMPL_PLATFORM_PARAM;
use crate::transition::starlark::ImplSingleReturnTy;

pub(crate) struct TransitionImplParams;

impl StarlarkCallableParamSpec for TransitionImplParams {
    fn params() -> ParamSpec {
        ParamSpec::new_named_only([
            (
                ArcStr::new_static(IMPL_PLATFORM_PARAM.name),
                ParamIsRequired::Yes,
                IMPL_PLATFORM_PARAM.ty.dupe(),
            ),
            (
                ArcStr::new_static(IMPL_ATTRS_PARAM.name),
                ParamIsRequired::No,
                IMPL_ATTRS_PARAM.ty.dupe(),
            ),
        ])
        .unwrap()
    }
}

#[internal_provider(transition_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub(crate) struct TransitionInfo<'v> {
    pub(crate) r#impl:
        ValueOf<'v, StarlarkCallableChecked<'v, TransitionImplParams, ImplSingleReturnTy<'static>>>,
    pub(crate) attrs: Option<ValueOf<'v, ListOrTupleType<StringValue<'v>>>>,
}

/// A `TransitionInfo` kept alive by its owning frozen heap; usable across threads and awaits.
pub(crate) type OwnedTransitionInfo = OwnedFrozen<ValueTyped<'static, TransitionInfo<'static>>>;

#[starlark_module]
fn transition_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = TransitionInfo<'static>)]
    fn TransitionInfo<'v>(
        #[starlark(require = named)] r#impl: ValueOf<
            'v,
            StarlarkCallableChecked<'v, TransitionImplParams, ImplSingleReturnTy<'static>>,
        >,
        #[starlark(require = named)] attrs: Option<ValueOf<'v, ListOrTupleType<StringValue<'v>>>>,
    ) -> starlark::Result<TransitionInfo<'v>> {
        Ok(TransitionInfo { r#impl, attrs })
    }
}

impl<'v> TransitionInfo<'v> {
    pub(crate) fn get_attrs_names(
        &self,
    ) -> Option<impl Iterator<Item = buck2_error::Result<&'v str>> + use<'v>> {
        Some(self.attrs?.iter().map(|s| Ok(s?.unpack().as_str())))
    }
}
