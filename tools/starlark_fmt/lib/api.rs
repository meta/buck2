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

use tracing::info_span;

use crate::autofixes;
use crate::autofixes::AutofixInput;
use crate::config::Config;
use crate::formatting;

/// Normalize line endings to Unix-style (`\n`).
///
/// This handles Windows files that use `\r\n` line endings by stripping all
/// carriage return characters. This is the standard approach for code
/// formatters since source files should use Unix line endings regardless of
/// platform.
fn normalize_line_endings(source: &str) -> String {
    if source.contains('\r') {
        source.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        source.to_owned()
    }
}

/// Result of applying the full formatting pipeline.
pub struct FormattedSource {
    /// The source after normalizing line endings.
    pub normalized: String,
    /// The final formatted output.
    pub formatted: String,
}

/// Apply the full formatting pipeline to source code: normalize line endings,
/// apply autofixes, and format.
///
/// `path` identifies the file being formatted; it is matched against config
/// overrides to decide which autofix passes to run.
pub fn format_source(
    source: &str,
    config: &Config,
    path: &Path,
) -> anyhow::Result<FormattedSource> {
    let normalized =
        info_span!("normalize_line_endings").in_scope(|| normalize_line_endings(source));

    let input = AutofixInput {
        source: &normalized,
        path,
    };
    let autofixed =
        info_span!("apply_autofixes").in_scope(|| autofixes::apply_autofixes(&input, config))?;
    let formatted = info_span!("format_file")
        .in_scope(|| formatting::format_file_with_path(autofixed, Some(path)))?;

    Ok(FormattedSource {
        normalized,
        formatted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_unix_line_endings_unchanged() {
        let input = "line1\nline2\nline3\n".to_owned();
        assert_eq!(normalize_line_endings(&input), input);
    }

    #[test]
    fn test_normalize_windows_line_endings() {
        let input = "line1\r\nline2\r\nline3\r\n".to_owned();
        assert_eq!(
            normalize_line_endings(&input),
            "line1\nline2\nline3\n".to_owned()
        );
    }

    #[test]
    fn test_normalize_mixed_line_endings() {
        let input = "line1\r\nline2\nline3\r\n".to_owned();
        assert_eq!(
            normalize_line_endings(&input),
            "line1\nline2\nline3\n".to_owned()
        );
    }

    #[test]
    fn test_normalize_bare_carriage_returns() {
        let input = "line1\rline2\rline3\r".to_owned();
        assert_eq!(
            normalize_line_endings(&input),
            "line1\nline2\nline3\n".to_owned()
        );
    }

    #[test]
    fn test_normalize_empty_string() {
        assert_eq!(normalize_line_endings(""), String::new());
    }
}
