/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_execute::digest_config::DigestConfig;
use derive_more::Display;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::starlark_value;

/// Functions to access the daemon's digest config. This is not normally all that useful, but it
/// allows instrospection in a few useful cases (such as knowing what hashes the daemon will be
/// able to defer).
#[derive(
    Debug,
    Clone,
    Dupe,
    Freeze,
    Display,
    Trace,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}", self.digest_config)]
pub struct StarlarkDigestConfig {
    #[freeze(identity)]
    #[starlark_pagable(pagable)]
    pub digest_config: DigestConfig,
}

starlark::methods_static!(DIGEST_CONFIG_METHODS = digest_config_methods);

#[starlark_value(type = "DigestConfig")]
impl<'v> StarlarkValue<'v> for StarlarkDigestConfig {
    fn get_methods() -> Option<&'static Methods> {
        Some(DIGEST_CONFIG_METHODS.methods())
    }
}

#[starlark_module]
fn digest_config_methods(builder: &mut MethodsBuilder) {
    fn allows_sha1(this: &StarlarkDigestConfig) -> starlark::Result<bool> {
        Ok(this.digest_config.cas_digest_config().allows_sha1())
    }

    fn allows_sha256(this: &StarlarkDigestConfig) -> starlark::Result<bool> {
        Ok(this.digest_config.cas_digest_config().allows_sha256())
    }

    fn allows_blake3(this: &StarlarkDigestConfig) -> starlark::Result<bool> {
        Ok(this.digest_config.cas_digest_config().allows_blake3())
    }

    fn allows_blake3_keyed(this: &StarlarkDigestConfig) -> starlark::Result<bool> {
        Ok(this.digest_config.cas_digest_config().allows_blake3_keyed())
    }
}

#[starlark_module]
#[starlark_types(StarlarkDigestConfig as DigestConfig)]
pub(crate) fn register_digest_config_type(globals: &mut GlobalsBuilder) {}
