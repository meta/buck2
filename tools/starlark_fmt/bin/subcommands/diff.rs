/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::path::Path;

use either::Either;
use similar::ChangeTag;
use similar::TextDiff;
use starlark_fmt_lib::Config;
use termcolor::Color;
use termcolor::ColorChoice;
use termcolor::ColorSpec;
use termcolor::StandardStream;
use termcolor::WriteColor;
use tracing::info;

use super::ProcessedFile;

pub fn diff_file(path: &Path, show_all: bool, config: &Config) -> anyhow::Result<()> {
    let processed = ProcessedFile::new(path, config)?;

    if processed.has_changes() {
        let mut stdout = StandardStream::stdout(ColorChoice::Auto);

        writeln!(stdout, "--- {}", processed.path.display())?;

        let diff = TextDiff::from_lines(&processed.source, &processed.formatted);
        let changes = if show_all {
            Either::Left(diff.iter_all_changes())
        } else {
            Either::Right(
                diff.iter_all_changes()
                    .filter(|change| change.tag() != ChangeTag::Equal),
            )
        };

        for change in changes {
            let (sign, color) = match change.tag() {
                ChangeTag::Delete => ('-', Some(Color::Red)),
                ChangeTag::Insert => ('+', Some(Color::Green)),
                ChangeTag::Equal => (' ', None),
            };

            stdout.set_color(ColorSpec::new().set_fg(color))?;
            write!(stdout, "{sign}{change}")?;
        }
        stdout.reset()?;
    } else {
        info!("{}: no changes", processed.path.display());
    }

    Ok(())
}
