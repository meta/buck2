/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Read;

use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutMap;

/// One file of the viewer bundle.
pub(crate) struct Asset {
    pub(crate) mime: String,
    pub(crate) bytes: Vec<u8>,
}

/// Unpacks the bundle linked into the binary, keyed by path relative to the
/// bundle root (`index.html`, `assets/index-abc123.js`, ...).
pub(crate) fn load() -> buck2_error::Result<BuckMutMap<String, Asset>> {
    let mut assets = BuckMutMap::default();
    let mut archive = tar::Archive::new(trailcam_bundle::get());
    for entry in archive
        .entries()
        .buck_error_context("Reading the embedded trailcam bundle")?
    {
        let mut entry = entry.buck_error_context("Reading the embedded trailcam bundle")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .buck_error_context("Reading the embedded trailcam bundle")?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_owned();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .with_buck_error_context(|| format!("Reading `{path}` from the trailcam bundle"))?;
        let mime = mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string();
        assets.insert(path, Asset { mime, bytes });
    }
    Ok(assets)
}
