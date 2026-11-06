/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;
use std::sync::OnceLock;

use buck2_hash::BuckFixedSortedMap;
use buck2_interpreter::types::provider::callable::ProviderCallableLike;
use dupe::Dupe;
use starlark::environment::GlobalsBuilder;
use starlark::typing::HasTyVTable;
use starlark::typing::Ty;
use starlark::typing::TyStarlarkValue;
use starlark::values::StarlarkValue;
use starlark::values::function::FUNCTION_TYPE;
use starlark::values::typing::TypeInstanceId;

use crate::interpreter::rule_defs::provider::ProviderLike;
use crate::interpreter::rule_defs::provider::ty::provider::ty_provider;
use crate::interpreter::rule_defs::provider::ty::provider_callable::ty_provider_callable;
use crate::interpreter::rule_defs::type_id_domain::Buck2TypeIdDomain;

/// Types associated with builtin providers.
pub struct BuiltinProviderTy<
    'v,
    P: StarlarkValue<'v> + HasTyVTable + ProviderLike<'v>,
    C: StarlarkValue<'v> + HasTyVTable + ProviderCallableLike,
> {
    callable: OnceLock<Ty>,
    instance: OnceLock<Ty>,
    /// The provider's attributes, which the typechecker needs since instances resolve
    /// attributes directly rather than through a methods table.
    fields: fn() -> BuckFixedSortedMap<String, Ty>,
    phantom: PhantomData<&'v (P, C)>,
}

unsafe impl<
    'v,
    P: StarlarkValue<'v> + HasTyVTable + ProviderLike<'v>,
    C: StarlarkValue<'v> + HasTyVTable + ProviderCallableLike,
> Sync for BuiltinProviderTy<'v, P, C>
{
}

impl<
    'v,
    P: StarlarkValue<'v> + HasTyVTable + ProviderLike<'v>,
    C: StarlarkValue<'v> + HasTyVTable + ProviderCallableLike,
> BuiltinProviderTy<'v, P, C>
{
    pub const fn new(
        fields: fn() -> BuckFixedSortedMap<String, Ty>,
    ) -> BuiltinProviderTy<'v, P, C> {
        BuiltinProviderTy {
            callable: OnceLock::new(),
            instance: OnceLock::new(),
            fields,
            phantom: PhantomData,
        }
    }

    pub fn callable(&self, creator_func: for<'a> fn(&'a mut GlobalsBuilder)) -> Ty {
        self.callable
            .get_or_init(|| builtin_provider_typechecker_ty::<C>(creator_func))
            .dupe()
    }

    pub fn instance(&self) -> Ty {
        self.instance
            .get_or_init(|| {
                ty_provider(
                    P::TYPE,
                    TypeInstanceId::from_identity(
                        Buck2TypeIdDomain::BuiltinProvider,
                        &(P::TYPE, "instance"),
                    ),
                    TyStarlarkValue::new::<P>(),
                    None,
                    (self.fields)(),
                )
                .unwrap()
            })
            .dupe()
    }
}

fn builtin_provider_typechecker_ty<
    'v,
    C: StarlarkValue<'v> + HasTyVTable + ProviderCallableLike,
>(
    creator_func: for<'a> fn(&'a mut GlobalsBuilder),
) -> Ty {
    let globals = GlobalsBuilder::new().with(creator_func).build();
    let mut iter = globals.iter();
    let Some(first) = iter.next() else {
        panic!("empty globals");
    };
    if iter.next().is_some() {
        panic!("more then one global in creator func globals");
    }
    if first.1.value().get_type() != FUNCTION_TYPE {
        panic!("creator func is not a function");
    }
    let ty = Ty::of_value(first.1.value());
    let ty_callable = ty
        .as_function()
        .expect("creator func is not a function")
        .callable()
        .dupe();
    ty_provider_callable::<C>(ty_callable).unwrap()
}

/// For providers whose attributes come from a custom methods table.
pub fn no_fields() -> BuckFixedSortedMap<String, Ty> {
    BuckFixedSortedMap::new()
}
