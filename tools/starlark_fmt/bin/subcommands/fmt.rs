/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use starlark_fmt_lib::Config;
use tracing::error;
use tracing::info;

use super::ProcessedFile;
use super::process_files;

fn format_file_in_place(path: &Path, config: &Config) -> anyhow::Result<()> {
    let processed = ProcessedFile::new(path, config)?;

    if processed.has_changes() {
        fs::write(processed.path, &processed.formatted)?;
        info!("{}: formatted", processed.path.display());
    }

    Ok(())
}

pub fn format_files(files: &[PathBuf], config: &Config) -> anyhow::Result<()> {
    let errors = process_files(files, |path| format_file_in_place(path, config));

    if errors.is_empty() {
        return Ok(());
    }

    for (_, err) in &errors {
        error!("{:#}", err);
    }

    Err(anyhow::anyhow!(
        "{} file{} failed to format",
        errors.len(),
        if errors.len() == 1 { "" } else { "s" }
    ))
}
