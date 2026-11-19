/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::*;

#[test]
fn test_parse_error_format_simple() {
    let error_format = vec!["%f:%l: %m".to_owned()];
    let lines = vec!["main.rs:10: expected `;`, found `}`".to_owned()];
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.filename, Some("main.rs".to_owned()));
    assert_eq!(entry.lnum, Some(10));
    assert_eq!(entry.message, Some("expected `;`, found `}`".to_owned()));
}

#[test]
fn test_parse_error_format_with_type() {
    let error_format = vec!["%f:%l:%t%*[^:]: %m".to_owned()];
    let lines = vec!["main.rs:10:error: expected `;`, found `}`".to_owned()];
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.filename, Some("main.rs".to_owned()));
    assert_eq!(entry.lnum, Some(10));
    assert_eq!(entry.error_type, Some("e".to_owned()));
    assert_eq!(entry.message, Some("expected `;`, found `}`".to_owned()));
}
#[test]
fn test_parse_error_format_multiple_patterns() {
    let error_format = vec!["%f:%l:%c: %m".to_owned(), "%f:%l: %m".to_owned()];
    let lines = vec![
        "main.rs:10: expected `;`, found `}`".to_owned(),
        "lib.rs:20:15: undefined variable".to_owned(),
    ];
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 2);

    let entry1 = &entries[0];
    assert_eq!(entry1.filename, Some("main.rs".to_owned()));
    assert_eq!(entry1.lnum, Some(10));
    assert_eq!(entry1.message, Some("expected `;`, found `}`".to_owned()));

    let entry2 = &entries[1];
    assert_eq!(entry2.filename, Some("lib.rs".to_owned()));
    assert_eq!(entry2.lnum, Some(20));
    assert_eq!(entry2.col, Some(15));
    assert_eq!(entry2.message, Some("undefined variable".to_owned()));
}

// test multi line errorformat
#[test]
fn test_parse_error_format_multiline() {
    let error_format = split_lines(
        "\
%EError\\ %n
%Cline\\ %l
%Ccolumn\\ %c
%Z%m",
    );
    let lines = split_lines(
        "\
Error 275
line 42
column 3
' ' expected after '--'",
    );
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.error_number, Some(275));
    assert_eq!(entry.lnum, Some(42));
    assert_eq!(entry.col, Some(3));
    assert_eq!(entry.message, Some("' ' expected after '--'".to_owned()));
}

#[test]
fn test_parse_python_unittest_error() {
    let error_format = split_lines(
        r#"
%C\ %.%#
%A\ \ File\ \"%f\"\\,\ line\ %l%.%#
%Z\(?=%[%^\ ]\)%m
"#,
    );
    let error_msg = r#"
==============================================================
FAIL: testGetTypeIdCachesResult (dbfacadeTest.DjsDBFacadeTest)
--------------------------------------------------------------
Traceback (most recent call last):
  File "unittests/dbfacadeTest.py", line 89, in testFoo
    self.assertEquals(34, dtid)
  File "/usr/lib/python3.8/unittest.py", line 286, in
 failUnlessEqual
    raise self.failureException, \
AssertionError: 34 != 33

--------------------------------------------------------------
Ran 27 tests in 0.063s
"#;
    let lines = split_lines(error_msg)
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.filename, Some("unittests/dbfacadeTest.py".to_owned()));
    assert_eq!(entry.lnum, Some(89));
    assert_eq!(entry.message, Some("AssertionError: 34 != 33".to_owned()));
    assert_eq!(entry.error_type, Some("A".to_owned()));
}

