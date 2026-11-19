/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::Entry;
use crate::efm::*;

#[test]
fn test_plus_modifier() {
    let input = "%+D";
    let expected = FormatSpecifier {
        specifier: 'D',
        modifier: PrefixModifier::Plus,
    };
    assert_eq!(parse_format_specifier(input), Ok(("", expected)));
}

#[test]
fn test_minus_modifier() {
    let input = "%-A";
    let expected = FormatSpecifier {
        specifier: 'A',
        modifier: PrefixModifier::Minus,
    };
    assert_eq!(parse_format_specifier(input), Ok(("", expected)));
}

#[test]
fn test_none_modifier() {
    let input = "%X";
    let expected = FormatSpecifier {
        specifier: 'X',
        modifier: PrefixModifier::None,
    };
    assert_eq!(parse_format_specifier(input), Ok(("", expected)));
}

#[test]
fn test_remaining_input() {
    let input = "%W_extra";
    let expected = FormatSpecifier {
        specifier: 'W',
        modifier: PrefixModifier::None,
    };
    assert_eq!(parse_format_specifier(input), Ok(("_extra", expected)));
}

#[test]
fn test_invalid_missing_percent() {
    let input = "D";
    assert!(parse_format_specifier(input).is_err());
}

#[test]
fn test_invalid_missing_specifier() {
    let input = "%";
    assert!(parse_format_specifier(input).is_err());
}

#[test]
fn test_invalid_modifier_only() {
    let input = "%+";
    assert!(parse_format_specifier(input).is_err());
}

#[test]
fn test_invalid_specifier_char() {
    let input = "%B";
    assert!(parse_format_specifier(input).is_err());
}

#[test]
fn test_empty_input() {
    let input = "";
    assert!(parse_format_specifier(input).is_err());
}

#[test]
fn parse_to_get_file_name() -> Result<(), ParseEfmError> {
    let error_format = "%f:%l:%c: %m";
    let line = "main.rs:42:4: the error message";
    let efm = SingleErrorFormatRule::new(error_format)?;
    let res = efm.match_line(line);
    assert!(res.is_ok());
    let res = res.unwrap();
    assert!(res.is_some());

    let res = res.unwrap();
    assert!(res.filename.is_some());
    assert_eq!(res.filename.unwrap(), "main.rs");
    assert_eq!(res.lnum.unwrap(), 42);
    assert_eq!(res.col.unwrap(), 4);
    assert_eq!(res.message.unwrap(), "the error message");

    Ok(())
}

#[test]
fn parse_literal_brackets() -> Result<(), ParseEfmError> {
    let error_format = "(): %m";
    let line = "(): the error message";
    let efm = SingleErrorFormatRule::new(error_format)?;
    let res = efm.match_line(line);
    assert!(res.is_ok());
    let res = res.unwrap();
    assert!(res.is_some());

    let res = res.unwrap();
    assert!(res.message.is_some());
    assert_eq!(res.message.unwrap(), "the error message");

    Ok(())
}

#[test]
fn parse_escape_brackets() -> Result<(), ParseEfmError> {
    let error_format = r"\(abc\)\+: %m";
    // it will be parsed as regex pattern: ^(abc)+: (?<m>.+)$
    let line = "abcabc: the error message";
    let efm = SingleErrorFormatRule::new(error_format)?;
    let res = efm.match_line(line);
    assert!(res.is_ok());
    let res = res.unwrap();
    assert!(res.is_some());

    let res = res.unwrap();
    assert!(res.message.is_some());
    assert_eq!(res.message.unwrap(), "the error message");

    Ok(())
}

#[test]
fn parse_literal_bar() -> Result<(), ParseEfmError> {
    let error_format = "|: %m";
    let line = "|: the error message";
    let efm = SingleErrorFormatRule::new(error_format)?;
    let res = efm.match_line(line);
    assert!(res.is_ok());
    let res = res.unwrap();
    assert!(res.is_some());

    let res = res.unwrap();
    assert!(res.message.is_some());
    assert_eq!(res.message.unwrap(), "the error message");

    Ok(())
}

