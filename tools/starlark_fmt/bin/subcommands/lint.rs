/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::path::PathBuf;

use lint_message::LintMessage;
use lint_message::LintSeverity;
use starlark_fmt_lib::Config;

use super::ProcessedFile;
use super::process_files;

fn lint_file(path: &Path, severity: LintSeverity, config: &Config) -> anyhow::Result<()> {
    let processed = ProcessedFile::new(path, config)?;

    if processed.has_changes() {
        let message = LintMessage::new(processed.path.display().to_string(), "format")
            .line(1)
            .char(1)
            .severity(severity)
            .original(processed.raw)
            .replacement(processed.formatted)
            .description(
                "File needs formatting. Run `arc lint` or `arc f` to resolve.\n\n\
                 See https://fburl.com/starlark-fmt for more info.",
            )
            .bypass_changed_line_filtering(true);

        println!("{}", serde_json::to_string(&message)?);
    }

    Ok(())
}

fn emit_lint_errors(errors: &[(PathBuf, anyhow::Error)]) {
    for (path, err) in errors {
        let message = LintMessage::new(path.display().to_string(), "starlark-fmt-error")
            .severity(LintSeverity::Error)
            .description(format!("{:#}", err))
            .bypass_changed_line_filtering(false);

        if let Ok(json) = serde_json::to_string(&message) {
            println!("{}", json);
        }
    }
}

pub fn lint_files(
    files: &[PathBuf],
    severity: LintSeverity,
    config: &Config,
) -> anyhow::Result<()> {
    let errors = process_files(files, |path| lint_file(path, severity, config));
    emit_lint_errors(&errors);
    Ok(())
}