// test push/pop file
#[test]
fn test_parse_error_format_with_file_push_pop() {
    let error_format = split_lines(
        "\
%+P[%f]
(%l\\,%c)%*[\\ ]%t%*[^:]:\\ %m
%-Q",
    );
    let lines = split_lines(
        "\
[a1.tt]
(1,17)  error: ';' missing
(21,2)  warning: variable 'z' not defined
(67,3)  error: end of file found before string ended

[a2.tt]

[a3.tt]
NEW compiler v1.1
(2,2)   warning: variable 'x' not defined
(67,3)  warning: 's' already defined",
    );
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 5);

    // First file (a1.tt) entries
    let entry1 = &entries[0];
    assert_eq!(entry1.filename, Some("a1.tt".to_owned()));
    assert_eq!(entry1.lnum, Some(1));
    assert_eq!(entry1.col, Some(17));
    assert_eq!(entry1.error_type, Some("e".to_owned()));
    assert_eq!(entry1.message, Some("';' missing".to_owned()));

    let entry2 = &entries[1];
    assert_eq!(entry2.filename, Some("a1.tt".to_owned()));
    assert_eq!(entry2.lnum, Some(21));
    assert_eq!(entry2.col, Some(2));
    assert_eq!(entry2.error_type, Some("w".to_owned()));
    assert_eq!(entry2.message, Some("variable 'z' not defined".to_owned()));

    let entry3 = &entries[2];
    assert_eq!(entry3.filename, Some("a1.tt".to_owned()));
    assert_eq!(entry3.lnum, Some(67));
    assert_eq!(entry3.col, Some(3));
    assert_eq!(entry3.error_type, Some("e".to_owned()));
    assert_eq!(
        entry3.message,
        Some("end of file found before string ended".to_owned())
    );

    // Third file (a3.tt) entries
    let entry4 = &entries[3];
    assert_eq!(entry4.filename, Some("a3.tt".to_owned()));
    assert_eq!(entry4.lnum, Some(2));
    assert_eq!(entry4.col, Some(2));
    assert_eq!(entry4.error_type, Some("w".to_owned()));
    assert_eq!(entry4.message, Some("variable 'x' not defined".to_owned()));

    let entry5 = &entries[4];
    assert_eq!(entry5.filename, Some("a3.tt".to_owned()));
    assert_eq!(entry5.lnum, Some(67));
    assert_eq!(entry5.col, Some(3));
    assert_eq!(entry5.error_type, Some("w".to_owned()));
    assert_eq!(entry5.message, Some("'s' already defined".to_owned()));
}

#[test]
fn test_parse_error_format_multiline_ignore() {
    let error_format = vec![
        "%-EError %n".to_owned(),
        "%CLine %l".to_owned(),
        "%ZMessage: %m".to_owned(),
    ];
    let lines = vec![
        "Error 100".to_owned(),
        "Line 10".to_owned(),
        "Message: This should be ignored".to_owned(),
    ];
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(
        entries.len(),
        0,
        "Ignored multi-line message should not produce entries"
    );
}

#[test]
fn test_parse_error_format_multiline_ignore_mixed_with_normal() {
    let error_format = vec![
        "%-EError %n".to_owned(),
        "%CLine %l".to_owned(),
        "%ZMessage: %m".to_owned(),
        "%f:%l: %m".to_owned(), // A regular format
    ];
    let lines = vec![
        "Error 100".to_owned(),                          // Start of ignored multi-line
        "Line 10".to_owned(),                            // Continuation of ignored multi-line
        "Message: This should be ignored".to_owned(),    // End of ignored multi-line
        "normal.rs:1: regular error message".to_owned(), // A normal error line
    ];
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(
        entries.len(),
        1,
        "Should only capture the non-ignored message"
    );
    let entry = &entries[0];
    assert_eq!(entry.filename, Some("normal.rs".to_owned()));
    assert_eq!(entry.lnum, Some(1));
    assert_eq!(entry.message, Some("regular error message".to_owned()));
}

