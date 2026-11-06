/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::Buck;

/// The files `files/**/*` matches, as the test data's `TARGETS.fixture` prints them on
/// `targets root//:`.
pub async fn get_files(buck: &Buck) -> anyhow::Result<Vec<String>> {
    let res = buck.targets(&["root//:"]).await?;
    parse_files(&res.stderr)
}

fn parse_files(stderr: &str) -> anyhow::Result<Vec<String>> {
    let files = stderr
        .lines()
        .find_map(|x| x.split_once("Files: ").map(|(_, files)| files))
        .with_context(|| format!("Missing files output: {stderr}"))?;
    Ok(serde_json::from_str(files)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_come_from_the_first_files_line() {
        assert_eq!(
            parse_files("noise\n[time] Files: [\"files/abc\", \"files/d/empty\"]\nFiles: []\n")
                .unwrap(),
            ["files/abc", "files/d/empty"]
        );
    }

    #[test]
    fn test_no_files_line_is_an_error() {
        assert!(parse_files("noise\n").is_err());
    }
}
