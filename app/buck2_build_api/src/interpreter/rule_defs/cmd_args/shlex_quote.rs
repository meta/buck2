/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// Quote string for shell.
///
/// This follows the quoting conventions of [`shlex`](https://github.com/comex/rust-shlex/) 1.0.
///
/// Generally shell quoting is unspecified, for example `a` can be quoted as:
/// - `a`
/// - `"a"`
/// - `'a'`
/// - and many more
///
/// to be used in shell. But we use shell quoting to generate arguments for argfiles.
/// And certain programs expect certain quoting style (for example, `cl.exe` expect double-quoted).
/// Additionally, we probably also incorrectly use shell quoting for `cmd.exe`.
///
/// Long story short, we should not depend on possible correct behavior change in `shlex` crate.
pub(crate) fn shlex_quote(in_str: String) -> String {
    if in_str.is_empty() {
        "\"\"".into()
    } else if in_str.bytes().any(|c| {
        matches!(
            c as char,
            '|' | '&'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '$'
                | '`'
                | '\\'
                | '"'
                | '\''
                | ' '
                | '\t'
                | '\r'
                | '\n'
                | '*'
                | '?'
                | '['
                | '#'
                | '~'
                | '='
                | '%'
                | '{'
                | '}'
        )
    }) {
        let mut out = String::with_capacity(in_str.len() + 2);
        out.push('"');
        let mut start = 0;
        for (i, c) in in_str.bytes().enumerate() {
            if matches!(c, b'$' | b'`' | b'"' | b'\\') {
                // ASCII bytes always lie on UTF-8 boundaries.
                out.push_str(&in_str[start..i]);
                out.push('\\');
                start = i;
            }
        }
        out.push_str(&in_str[start..]);
        out.push('"');
        out
    } else {
        in_str
    }
}

#[cfg(test)]
mod tests {
    use crate::interpreter::rule_defs::cmd_args::shlex_quote::shlex_quote;

    #[test]
    fn test_quote() {
        assert_eq!(shlex_quote("foobar".to_owned()), "foobar");
        assert_eq!(shlex_quote("foo bar".to_owned()), "\"foo bar\"");
        assert_eq!(shlex_quote("\"".to_owned()), "\"\\\"\"");
        assert_eq!(shlex_quote("".to_owned()), "\"\"");
    }

    #[test]
    fn test_quote_special_characters() {
        for c in "|&;<>()' \t\r\n*?[#~=%{}".chars() {
            assert_eq!(
                shlex_quote(format!("before{c}after")),
                format!("\"before{c}after\"")
            );
        }
        for c in "$`\"\\".chars() {
            assert_eq!(
                shlex_quote(format!("before{c}after")),
                format!("\"before\\{c}after\"")
            );
        }
    }

    #[test]
    fn test_quote_utf8_and_control_characters() {
        // Control characters other than tab, CR and LF are left unquoted.
        let unchanged = "café/日本語/🦀\0\u{000b}\u{000c}\u{007f}";
        assert_eq!(shlex_quote(unchanged.to_owned()), unchanged);
        assert_eq!(
            shlex_quote("café $日本語`🦀\"\\".to_owned()),
            "\"café \\$日本語\\`🦀\\\"\\\\\""
        );
    }

    #[test]
    fn test_quote_adjacent_escapes() {
        assert_eq!(shlex_quote("$`\"\\".to_owned()), "\"\\$\\`\\\"\\\\\"");
        assert_eq!(shlex_quote("\\\\".to_owned()), "\"\\\\\\\\\"");
    }

    #[test]
    fn test_quote_braces() {
        // Bare braces would be brace-expanded by `bash` into several words.
        assert_eq!(shlex_quote("a{b,c}".to_owned()), "\"a{b,c}\"");
        assert_eq!(shlex_quote("{a..c}".to_owned()), "\"{a..c}\"");
        assert_eq!(shlex_quote("x{2,5}".to_owned()), "\"x{2,5}\"");
    }
}