#[test]
fn test_parse_error_format_with_dir_push_pop_wihtout_filename() {
    let error_format = vec![
        "%+D[%f]".to_owned(), // Push directory
        "(%l\\,%c)%*[\\ ]%t%*[^:]:\\ %m".to_owned(),
        "%-X".to_owned(), // Pop directory
    ];
    let lines = split_lines(
        "\
[/path/to/src]
(1,17)  error: ';' missing
(21,2)  warning: variable 'z' not defined

[/path/to/lib]
(2,2)   warning: variable 'x' not defined
(67,3)  warning: 's' already defined

[/path/to/test]
(5,1)   error: test failed",
    );
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 5);

    // First directory (/path/to/src) entries
    let entry1 = &entries[0];
    assert_eq!(entry1.filename, Some("/path/to/src".to_owned()));
    assert_eq!(entry1.lnum, Some(1));
    assert_eq!(entry1.col, Some(17));
    assert_eq!(entry1.error_type, Some("e".to_owned()));
    assert_eq!(entry1.message, Some("';' missing".to_owned()));

    let entry2 = &entries[1];
    assert_eq!(entry2.filename, Some("/path/to/src".to_owned()));
    assert_eq!(entry2.lnum, Some(21));
    assert_eq!(entry2.col, Some(2));
    assert_eq!(entry2.error_type, Some("w".to_owned()));
    assert_eq!(entry2.message, Some("variable 'z' not defined".to_owned()));

    // Second directory (/path/to/lib) entries
    let entry3 = &entries[2];
    assert_eq!(entry3.filename, Some("/path/to/lib".to_owned()));
    assert_eq!(entry3.lnum, Some(2));
    assert_eq!(entry3.col, Some(2));
    assert_eq!(entry3.error_type, Some("w".to_owned()));
    assert_eq!(entry3.message, Some("variable 'x' not defined".to_owned()));

    let entry4 = &entries[3];
    assert_eq!(entry4.filename, Some("/path/to/lib".to_owned()));
    assert_eq!(entry4.lnum, Some(67));
    assert_eq!(entry4.col, Some(3));
    assert_eq!(entry4.error_type, Some("w".to_owned()));
    assert_eq!(entry4.message, Some("'s' already defined".to_owned()));

    // Third directory (/path/to/test) entry
    let entry5 = &entries[4];
    assert_eq!(entry5.filename, Some("/path/to/test".to_owned()));
    assert_eq!(entry5.lnum, Some(5));
    assert_eq!(entry5.col, Some(1));
    assert_eq!(entry5.error_type, Some("e".to_owned()));
    assert_eq!(entry5.message, Some("test failed".to_owned()));
}

#[cfg(unix)]
#[test]
fn test_parse_error_format_with_relative_paths() {
    let error_format = vec![
        "%+D[%f]".to_owned(), // Push directory
        "[%f](%l\\,%c)%*[\\ ]%t%*[^:]:\\ %m".to_owned(),
        "%-X".to_owned(), // Pop directory
    ];
    let lines = split_lines(
        "\
[/path/to/src]
[main.rs](1,17)  error: ';' missing
[utils.rs](21,2)  warning: variable 'z' not defined

[/path/to/lib]
[helper.rs](2,2)   warning: variable 'x' not defined",
    );
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 3);

    // First directory entries with relative paths
    let entry1 = &entries[0];
    assert_eq!(entry1.filename, Some("/path/to/src/main.rs".to_owned()));
    assert_eq!(entry1.lnum, Some(1));
    assert_eq!(entry1.col, Some(17));
    assert_eq!(entry1.error_type, Some("e".to_owned()));
    assert_eq!(entry1.message, Some("';' missing".to_owned()));

    let entry2 = &entries[1];
    assert_eq!(entry2.filename, Some("/path/to/src/utils.rs".to_owned()));
    assert_eq!(entry2.lnum, Some(21));
    assert_eq!(entry2.col, Some(2));
    assert_eq!(entry2.error_type, Some("w".to_owned()));
    assert_eq!(entry2.message, Some("variable 'z' not defined".to_owned()));

    // Second directory entry with relative path
    let entry3 = &entries[2];
    assert_eq!(entry3.filename, Some("/path/to/lib/helper.rs".to_owned()));
    assert_eq!(entry3.lnum, Some(2));
    assert_eq!(entry3.col, Some(2));
    assert_eq!(entry3.error_type, Some("w".to_owned()));
    assert_eq!(entry3.message, Some("variable 'x' not defined".to_owned()));
}

