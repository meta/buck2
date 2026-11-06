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
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueOfUnchecked;
use starlark::values::ValueTyped;
use starlark::values::list::ListRef;
use starlark::values::list_or_tuple::UnpackListOrTuple;
use starlark::values::none::NoneOr;
use starlark::values::tuple::TupleRef;
use starlark::values::typing::StarlarkCallable;
use starlark::values::typing::StarlarkCallableChecked;
use starlark::values::typing::StarlarkCallableParamSpec;

use crate::transition::starlark::IMPL_ATTRS_PARAM;
use crate::transition::starlark::IMPL_PLATFORM_PARAM;
use crate::transition::starlark::ImplSingleReturnTy;

struct TransitionImplParams;

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
    pub(crate) r#impl: ValueOfUnchecked<'v, StarlarkCallable<'static>>,
    pub(crate) attrs: ValueOfUnchecked<'v, NoneOr<UnpackListOrTuple<String>>>,
}

/// A `TransitionInfo` kept alive by its owning frozen heap; usable across threads and awaits.
pub(crate) type OwnedTransitionInfo = OwnedFrozen<ValueTyped<'static, TransitionInfo<'static>>>;

#[starlark_module]
fn transition_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = TransitionInfo<'static>)]
    fn TransitionInfo<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallableChecked<
            'v,
            TransitionImplParams,
            ImplSingleReturnTy,
        >,
        #[starlark(require = named)] attrs: Option<ValueOf<'v, UnpackListOrTuple<String>>>,
    ) -> starlark::Result<TransitionInfo<'v>> {
        let attrs = attrs.map_or(Value::new_none(), |v| v.to_value());
        Ok(TransitionInfo {
            r#impl: ValueOfUnchecked::new(r#impl.0),
            attrs: ValueOfUnchecked::new(attrs),
        })
    }
}

impl<'v> TransitionInfo<'v> {
    pub(crate) fn get_attrs_names(&self) -> Option<impl IntoIterator<Item = &'v str> + use<'v>> {
        let v = self.attrs.get();
        let slice: &[_] = if v.is_none() {
            return None;
        } else if let Some(v) = ListRef::from_value(v) {
            v
        } else if let Some(v) = TupleRef::from_value(v) {
            v.content()
        } else {
            unreachable!("Checked at construction");
        };
        Some(slice.iter().map(|v| {
            v.unpack_starlark_str()
                .expect("checked at construction")
                .as_str()
        }))
    }
}
