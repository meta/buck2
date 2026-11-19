/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

macro_rules! late_binding_ty {
    ($name:ident, $late:ident) => {
        static $late: buck2_util::late_binding::LateBinding<LateTy> =
            buck2_util::late_binding::LateBinding::new(stringify!($name));

        #[allow(clippy::empty_enums)]
        pub enum $name {}

        impl $name {
            /// Binds the type: its `Ty`, and the check of a value against it, which is
            /// [`StarlarkTypeExpr::matches`](starlark::values::StarlarkTypeExpr::matches) of the
            /// type that `$name` stands for.
            pub fn init(
                ty: starlark::typing::Ty,
                matches: fn(starlark::values::Value<'_>) -> bool,
            ) {
                $late.init(LateTy { ty, matches });
            }

            pub fn ty() -> starlark::typing::Ty {
                dupe::Dupe::dupe(&$late.get().unwrap().ty)
            }
        }

        impl<'v> starlark::values::StarlarkTypeExpr<'v> for $name {
            fn starlark_type_repr() -> starlark::typing::Ty {
                Self::ty()
            }

            type Get = starlark::values::ValueOf<'v, Self>;

            fn get(value: starlark::values::Value<'v>) -> Option<Self::Get> {
                ($late.get().unwrap().matches)(value)
                    .then(|| starlark::values::ValueOf::new_unchecked(value))
            }
        }

        impl<'v> starlark::values::StarlarkTypeExprStable<'v> for $name {}
    };
}

/// A type bound at startup by the crate that defines it.
pub struct LateTy {
    ty: starlark::typing::Ty,
    matches: fn(starlark::values::Value<'_>) -> bool,
}

late_binding_ty!(AnalysisContextReprLate, ANALYSIS_CONTEXT_REPR_LATE);
late_binding_ty!(ProviderReprLate, PROVIDER_REPR_LATE);
late_binding_ty!(TransitionReprLate, TRANSITION_REPR_LATE);