// tests for %-G
#[test]
fn test_parse_error_format_with_ignore_pattern() {
    let error_format = vec![
        "%f:%l:%c: %m".to_owned(),
        "%-G%.%#".to_owned(), // Ignore any line that matches this pattern
    ];
    let lines = split_lines(
        "\
main.rs:10:5: error: expected `;`
This line should be ignored
Another line to ignore
lib.rs:20:15: warning: unused variable",
    );
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 2);

    let entry1 = &entries[0];
    assert_eq!(entry1.filename, Some("main.rs".to_owned()));
    assert_eq!(entry1.lnum, Some(10));
    assert_eq!(entry1.col, Some(5));
    assert_eq!(entry1.message, Some("error: expected `;`".to_owned()));

    let entry2 = &entries[1];
    assert_eq!(entry2.filename, Some("lib.rs".to_owned()));
    assert_eq!(entry2.lnum, Some(20));
    assert_eq!(entry2.col, Some(15));
    assert_eq!(entry2.message, Some("warning: unused variable".to_owned()));
}

#[test]
fn test_parse_error_format_with_ignore_pattern_and_file_stack() {
    let error_format = vec![
        "%+P[%f]".to_owned(),
        "(%l\\,%c)%*[\\ ]%t%*[^:]:\\ %m".to_owned(),
        "%-Q".to_owned(),
        "%-G%.%#".to_owned(), // Ignore any line that doesn't match the above patterns
    ];
    let lines = split_lines(
        "\
[a1.tt]
(1,17)  error: ';' missing
This line should be ignored
(21,2)  warning: variable 'z' not defined
Another ignored line
[a2.tt]
(2,2)   warning: variable 'x' not defined",
    );
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 3);

    let entry1 = &entries[0];
    assert_eq!(entry1.filename, Some("a1.tt".to_owned()));
    assert_eq!(entry1.lnum, Some(1));
    assert_eq!(entry1.col, Some(17));
    assert_eq!(entry1.error_type, Some("e".to_owned()));
    assert_eq!(entry1.message, Some("';' missing".to_owned()));

    let entry2 = &entries[1];
    assert_eq!(entry2.filename, Some("a1.tt".to_owned()));
    assert_eq!(entry2.lnum, Some(21));
    assert_eq!(entry2.col, Some(2));
    assert_eq!(entry2.error_type, Some("w".to_owned()));
    assert_eq!(entry2.message, Some("variable 'z' not defined".to_owned()));

    let entry3 = &entries[2];
    assert_eq!(entry3.filename, Some("a2.tt".to_owned()));
    assert_eq!(entry3.lnum, Some(2));
    assert_eq!(entry3.col, Some(2));
    assert_eq!(entry3.error_type, Some("w".to_owned()));
    assert_eq!(entry3.message, Some("variable 'x' not defined".to_owned()));
}

// test for %+ cases
#[test]
fn test_parse_error_format_with_plus_pattern() {
    let error_format = vec![
        r"%+EError\ %n".to_owned(),
        r"%+Cline\ %l,\ %m".to_owned(),
        r"%Ccolumn\ %c".to_owned(),
        r"%Z%m".to_owned(),
    ];
    let lines = vec![
        "Error 275".to_owned(),
        "line 42, helloworld".to_owned(),
        "column 3".to_owned(),
        "This is error message".to_owned(),
    ];
    let entries = parse_error_format(error_format, lines).unwrap();

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.error_number, Some(275));
    assert_eq!(entry.lnum, Some(42));
    assert_eq!(entry.col, Some(3));
    assert_eq!(
        entry.message,
        Some("Error 275\nline 42, helloworld\nThis is error message".to_owned())
    );
}

