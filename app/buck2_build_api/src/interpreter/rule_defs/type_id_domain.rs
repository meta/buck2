/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! buck2's [`TypeIdDomain`]s (providers, transitive sets, BXL target sets).
//! Kept in buck2 rather than starlark-rust, which is buck2-agnostic and only
//! knows its own `record`/`enum` domains.

use dupe::Dupe;
use starlark::values::typing::TypeIdDomain;

/// [`TypeIdDomain`]s for buck2-defined nominal types.
#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq)]
pub enum Buck2TypeIdDomain {
    /// A user-defined `provider(...)` type.
    UserProvider,
    /// A transitive set definition.
    TransitiveSet,
    /// A builtin (Rust-defined) provider type — both its instance and callable
    /// types; the role is disambiguated by the identity passed to `from_identity`.
    BuiltinProvider,
    /// The singleton `Provider` type that matches any provider instance.
    ProviderSingleton,
    /// A BXL `bxl.TargetSet[...]` type; the identity is the node kind.
    BxlTargetSet,
}

impl TypeIdDomain for Buck2TypeIdDomain {
    fn tag(&self) -> &'static str {
        match self {
            Buck2TypeIdDomain::UserProvider => "buck2.user_provider",
            Buck2TypeIdDomain::TransitiveSet => "buck2.transitive_set",
            Buck2TypeIdDomain::BuiltinProvider => "buck2.builtin_provider",
            Buck2TypeIdDomain::ProviderSingleton => "buck2.provider_singleton",
            Buck2TypeIdDomain::BxlTargetSet => "buck2.bxl_target_set",
        }
    }
}
