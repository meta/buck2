/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Providers are the data returned from a rule, and are the only way that information from this
//! rule is available to rules that depend on it. Every rule must return at least the `DefaultInfo`
//! provider, but most will also return either `RunInfo` (because they are executable) or some
//! custom provider (because they are incorporated into something that is ultimately executable).
//!
//! Internal providers (those defined and used by buck itself) can be defined easily using the
//! #[internal_provider(creator_func)] macro. This will generate all the code needed for that
//! provider to be used in starlark and to be treated as a provider in the various rust utilities
//! we have for providers.
//!
//! For an internal provider like:
//! ```skip
//! #[internal_provider(create_my_prov)]
//! #[derive(Clone, Debug, Trace, Freeze, ProvidesStaticType, Allocative, StarlarkPagable)]
//! #[repr(C)]
//! pub struct MyProvider<'v> {
//!    field1: StringValue<'v>,
//!    field2: ValueOf<'v, ListType<StringValue<'v>>>,
//! }
//!
//! #[starlark_module]
//! fn create_my_prov(globals: &mut GlobalsBuilder) {
//!    fn NameDoesntMatter<'v>(
//!        // It's not enforced that the args here match the fields, but it's generally the user expectation that they do.
//!        field1: StringValue<'v>,
//!        field2: ValueOf<'v, ListType<StringValue<'v>>>,
//!    ) -> starlark::Result<MyProvider<'v>> {
//!       // Can do some arg validation or computation here, just need to construct the provider.
//!       Ok(MyProvider {
//!            field1,
//!            field2
//!        })
//!    }
//! }
//! ```
//!
//! See the `internal_provider` attribute for the shape the struct must have.
//!
//! This will generate a "ProviderCallable" starlark type named (in starlark) `MyProvider` that acts like
//! the instance returned by a `provider()` call in starlark (so can be used to construct instances of the
//! provider or used in places like `attrs.dep(required_providers=[MyProvider]))`.
//!
//! For provider instances, in starlark all of their fields will be accessible by the field name.
//!
//! In rust, a StarlarkValue can be converted to the provider like normal with `MyProvider::from_value()`.
//! Often internally we'd have the analysis result (`ProviderCollection`) and want to get the
//! provider out of their so there's a convenience function for that: `MyProvider::from_providers(collect)`.

use std::convert::Infallible;
// TODO(cjhopman): That last one would be more discoverable if we moved it onto the
// `FrozenProviderCollectionValue` itself so you could do `collection.get::<MyProvider>()`.
use std::fmt::Debug;
use std::sync::Arc;

use buck2_core::provider::id::ProviderId;
use starlark::any::IsStaticType;
use starlark::any::ProvidesStaticType;
use starlark::typing::Ty;
use starlark::values::StarlarkValue;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::interpreter::rule_defs::provider::builtin::default_info::DefaultInfo;
use crate::interpreter::rule_defs::provider::builtin::default_info::DefaultInfoCallable;
use crate::interpreter::rule_defs::provider::collection::ProviderCollection;
use crate::interpreter::rule_defs::provider::ty::abstract_provider::AbstractProvider;

pub mod builtin;
pub mod callable;
pub mod collection;
pub mod dependency;
pub mod doc;
pub mod execution_platform;
pub mod registration;
pub mod test_provider;
pub mod ty;
pub(crate) mod user;

/// Implemented by providers (builtin or user defined).
pub trait ProviderLike<'v>: Debug {
    /// The ID. Guaranteed to be set on the `ProviderCallable` before constructing this object
    fn id(&self) -> &Arc<ProviderId>;
    /// Returns a list of all the keys and values.
    // TODO(cjhopman): I'd rather return an iterator. I couldn't get that to work, though.
    fn items(&self) -> Vec<(&str, Value<'v>)>;
}

/// Implemented by the frozen form of builtin providers.
///
/// `Self` is the brand-erased frozen type; `Self::Reinfect` recovers the branded form.
pub trait FrozenBuiltinProviderLike:
    ProviderLike<'static> + StarlarkValue<'static> + IsStaticType + Send + Sync
{
    fn builtin_provider_id() -> &'static Arc<ProviderId>;
}

unsafe impl<'v> ProvidesStaticType<'v> for &'v dyn ProviderLike<'v> {
    type StaticType = &'static dyn ProviderLike<'static>;
}

pub struct ValueAsProviderLike<'v>(pub(crate) &'v dyn ProviderLike<'v>);

impl<'v> StarlarkTypeRepr for ValueAsProviderLike<'v> {
    type Canonical = <AbstractProvider as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        AbstractProvider::starlark_type_repr()
    }
}

impl<'v> ValueAsProviderLike<'v> {
    pub(crate) fn unpack(value: Value<'v>) -> Option<Self> {
        Some(ValueAsProviderLike(value.request_value()?))
    }
}

impl<'v> UnpackValue<'v> for ValueAsProviderLike<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(ValueAsProviderLike::unpack(value))
    }
}