#[test]
fn test_parse_rust_errorformat_patterns() {
    let error_format = r#"
%-G
%-Gerror:\ aborting\ %.%#
%-Gerror:\ Could\ not\ compile\ %.%#
%Eerror:\ %m
%Eerror[E%n]:\ %m
%Wwarning:\ %m
%Inote:\ %m
%C\ %#-->\ %f:%l:%c
%E\ \ left:%m
%C\ right:%m\ %f:%l:%c
%Z
"#;
    let error_format = split_lines(error_format);

    // Test data that matches the provided error message
    let error_text = r#"\
error[E0599]: no method named `test_method` found for struct `Vec<TestStruct>` in the current scope
   --> src/test_file.rs:42:15
    |
42  |     let result = vec.test_method();
    |                 ^^^^^^^^^^^^^^^^
    |
help: there is a method `test` with a similar name
    |
42  |     let result = vec.test();
    |                 ^^^
"#;
    let lines = split_lines(error_text);

    let entries = parse_error_format(error_format, lines).unwrap();

    // Should capture one combined entry with both error message and file location
    assert_eq!(entries.len(), 1);

    // The entry should combine the error message and file location
    let entry = &entries[0];
    assert_eq!(entry.error_type, Some("E".to_owned()));
    assert_eq!(entry.error_number, Some(599));
    assert_eq!(
        entry.message,
        Some(
            "no method named `test_method` found for struct `Vec<TestStruct>` in the current scope"
                .to_owned()
        )
    );
    assert_eq!(entry.filename, Some("src/test_file.rs".to_owned()));
    assert_eq!(entry.lnum, Some(42));
    assert_eq!(entry.col, Some(15));
}

#[test]
fn test_parse_go_errorformat_patterns() {
    // https://github.com/fatih/vim-go/blob/06ac99359b0b1a7de1e213447d92fd0a46cb4cd0/compiler/go.vim#L34-L40C26
    let error_format = r#"\
%-G#\ %.%#
%-G%.%#panic:\ %m
%Ecan\'t\ load\ package:\ %m
%A\(\[\^:]\+:\ \)\?%f:%l:%c:\ %m
%A\(\[\^:]\+:\ \)\?%f:%l:\ %m
%C%*\s%m
%-G%.%#
"#;
    let error_format = split_lines(error_format);

    let error_text = r#"
fbcode/neteng/vending_machine/bin/spy/semlock.go:158:93: not enough arguments in call to vmtask.CreateTaskWithClient
    have (*"facebook/task".Task, string, "facebook/task".TaskServiceClient)
    want ("context".Context, *"facebook/task".Task, string, "facebook/task".TaskServiceClient)
"#;
    let lines = split_lines(error_text);

    let entries = parse_error_format(error_format.clone(), lines).unwrap();

    assert_eq!(entries.len(), 1);
    let entry = &entries[0];

    assert_eq!(
        entry.filename,
        Some("fbcode/neteng/vending_machine/bin/spy/semlock.go".to_owned())
    );
    assert_eq!(entry.lnum, Some(158));
    assert_eq!(entry.end_lnum, None);
    assert_eq!(entry.col, Some(93));
    assert_eq!(entry.end_col, None);
    assert_eq!(
        entry.message,
        Some("not enough arguments in call to vmtask.CreateTaskWithClient\nhave (*\"facebook/task\".Task, string, \"facebook/task\".TaskServiceClient)\nwant (\"context\".Context, *\"facebook/task\".Task, string, \"facebook/task\".TaskServiceClient)".to_owned())
    );
    assert_eq!(entry.error_type, Some("A".to_owned()));
    assert_eq!(entry.error_number, None);

    let error_text2 = r#"
fbcode/security/duo-2fac/pam_duo.go:26:43: undefined: pam.Style
fbcode/security/duo-2fac/pam_duo.go:28:11: undefined: pam.PromptEchoOn
"#;
    let lines2 = split_lines(error_text2);

    let entries2 = parse_error_format(error_format, lines2).unwrap();

    assert_eq!(entries2.len(), 2);

    // First error
    let entry2_1 = &entries2[0];
    assert_eq!(
        entry2_1.filename,
        Some("fbcode/security/duo-2fac/pam_duo.go".to_owned())
    );
    assert_eq!(entry2_1.lnum, Some(26));
    assert_eq!(entry2_1.end_lnum, None);
    assert_eq!(entry2_1.col, Some(43));
    assert_eq!(entry2_1.end_col, None);
    assert_eq!(entry2_1.message, Some("undefined: pam.Style".to_owned()));
    assert_eq!(entry2_1.error_type, Some("A".to_owned()));
    assert_eq!(entry2_1.error_number, None);

    // Second error
    let entry2_2 = &entries2[1];
    assert_eq!(
        entry2_2.filename,
        Some("fbcode/security/duo-2fac/pam_duo.go".to_owned())
    );
    assert_eq!(entry2_2.lnum, Some(28));
    assert_eq!(entry2_2.end_lnum, None);
    assert_eq!(entry2_2.col, Some(11));
    assert_eq!(entry2_2.end_col, None);
    assert_eq!(
        entry2_2.message,
        Some("undefined: pam.PromptEchoOn".to_owned())
    );
    assert_eq!(entry2_2.error_type, Some("A".to_owned()));
    assert_eq!(entry2_2.error_number, None);
}

