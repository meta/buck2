/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;

use allocative::Allocative;
use buck2_interpreter::late_binding_ty::ProviderReprLate;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::type_matcher;
use starlark::typing::Ty;
use starlark::typing::TyStarlarkValue;
use starlark::typing::TyUser;
use starlark::typing::TyUserFields;
use starlark::typing::TyUserParams;
use starlark::values::StarlarkTypeExpr;
use starlark::values::StarlarkTypeExprStable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::typing::TypeInstanceId;
use starlark::values::typing::TypeMatcher;
use starlark::values::typing::TypeMatcherDyn;
use starlark::values::typing::TypeMatcherFactory;

use crate::interpreter::rule_defs::provider::ValueAsProviderLike;
use crate::interpreter::rule_defs::provider::user::UserProvider;
use crate::interpreter::rule_defs::type_id_domain::Buck2TypeIdDomain;

#[derive(Allocative, Clone, Debug, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
struct ProviderMatcher;

#[type_matcher]
impl TypeMatcher for ProviderMatcher {
    fn matches(&self, value: Value) -> bool {
        ValueAsProviderLike::unpack(value).is_some()
    }
}

fn mk_ty_provider() -> buck2_error::Result<Ty> {
    Ok(Ty::custom(TyUser::new(
        UserProvider::TYPE.to_owned(),
        // Builtin providers behave like `UserProvider`.
        TyStarlarkValue::new::<UserProvider>(),
        TypeInstanceId::from_identity(Buck2TypeIdDomain::ProviderSingleton, &UserProvider::TYPE),
        TyUserParams {
            matcher: Some(TypeMatcherFactory::new(ProviderMatcher)),
            fields: TyUserFields::unknown(),
            ..TyUserParams::default()
        },
    )?))
}

/// Type of any provider instance. In Starlark it is available as `Provider`.
pub struct AbstractProvider;

impl<'v> StarlarkTypeExpr<'v> for AbstractProvider {
    fn starlark_type_repr() -> Ty {
        static TY: OnceLock<Ty> = OnceLock::new();
        TY.get_or_init(|| mk_ty_provider().unwrap()).dupe()
    }

    type Get = ValueOf<'v, Self>;

    fn get(value: Value<'v>) -> Option<Self::Get> {
        ValueAsProviderLike::unpack(value)
            .is_some()
            .then(|| ValueOf::new_unchecked(value))
    }
}

impl<'v> StarlarkTypeExprStable<'v> for AbstractProvider {}

pub(crate) fn init_provider_ty() {
    ProviderReprLate::init(AbstractProvider::starlark_type_repr(), |value| {
        AbstractProvider::matches(value)
    })
}
