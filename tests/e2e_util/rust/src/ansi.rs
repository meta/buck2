/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Terminal escape sequences in buck2's output.

use std::sync::LazyLock;

use regex::Regex;

/// `text` without its ANSI escape sequences: the console colors parts of its output, such as the
/// counts in `Tests finished: Pass 1. Fail 0. ...`, even when stderr is not a terminal.
pub fn remove_ansi_escape_sequences(text: &str) -> String {
    static ANSI_ESCAPE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").unwrap());
    ANSI_ESCAPE.replace_all(text, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_sequences_are_removed() {
        assert_eq!(
            remove_ansi_escape_sequences(
                "Tests finished: \x1b[mPass 15\x1b[m. Fail 0. \x1b[1;31mFatal 1\x1b[0m"
            ),
            "Tests finished: Pass 15. Fail 0. Fatal 1"
        );
    }

    #[test]
    fn test_two_character_sequences_are_removed() {
        assert_eq!(remove_ansi_escape_sequences("a\x1bMb\x1b_c"), "abc");
    }

    #[test]
    fn test_text_without_escapes_is_kept() {
        let text = "[2026-11-03] Pass 1. [x] ESC is \\x1b\n";
        assert_eq!(remove_ansi_escape_sequences(text), text);
    }
}