#[test]
fn parse_regex_bar() -> Result<(), ParseEfmError> {
    let error_format = r"\(abc\|efg\): %m";
    let line = "abc: the error message";
    let efm = SingleErrorFormatRule::new(error_format)?;
    let res = efm.match_line(line);
    assert!(res.is_ok());
    let res = res.unwrap();
    assert!(res.is_some());

    let res = res.unwrap();
    assert!(res.message.is_some());
    assert_eq!(res.message.unwrap(), "the error message");

    let line2 = "efg: the error message";
    let res2 = efm.match_line(line2);
    assert!(res2.is_ok());
    let res2 = res2.unwrap();
    assert!(res2.is_some());

    let res2 = res2.unwrap();
    assert!(res2.message.is_some());
    assert_eq!(res2.message.unwrap(), "the error message");

    Ok(())
}

fn assert_entry_matches(
    result: Result<Option<Entry>, ParseEfmError>,
    line: &str,
    expected_opt: Option<Entry>,
) {
    match result {
        Ok(Some(got_entry)) => {
            if let Some(mut expected_entry) = expected_opt {
                // `original_err_lines` is always set by `match_line` on success
                expected_entry.original_err_lines = vec![line.to_owned()];
                // Clear it from got_entry if we don't want to compare it strictly, or ensure it's set in expected.
                // For these tests, we'll ensure original_err_lines matches.
                assert_eq!(got_entry, expected_entry, "Entry mismatch for line: {line}");
            } else {
                panic!("Expected None, got Some({got_entry:?}) for line: {line}");
            }
        }
        Ok(None) => {
            if let Some(expected) = &expected_opt {
                panic!("Expected Some({expected:?}), got None for line: {line}");
            }
            // Both None, success.
        }
        Err(e) => {
            panic!("match_line returned an error: {e:?} for line: {line}");
        }
    }
}

