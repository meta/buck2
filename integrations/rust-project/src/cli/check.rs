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
use std::str::FromStr;

use anyhow::Context as _;
use rustc_hash::FxHashSet;

use crate::buck;
use crate::buck::Buck;
use crate::diagnostics;
use crate::path::safe_canonicalize;

pub(crate) struct Check {
    pub(crate) buck: buck::Buck,
    pub(crate) use_clippy: bool,
    pub(crate) saved_file: PathBuf,
}

impl Check {
    pub(crate) fn new(buck: Buck, use_clippy: bool, saved_file: PathBuf) -> Self {
        let saved_file = safe_canonicalize(&saved_file);

        Self {
            buck,
            use_clippy,
            saved_file,
        }
    }

    pub(crate) fn run(&self) -> Result<(), anyhow::Error> {
        let start = std::time::Instant::now();
        let buck = &self.buck;

        let check_output = buck.check_saved_file(self.use_clippy, &self.saved_file)?;

        let contents: Vec<String> = check_output
            .diagnostic_paths
            .iter()
            .map(|path| {
                std::fs::read_to_string(path).context(format!(
                    "Trying to read JSON file of diagnostics: {}",
                    path.display(),
                ))
            })
            .collect::<Result<_, _>>()?;
        let diagnostics = parse_diagnostics(&contents, &check_output.project_root)?;

        for diagnostic in diagnostics {
            let out = serde_json::to_string(&diagnostic)?;
            println!("{out}");
        }

        crate::scuba::log_check(start.elapsed(), &self.saved_file, self.use_clippy);

        Ok(())
    }
}

/// Given `contents` of newline-delimited JSON of rustc diagnostics, parse the
/// JSON and fix up diagnostics for rust-analyzer consumption.
fn parse_diagnostics(
    contents: &[String],
    project_root: &Path,
) -> Result<Vec<serde_json::Value>, anyhow::Error> {
    let mut seen = FxHashSet::default();
    let mut diagnostics = vec![];
    for content in contents {
        for l in content.lines() {
            if let Ok(mut message) = serde_json::from_str::<diagnostics::Message>(l) {
                make_message_absolute(&mut message, project_root);

                // We may have diagnostics from both foo and foo-unittest
                // targets. They are identical other than the target names
                // and build directories.
                //
                // To deduplicate, we assume that two diagnostics with
                // exactly the same spans and message are identical, and
                // don't consider other fields.
                if seen.insert(drop_minor_details(&message)) {
                    diagnostics.push(serde_json::to_value(message)?);
                }
            } else {
                let value = serde_json::Value::from_str(l)?;
                diagnostics.push(value)
            }
        }
    }
    Ok(diagnostics)
}

fn drop_minor_details(message: &diagnostics::Message) -> diagnostics::Message {
    diagnostics::Message {
        children: vec![],
        rendered: Some("".to_owned()),
        ..message.clone()
    }
}

/// rustc returns diagnostics with relative paths. For example, the path for
/// `lib.rs` in `fbcode//common/rust/tracing-scuba:tracing-scuba` will be shown
/// as `fbcode/common/rust/tracing-scuba/src/lib.rs`.
///
/// Unfortunately, the user's working directory may not be the project
/// root. They might open their editor at the cell root (e.g. fbsource/fbcode/)
/// or a more specific subdirectory. As a result, rust-analyzer won't be able to
/// find the file referenced in the diagnostic.
///
/// By converting relative paths to absolute paths, rust-analyzer will always be
/// able to process the diagnostics.
fn make_message_absolute(message: &mut diagnostics::Message, base_dir: &Path) {
    for span in message.spans.iter_mut() {
        make_span_absolute(span, base_dir);
    }

    for message in message.children.iter_mut() {
        make_message_absolute(message, base_dir);
    }
}

