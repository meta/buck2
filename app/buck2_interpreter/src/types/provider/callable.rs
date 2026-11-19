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
use starlark::any::ProvidesStaticType;
use starlark::values::Value;

pub trait ProviderCallableLike {
    fn id(&self) -> buck2_error::Result<&Arc<ProviderId>>;
}

unsafe impl<'v> ProvidesStaticType<'v> for &'v dyn ProviderCallableLike {
    type StaticType = &'static dyn ProviderCallableLike;
}

pub trait ValueAsProviderCallableLike<'v> {
    fn as_provider_callable(&self) -> Option<&'v dyn ProviderCallableLike>;
}

impl<'v> ValueAsProviderCallableLike<'v> for Value<'v> {
    fn as_provider_callable(&self) -> Option<&'v dyn ProviderCallableLike> {
        self.request_value::<&dyn ProviderCallableLike>()
    }
}