#[test]
fn test_basic_format_f_l_c_m() -> Result<(), ParseEfmError> {
    let errorformat = "%f:%l:%c: %m";
    let efm = SingleErrorFormatRule::new(errorformat)?;

    let line1 = "golint.new.go:3:5: exported var V should have comment or be unexported";
    let mut expected1 = Entry::new();
    expected1.filename = Some("golint.new.go".to_owned());
    expected1.lnum = Some(3);
    expected1.col = Some(5);
    expected1.message = Some("exported var V should have comment or be unexported".to_owned());
    assert_entry_matches(efm.match_line(line1), line1, Some(expected1));

    let line2 = "";
    assert_entry_matches(efm.match_line(line2), line2, None);

    let line3 = "golint.new.go:3:5"; // Missing message part
    assert_entry_matches(efm.match_line(line3), line3, None);

    let line4 = r#"/path/t\ o/file:1:4: message"#;
    let mut expected4 = Entry::new();
    expected4.filename = Some(r#"/path/t\ o/file"#.to_owned());
    expected4.lnum = Some(1);
    expected4.col = Some(4);
    expected4.message = Some("message".to_owned());
    assert_entry_matches(efm.match_line(line4), line4, Some(expected4));

    let line5 = r#"\path\to\file:1:4: message"#;
    let mut expected5 = Entry::new();
    expected5.filename = Some(r#"\path\to\file"#.to_owned());
    expected5.lnum = Some(1);
    expected5.col = Some(4);
    expected5.message = Some("message".to_owned());
    assert_entry_matches(efm.match_line(line5), line5, Some(expected5));

    Ok(())
}

#[test]
fn test_empty_errorformat() -> Result<(), ParseEfmError> {
    let errorformat = ""; // Regex becomes "^$"
    let efm = SingleErrorFormatRule::new(errorformat)?;

    let line1 = "";
    let expected1 = Entry::new(); // All fields None, original_err_lines will be vec![""]
    assert_entry_matches(efm.match_line(line1), line1, Some(expected1));

    let line2 = "anything";
    assert_entry_matches(efm.match_line(line2), line2, None);
    Ok(())
}

#[test]
fn test_windows_paths_f_m() -> Result<(), ParseEfmError> {
    let errorformat = "%f:%m";
    let efm = SingleErrorFormatRule::new(errorformat)?;

    let line1 = "c:/foo/bar.c:msg:hi:14";
    let mut expected1 = Entry::new();
    expected1.filename = Some("c:/foo/bar.c".to_owned());
    expected1.message = Some("msg:hi:14".to_owned());
    assert_entry_matches(efm.match_line(line1), line1, Some(expected1));

    let line2 = "hoge/c:/foo/bar.c:msg:hi:14";
    let mut expected2 = Entry::new();
    // %f is `(?<f>(?:[[:alpha:]]:)?(?:\\ |[^ ])+?)` which is non-greedy.
    // It will match "hoge/c" before the first literal ':' in the EFM.
    expected2.filename = Some("hoge/c".to_owned());
    expected2.message = Some("/foo/bar.c:msg:hi:14".to_owned());
    assert_entry_matches(efm.match_line(line2), line2, Some(expected2));
    Ok(())
}

#[test]
fn test_pointer_pattern_with_tab() -> Result<(), ParseEfmError> {
    // POINTER_PATTERN regex is r"(?<p>[- \t.]*)" (assuming \t or literal tab)
    // The provided code's POINTER_PATTERN is r"(?<p>[- <literal_tab_char> .]*)"
    let errorformat = "%p";
    let efm = SingleErrorFormatRule::new(errorformat)?;

    let line = "-\t.\t -"; // Input line with actual tab characters
    let mut expected = Entry::new();
    expected.pointer_line = Some(line.to_owned());
    assert_entry_matches(efm.match_line(line), line, Some(expected));
    Ok(())
}

#[test]
#[allow(non_snake_case)]
fn test_ERR_NUMBER_PATTERN_n_not_in_entry() -> Result<(), ParseEfmError> {
    // %n uses capture group "n", but `match_line` doesn't assign it to a field in `Entry`.
    // The regex will still use it for matching.
    let errorformat = "Error %n: %m";
    let efm = SingleErrorFormatRule::new(errorformat)?;

    let line = "Error 123: Something bad happened";
    let mut expected = Entry::new();
    expected.message = Some("Something bad happened".to_owned());
    expected.error_number = Some(123);
    // No field for 'nr' or 'error_number' in the current Entry structure based on match_line.
    assert_entry_matches(efm.match_line(line), line, Some(expected));
    Ok(())
}

#[test]
fn test_star_bracket_pattern() -> Result<(), ParseEfmError> {
    // %*[...] -> `pattern`+
    let errorformat = "%f: %*[^:]: %m"; // %*[^:] becomes [^:]+
    let efm = SingleErrorFormatRule::new(errorformat)?;
    let line = "file.c: abcdef : error message";
    let mut expected = Entry::new();
    expected.filename = Some("file.c".to_owned());
    expected.message = Some("error message".to_owned()); // " abcdef " is consumed by %*[^:]
    assert_entry_matches(efm.match_line(line), line, Some(expected));

    let line_no_match = "file.c:: error message"; // %*[^:] requires at least one char
    assert_entry_matches(efm.match_line(line_no_match), line_no_match, None);
    Ok(())
}

#[test]
fn test_star_escape_pattern() -> Result<(), ParseEfmError> {
    // %*\D -> \D+
    let errorformat = "%f %*\\D %m"; // %*\\D becomes \D+
    let efm = SingleErrorFormatRule::new(errorformat)?;
    let line = "item.txt nonDigits Details";
    let mut expected = Entry::new();
    expected.filename = Some("item.txt".to_owned());
    expected.message = Some("Details".to_owned()); // "nonDigits" consumed by %*\\D
    assert_entry_matches(efm.match_line(line), line, Some(expected));

    let line_no_match = "item.txt 123 Details"; // %*\\D won't match digits
    assert_entry_matches(efm.match_line(line_no_match), line_no_match, None);
    Ok(())
}

#[test]
fn test_literal_percent_char() -> Result<(), ParseEfmError> {
    // %% -> % (literal percent)
    let errorformat = "%% %f %m";
    let efm = SingleErrorFormatRule::new(errorformat)?;
    let line = "% main.c Error here";
    let mut expected = Entry::new();
    expected.filename = Some("main.c".to_owned());
    expected.message = Some("Error here".to_owned());
    assert_entry_matches(efm.match_line(line), line, Some(expected));
    Ok(())
}

#[test]
fn test_special_percent_chars_as_regex() -> Result<(), ParseEfmError> {
    // `%.` becomes regex `.` (any char)
    // `%#` becomes regex `*` (quantifier for previous item)
    // This tests the implemented behavior, which may differ from Vim's literal interpretation.

    // Test `%.` (becomes regex `.`)
    let efm_dot = SingleErrorFormatRule::new("a%.c")?; // Regex: ^a.c$
    assert_entry_matches(efm_dot.match_line("abc"), "abc", Some(Entry::new()));
    assert_entry_matches(efm_dot.match_line("axc"), "axc", Some(Entry::new()));
    assert_entry_matches(efm_dot.match_line("ac"), "ac", None); // `.` needs one char
    assert_entry_matches(efm_dot.match_line("a_extra_c"), "a_extra_c", None);

    // Test `%#` (becomes regex `*`) - applied to 'a' in `a%#c` -> `a*c`
    let efm_hash = SingleErrorFormatRule::new("a%#c")?; // Regex: ^a*c$
    assert_entry_matches(efm_hash.match_line("c"), "c", Some(Entry::new())); // 'a' zero times
    assert_entry_matches(efm_hash.match_line("ac"), "ac", Some(Entry::new())); // 'a' one time
    assert_entry_matches(efm_hash.match_line("aaac"), "aaac", Some(Entry::new())); // 'a' three times
    assert_entry_matches(efm_hash.match_line("ab_c"), "ab_c", None); // 'b' not allowed by `a*`

    // Test `%?` (becomes regex `?`) - applied to 'a' in `a%?c` -> `a?c`
    let efm_q = SingleErrorFormatRule::new("a%?c")?; // Regex: ^a?c$
    assert_entry_matches(efm_q.match_line("c"), "c", Some(Entry::new()));
    assert_entry_matches(efm_q.match_line("ac"), "ac", Some(Entry::new()));
    assert_entry_matches(efm_q.match_line("aac"), "aac", None);
    Ok(())
}

#[test]
fn test_normal_chars_escaped_in_regex() -> Result<(), ParseEfmError> {
    // Chars like '.', '+', '*' should be escaped if they are normal chars in efm.
    let errorformat = "file.ext calcio+bonus* %m";
    let efm = SingleErrorFormatRule::new(errorformat)?;
    // Expected regex part: file\.ext calcio\+bonus\*
    // You can print efm.regex.to_owned() to verify this.
    // println!("Regex for test_normal_chars_escaped_in_regex: {}", efm.regex.to_owned());

    let line = "file.ext calcio+bonus* The message";
    let mut expected = Entry::new();
    expected.message = Some("The message".to_owned());
    assert_entry_matches(efm.match_line(line), line, Some(expected));

    let line_no_match = "fileAext calcio+bonus* The message";
    assert_entry_matches(efm.match_line(line_no_match), line_no_match, None);

    let line_no_match2 = "file.ext calcio bonus* The message"; // missing +
    assert_entry_matches(efm.match_line(line_no_match2), line_no_match2, None);
    Ok(())
}

#[test]
fn test_format_specifier_parsing() -> Result<(), ParseEfmError> {
    // Tests that Efm::new parses the initial format specifier correctly.
    // The specifier itself doesn't alter single-line regex matching in `match_line`.
    let efm1 = SingleErrorFormatRule::new("%+A%f:%l")?;
    assert_eq!(
        efm1.format_specifier,
        Some(FormatSpecifier {
            specifier: 'A',
            modifier: PrefixModifier::Plus
        })
    );

    let efm2 = SingleErrorFormatRule::new("%-C%f:%l")?;
    assert_eq!(
        efm2.format_specifier,
        Some(FormatSpecifier {
            specifier: 'C',
            modifier: PrefixModifier::Minus
        })
    );

    let efm3 = SingleErrorFormatRule::new("%D%f:%l")?;
    assert_eq!(
        efm3.format_specifier,
        Some(FormatSpecifier {
            specifier: 'D',
            modifier: PrefixModifier::None
        })
    );

    let efm4 = SingleErrorFormatRule::new("%f:%l")?; // No multi-line specifier
    assert_eq!(efm4.format_specifier, None);
    Ok(())
}

#[test]
fn test_invalid_efm_pattern() {
    // %Y is not a valid single char pattern like %f, nor a special %., %*, etc.
    // `parse_format_pattern` should fail for 'Y'.
    let res = SingleErrorFormatRule::new("%Y");
    println!("{res:?}");
    assert!(res.is_err());
    match res {
        Err(ParseEfmError::InvalidFormat(_)) => {} // Expected
        _ => panic!("Expected InvalidFormat error for invalid pattern %Y, got {res:?}"),
    }
}

#[test]
fn test_filename_with_unescaped_spaces() -> Result<(), ParseEfmError> {
    // The FILE_PATTERN `(?:\\ |[^ ])+?` means it matches an escaped space OR a non-space.
    // It cannot match an unescaped space if that space is part of the filename.

    // Case 1: Space in efm separates %f from next part
    let errorformat_space_sep = "%f %l: %m";
    let efm_space_sep = SingleErrorFormatRule::new(errorformat_space_sep)?;
    let line_unescaped = "my file.c 10: error";
    // %f will match "my". Then " file.c..." won't match " %l..." because of leading space.
    assert_entry_matches(
        efm_space_sep.match_line(line_unescaped),
        line_unescaped,
        None,
    );

    let line_unescaped_escaped = r#"my\ file.c 10: error"#;
    let mut expected_escaped = Entry::new();
    expected_escaped.filename = Some(r#"my\ file.c"#.to_owned());
    expected_escaped.lnum = Some(10);
    expected_escaped.message = Some("error".to_owned());
    assert_entry_matches(
        efm_space_sep.match_line(line_unescaped_escaped),
        line_unescaped_escaped,
        Some(expected_escaped),
    );

    // Case 2: Colon in efm separates %f
    let errorformat_colon_sep = "%f:%l: %m";
    let efm_colon_sep = SingleErrorFormatRule::new(errorformat_colon_sep)?;
    let line_unescaped_colon = "my file.c:10: error";
    // %f will match "my". Then " file.c..." won't match ":%l..."
    assert_entry_matches(
        efm_colon_sep.match_line(line_unescaped_colon),
        line_unescaped_colon,
        None,
    );

    let line_escaped_colon = r#"my\ file.c:10: error"#;
    let mut expected_escaped_colon = Entry::new();
    expected_escaped_colon.filename = Some(r#"my\ file.c"#.to_owned());
    expected_escaped_colon.lnum = Some(10);
    expected_escaped_colon.message = Some("error".to_owned());
    assert_entry_matches(
        efm_colon_sep.match_line(line_escaped_colon),
        line_escaped_colon,
        Some(expected_escaped_colon),
    );

    Ok(())
}