fn first_lines(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| e.original_err_lines.first().cloned().unwrap_or_default())
        .collect()
}

/// The rules of `prelude/go/go_error_handler.bzl`, after Starlark unescaping.
fn go_error_format() -> Vec<String> {
    vec![
        r"%-G#\ %.%#".to_owned(),
        r"%-G%.%#panic:\ %m".to_owned(),
        r"%Ecan't\ load\ package:\ %m".to_owned(),
        r"%A\(\[\^:]\+:\ \)\?%f:%l:%c:\ %m".to_owned(),
        r"%A\(\[\^:]\+:\ \)\?%f:%l:\ %m".to_owned(),
        r"%C%*\s%m".to_owned(),
        r"%-G%.%#".to_owned(),
    ]
}

/// `go build` prints a `# <package>` header before each package's errors. The catch-all `%-G`
/// rule drops the header on its own; the error that is still open survives, as does any error
/// followed by an unrelated line.
#[test]
fn test_ignored_line_between_two_errors_keeps_both_errors() {
    let lines = split_lines(
        "# example.com/a\na/a.go:3:2: undefined: x\n# example.com/b\nb/b.go:4:2: undefined: y\n",
    );
    let entries = parse_error_format(go_error_format(), lines).unwrap();
    assert_eq!(
        first_lines(&entries),
        vec!["a/a.go:3:2: undefined: x", "b/b.go:4:2: undefined: y"]
    );

    let lines = split_lines("a.go:1:2: first\nsome unrelated line\na.go:5:6: second\n");
    let entries = parse_error_format(go_error_format(), lines).unwrap();
    assert_eq!(
        first_lines(&entries),
        vec!["a.go:1:2: first", "a.go:5:6: second"]
    );
}

/// An ignored multi-line message (`%-E`) stays ignored when the input ends, or a line matches
/// nothing, before the message is closed, and an error it follows is kept.
#[test]
fn test_ignored_multi_line_message_stays_ignored_when_not_closed() {
    let efm = vec!["%-EError %n".to_owned(), "%CLine %l".to_owned()];
    let entries = parse_error_format(efm.clone(), split_lines("Error 100\nLine 10\n")).unwrap();
    assert!(entries.is_empty(), "{entries:?}");
    let entries = parse_error_format(efm, split_lines("Error 100\nunrelated\n")).unwrap();
    assert!(entries.is_empty(), "{entries:?}");

    let efm = vec!["%Eerror: %m".to_owned(), "%-Enote: %m".to_owned()];
    let entries = parse_error_format(efm, split_lines("error: boom\nnote: ignore me\n")).unwrap();
    assert_eq!(first_lines(&entries), vec!["error: boom"]);
}

/// A rule without a prefix is tried while a multi-line message is open; it closes that message
/// and stands on its own.
#[test]
fn test_plain_rule_closes_an_open_multi_line_message() {
    let efm = vec![
        "%Eerror: %m".to_owned(),
        "%C  --> %f:%l".to_owned(),
        "%f:%l: warning: %m".to_owned(),
    ];
    let entries = parse_error_format(efm, split_lines("error: boom\na.c:3: warning: w\n")).unwrap();
    assert_eq!(
        first_lines(&entries),
        vec!["error: boom", "a.c:3: warning: w"]
    );
}
