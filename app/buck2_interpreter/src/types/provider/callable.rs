/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_core::provider::id::ProviderId;
use starlark::starlark_predicate_type;

pub trait ProviderCallableLike {
    fn id(&self) -> buck2_error::Result<&Arc<ProviderId>>;
}

// There is no global of this name, and so no `.bzl` file can write the type: an honest type for
// `FooInfo` would say which provider it makes, and what Starlark code gets to write for that
// is not decided. Signatures and error messages print the name all the same.
starlark_predicate_type! {
    /// A provider type: what makes providers of one kind, and what a provider is asked for by.
    ///
    /// `DefaultInfo` is one, and so is `FooInfo` for a `FooInfo = provider(..)` of a `.bzl`
    /// file. It is what `dep[FooInfo]` is indexed with and what
    /// `attrs.dep(providers = [FooInfo])` lists. What calling one gives, `FooInfo(..)`, is
    /// a `Provider`.
    pub StarlarkProviderCallable<'v>;
    name "ProviderCallable";
    get &'v dyn ProviderCallableLike;
}
