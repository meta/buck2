/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use starlark_fmt_lib::Config;
use starlark_fmt_lib::FormattedSource;
use starlark_fmt_lib::format_source;
use tracing::info_span;

pub fn format_stdin(path: &Path, config: &Config) -> anyhow::Result<()> {
    let mut source = String::new();
    info_span!("read_stdin").in_scope(|| io::stdin().read_to_string(&mut source))?;

    let FormattedSource { formatted, .. } = format_source(&source, config, path)?;

    info_span!("write_stdout").in_scope(|| io::stdout().write_all(formatted.as_bytes()))?;

    Ok(())
}
