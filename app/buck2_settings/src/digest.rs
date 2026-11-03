/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_settings_derive::buck_settings_section;

/// How buck2 hashes file contents and action inputs.
#[buck_settings_section(name = "digest", version = 0, scope = daemon)]
pub struct Digest {
    /// The algorithms the daemon accepts, most preferred first, as a
    /// comma-separated list of `BLAKE3-KEYED`, `BLAKE3`, `SHA1` and `SHA256`.
    #[setting(
        buckconfig_fallback = "buck2.digest_algorithms",
        overridable_in(CommandLine, LocalSettings)
    )]
    algorithms: Option<String>,
    /// The algorithm source files are hashed with, one of `algorithms`; the
    /// first of them when unset.
    #[setting(
        buckconfig_fallback = "buck2.source_digest_algorithm",
        overridable_in(CommandLine, LocalSettings)
    )]
    source_algorithm: Option<String>,
}
