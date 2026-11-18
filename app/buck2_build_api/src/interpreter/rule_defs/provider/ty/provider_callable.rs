/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter::types::provider::callable::ProviderCallableLike;
use starlark::typing::HasTyVTable;
use starlark::typing::Ty;
use starlark::typing::TyCallable;
use starlark::typing::TyStarlarkValue;
use starlark::typing::TyUser;
use starlark::typing::TyUserParams;
use starlark::values::StarlarkValue;
use starlark::values::typing::TypeInstanceId;

use crate::interpreter::rule_defs::type_id_domain::Buck2TypeIdDomain;

pub(crate) fn ty_provider_callable<
    'v,
    C: StarlarkValue<'v> + HasTyVTable + ProviderCallableLike,
>(
    creator_func: TyCallable,
) -> buck2_error::Result<Ty> {
    Ok(Ty::custom(TyUser::new(
        C::TYPE.to_owned(),
        TyStarlarkValue::new::<C>(),
        TypeInstanceId::from_identity(Buck2TypeIdDomain::BuiltinProvider, &(C::TYPE, "callable")),
        TyUserParams {
            callable: Some(creator_func),
            ..TyUserParams::default()
        },
    )?))
}
