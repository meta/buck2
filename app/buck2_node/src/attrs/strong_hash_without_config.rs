/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hasher;

use buck2_core::provider::label::ConfiguredProvidersLabel;
use strong_hash::StrongHash;

/// Strongly hashes a configured value while omitting configurations embedded in labels.
pub(crate) trait StrongHashWithoutConfig {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H);
}

impl StrongHashWithoutConfig for ConfiguredProvidersLabel {
    fn strong_hash_without_config<H: Hasher>(&self, state: &mut H) {
        self.target().unconfigured().strong_hash(state);
        self.name().strong_hash(state);
    }
}
