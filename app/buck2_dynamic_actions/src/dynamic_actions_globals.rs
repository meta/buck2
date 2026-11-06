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
use std::iter;

use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_hash::BuckMutInsertionMap;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::typing::ParamIsRequired;
use starlark::typing::ParamSpec;
use starlark::typing::Ty;
use starlark::util::ArcStr;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark::values::typing::StarlarkCallableChecked;

use crate::attrs::DynamicAttrType;
use crate::attrs_starlark::StarlarkDynamicAttrType;
use crate::dynamic_actions::StarlarkDynamicActions;
use crate::dynamic_actions_callable::DynamicActionsCallable;
use crate::dynamic_actions_callable::DynamicActionsCallbackParam;
use crate::dynamic_actions_callable::DynamicActionsCallbackParamSpec;
use crate::dynamic_actions_callable::DynamicActionsCallbackReturnType;
use crate::dynamic_actions_callable::FrozenStarlarkDynamicActionsCallable;
use crate::dynamic_actions_callable::P_ACTIONS;

pub fn new_dynamic_actions_callable<'v>(
    r#impl: StarlarkCallableChecked<
        'v,
        DynamicActionsCallbackParamSpec,
        DynamicActionsCallbackReturnType,
    >,
    attrs: ValueOf<'v, DictType<String, &'v StarlarkDynamicAttrType<'v>>>,
    callback_param: &DynamicActionsCallbackParam,
) -> buck2_error::Result<DynamicActionsCallable<'v>> {
    if attrs.dict().get_str(callback_param.name.as_str()).is_some() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "Cannot define `actions` attribute"
        ));
    }
    let attrs: BuckMutInsertionMap<String, DynamicAttrType<'v>> = attrs
        .try_items()
        .map(|entry| {
            let (name, ty) = entry?;
            Ok::<_, starlark::Error>((name, ty.ty.clone()))
        })
        .collect::<starlark::Result<_>>()?;

    let attr_args = attrs
        .iter()
        .map(|(name, ty)| (name.as_str(), ty.impl_param_ty()))
        .collect::<Vec<_>>();

    r#impl
        .0
        .check_callable_with(
            [],
            iter::once((callback_param.name.as_str(), &*callback_param.ty))
                .chain(attr_args.iter().map(|(name, ty)| (*name, ty))),
            None,
            None,
            &DynamicActionsCallbackReturnType::starlark_type_repr(),
        )
        .buck_error_context("`impl` function must be callable with given params")?;

    let callable_ty = Ty::function(
        ParamSpec::new_named_only(attrs.iter().map(|(name, ty)| {
            (
                ArcStr::from(name.as_str()),
                ParamIsRequired::Yes,
                ty.callable_param_ty(),
            )
        }))
        .internal_error("Signature must be correct")?,
        StarlarkDynamicActions::starlark_type_repr(),
    );

    Ok(DynamicActionsCallable {
        self_ty: callable_ty,
        implementation: r#impl.to_unchecked(),
        name: OnceCell::new(),
        attrs,
    })
}

#[starlark_module]
#[starlark_types(
    StarlarkDynamicActions<'_> as DynamicActions,
    FrozenStarlarkDynamicActionsCallable<'_> as DynamicActionsCallable
)]
pub fn register_dynamic_actions(globals: &mut GlobalsBuilder) {
    /// Create new dynamic action callable. Returned object will be callable,
    /// and the result of calling it can be passed to `ctx.actions.dynamic_output_new`.
    ///
    /// Be aware that the context argument of the called impl function differs between
    /// [`dynamic_actions`](#dynamic_actions) where it is [`actions: AnalysisActions`](./AnalysisActions)
    /// and [`bxl.dynamic_actions`](../bxl/#dynamic_actions)
    /// where it is [`bxl_ctx: bxl.Context`](../bxl/Context).
    fn dynamic_actions<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallableChecked<
            'v,
            DynamicActionsCallbackParamSpec,
            DynamicActionsCallbackReturnType,
        >,
        #[starlark(require = named)] attrs: ValueOf<
            'v,
            DictType<String, &'v StarlarkDynamicAttrType<'v>>,
        >,
    ) -> starlark::Result<DynamicActionsCallable<'v>> {
        Ok(new_dynamic_actions_callable(r#impl, attrs, &P_ACTIONS)?)
    }
}
