/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod diff;
mod fmt;
// The `lint` subcommand emits `lint_message::LintMessage` records for `arc lint`
// integration and is Meta-internal only; excluded from the open-source build.
#[cfg(fbcode_build)]
mod lint;
mod stdin;

use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub(crate) use diff::diff_file;
pub(crate) use fmt::format_files;
#[cfg(fbcode_build)]
pub(crate) use lint::lint_files;
use rayon::prelude::*;
use starlark_fmt_lib::Config;
use starlark_fmt_lib::FormattedSource;
use starlark_fmt_lib::format_source;
pub(crate) use stdin::format_stdin;
use tracing::info_span;

pub(crate) struct ProcessedFile<'a> {
    pub(crate) path: &'a Path,
    /// File contents as read from disk, with line endings preserved. Used as
    /// the `original` field in lint output so `arc lint --apply-patches` finds
    /// and replaces the exact byte range that exists on disk (files with
    /// `\r\n` line endings would otherwise leave a tail of unconsumed CRs).
    // Only read by the `lint` subcommand, which is `#[cfg(fbcode_build)]`, so this
    // field is unused in the open-source build.
    #[cfg_attr(not(fbcode_build), allow(dead_code))]
    pub(crate) raw: String,
    /// Source after normalizing line endings (`\r\n` → `\n`).
    pub(crate) source: String,
    pub(crate) formatted: String,
}

impl<'a> ProcessedFile<'a> {
    pub(crate) fn new(path: &'a Path, config: &Config) -> anyhow::Result<Self> {
        if path.is_dir() {
            return Err(anyhow::anyhow!(
                "{}: path is a directory, expected a file",
                path.display()
            ));
        }

        let raw = info_span!("read_file").in_scope(|| {
            fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))
        })?;
        let FormattedSource {
            normalized: source,
            formatted,
        } = format_source(&raw, config, path)?;

        Ok(Self {
            path,
            raw,
            source,
            formatted,
        })
    }

    pub(crate) fn has_changes(&self) -> bool {
        self.formatted != self.source
    }
}

pub(crate) fn process_files<F>(files: &[PathBuf], process_fn: F) -> Vec<(PathBuf, anyhow::Error)>
where
    F: Fn(&Path) -> anyhow::Result<()> + Sync,
{
    files
        .par_iter()
        .filter_map(|path| {
            info_span!("process_file").in_scope(|| {
                process_fn(path.as_path())
                    .err()
                    .map(|err| (path.clone(), err))
            })
        })
        .collect()
}