fn make_span_absolute(span: &mut diagnostics::Span, base_dir: &Path) {
    span.file_name = base_dir.join(&span.file_name);

    if let Some(expansion) = &mut span.expansion {
        if let Some(def_site_span) = &mut expansion.def_site_span {
            make_span_absolute(def_site_span, base_dir);
        }

        make_span_absolute(&mut expansion.span, base_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message(file_name: &str, msg: &str) -> String {
        serde_json::json!({
            "message": msg,
            "code": null,
            "level": "error",
            "spans": [{
                "file_name": file_name,
                "byte_start": 0,
                "byte_end": 1,
                "line_start": 1,
                "line_end": 1,
                "column_start": 1,
                "column_end": 2,
                "is_primary": true,
                "text": [],
                "label": null,
                "suggested_replacement": null,
                "suggestion_applicability": null,
                "expansion": null,
            }],
            "children": [],
            "rendered": null,
        })
        .to_string()
    }

    #[cfg(unix)] // TODO: switch to unix_path crate so this works on windows too
    #[test]
    fn test_parse_single_diagnostic() {
        let contents = vec![sample_message("src/lib.rs", "unused variable")];
        let root = Path::new("/project");

        let result = parse_diagnostics(&contents, root).unwrap();
        assert_eq!(result.len(), 1);

        let span = &result[0]["spans"][0];
        assert_eq!(span["file_name"], "/project/src/lib.rs");
    }

    #[cfg(unix)] // TODO: switch to unix_path crate so this works on windows too
    #[test]
    fn test_parse_multiple_diagnostics() {
        let line1 = sample_message("src/a.rs", "error one");
        let line2 = sample_message("src/b.rs", "error two");
        let contents = vec![format!("{line1}\n{line2}")];
        let root = Path::new("/project");

        let result = parse_diagnostics(&contents, root).unwrap();
        assert_eq!(result.len(), 2);

        assert_eq!(result[0]["spans"][0]["file_name"], "/project/src/a.rs");
        assert_eq!(result[1]["spans"][0]["file_name"], "/project/src/b.rs");
    }

    #[test]
    fn test_parse_deduplicates_identical_diagnostics() {
        let line = sample_message("src/lib.rs", "duplicate error");
        let contents = vec![format!("{line}\n{line}")];
        let root = Path::new("/project");

        let result = parse_diagnostics(&contents, root).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_deduplicates_diagnostics_with_different_children() {
        let msg1 = serde_json::json!({
            "message": "unused variable",
            "code": null,
            "level": "error",
            "spans": [{
                "file_name": "src/lib.rs",
                "byte_start": 0,
                "byte_end": 1,
                "line_start": 1,
                "line_end": 1,
                "column_start": 1,
                "column_end": 2,
                "is_primary": true,
                "text": [],
                "label": null,
                "suggested_replacement": null,
                "suggestion_applicability": null,
                "expansion": null,
            }],
            "children": [{
                "message": "see foo for more details",
                "code": null,
                "level": "help",
                "spans": [],
                "children": [],
                "rendered": null,
            }],
            "rendered": "unused variable in foo",
        })
        .to_string();

        let msg2 = serde_json::json!({
            "message": "unused variable",
            "code": null,
            "level": "error",
            "spans": [{
                "file_name": "src/lib.rs",
                "byte_start": 0,
                "byte_end": 1,
                "line_start": 1,
                "line_end": 1,
                "column_start": 1,
                "column_end": 2,
                "is_primary": true,
                "text": [],
                "label": null,
                "suggested_replacement": null,
                "suggestion_applicability": null,
                "expansion": null,
            }],
            "children": [{
                "message": "see foo-unittest for more details",
                "code": null,
                "level": "help",
                "spans": [],
                "children": [],
                "rendered": null,
            }],
            "rendered": "unused variable in foo-unittest",
        })
        .to_string();

        let contents = vec![format!("{msg1}\n{msg2}")];
        let root = Path::new("/project");

        let result = parse_diagnostics(&contents, root).unwrap();
        assert_eq!(
            result.len(),
            1,
            "messages differing only in children should be deduplicated"
        );
    }

    #[test]
    fn test_parse_non_message_json() {
        let contents = vec![r#"{"reason":"compiler-artifact","target":"foo"}"#.to_owned()];
        let root = Path::new("/project");

        let result = parse_diagnostics(&contents, root).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["reason"], "compiler-artifact");
    }
}
