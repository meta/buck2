/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Tests for `cmd_args` options: parent, absolute_prefix, absolute_suffix,
//! relative_to, format, delimiter, prepend, quote, replace_regex — including
//! nesting behavior.

use buck2_interpreter_for_build::interpreter::testing::expect_error;
use indoc::indoc;

use crate::interpreter::rule_defs::cmd_args::testing::tester;

// ===================================================================
// Declarative test infrastructure
// ===================================================================

/// An option applied to one `cmd_args` layer.
enum CmdArgsOpt {
    Parent(u32),
    AbsolutePrefix(&'static str),
    AbsoluteSuffix(&'static str),
    /// `(path, parent)` for the `relative_to` artifact (always uses empty package).
    RelativeTo(&'static str, u32),
    Format(&'static str),
    Delimiter(&'static str),
    Prepend(&'static str),
    Quote,
}

/// Generates the `cmd_args` wrapping layers shared by all code-gen helpers.
fn gen_layers(code: &mut String, layers: &[&[CmdArgsOpt]]) {
    use std::fmt::Write;

    for (i, opts) in layers.iter().enumerate() {
        let mut opt_parts = Vec::new();
        for opt in opts.iter() {
            match opt {
                CmdArgsOpt::Parent(n) => opt_parts.push(format!("parent={n}")),
                CmdArgsOpt::AbsolutePrefix(p) => opt_parts.push(format!("absolute_prefix=\"{p}\"")),
                CmdArgsOpt::AbsoluteSuffix(s) => opt_parts.push(format!("absolute_suffix=\"{s}\"")),
                CmdArgsOpt::RelativeTo(rpath, rparent) => {
                    if *rparent == 0 {
                        opt_parts.push(format!("relative_to=source_artifact(\"\", \"{rpath}\")"));
                    } else {
                        opt_parts.push(format!(
                            "relative_to=(source_artifact(\"\", \"{rpath}\"), {rparent})"
                        ));
                    }
                }
                CmdArgsOpt::Format(f) => opt_parts.push(format!("format=\"{f}\"")),
                CmdArgsOpt::Delimiter(d) => opt_parts.push(format!("delimiter=\"{d}\"")),
                CmdArgsOpt::Prepend(p) => opt_parts.push(format!("prepend=\"{p}\"")),
                CmdArgsOpt::Quote => opt_parts.push("quote=\"shell\"".to_owned()),
            }
        }
        if opt_parts.is_empty() {
            writeln!(code, "    x{i} = cmd_args(x)").unwrap();
        } else {
            let opts_str = opt_parts.join(", ");
            writeln!(code, "    x{i} = cmd_args(x, {opts_str})").unwrap();
        }
        writeln!(code, "    x = x{i}").unwrap();
    }
}

fn starlark_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Creates a source artifact, wraps it in `cmd_args` layers, and asserts the
/// result equals `expected`.
fn check_cmd_args(path: &str, layers: &[&[CmdArgsOpt]], expected: &[&str]) {
    use std::fmt::Write;

    let mut code = String::new();
    writeln!(code, "def test():").unwrap();
    writeln!(code, "    x = cmd_args(source_artifact(\"\", \"{path}\"))").unwrap();
    gen_layers(&mut code, layers);

    let expected_strs: Vec<String> = expected.iter().map(|s| starlark_quote(s)).collect();
    writeln!(
        code,
        "    assert_eq([{}], get_args(x))",
        expected_strs.join(", ")
    )
    .unwrap();

    let mut t = tester().unwrap();
    t.run_starlark_bzl_test(&code).unwrap();
}

/// Like [`check_cmd_args`], but starts from string arguments instead of an
/// artifact.
fn check_cmd_args_str(args: &[&str], layers: &[&[CmdArgsOpt]], expected: &[&str]) {
    use std::fmt::Write;

    let mut code = String::new();
    writeln!(code, "def test():").unwrap();
    let args_str: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
    writeln!(code, "    x = cmd_args({})", args_str.join(", ")).unwrap();
    gen_layers(&mut code, layers);

    let expected_strs: Vec<String> = expected.iter().map(|s| starlark_quote(s)).collect();
    writeln!(
        code,
        "    assert_eq(get_args(x), [{}])",
        expected_strs.join(", ")
    )
    .unwrap();

    let mut t = tester().unwrap();
    t.run_starlark_bzl_test(&code).unwrap();
}

/// Like [`check_cmd_args`], but asserts that evaluation fails with the given
/// error message.
fn check_cmd_args_error(path: &str, layers: &[&[CmdArgsOpt]], error_msg: &str) {
    use std::fmt::Write;

    let mut code = String::new();
    writeln!(code, "def test():").unwrap();
    writeln!(code, "    x = cmd_args(source_artifact(\"\", \"{path}\"))").unwrap();
    gen_layers(&mut code, layers);
    writeln!(code, "    get_args(x)").unwrap();

    let mut t = tester().unwrap();
    expect_error(t.run_starlark_bzl_test(&code), &code, error_msg);
}

// ===================================================================
// parent
// ===================================================================

#[test]
fn test_opts_parent_in_parent() {
    use CmdArgsOpt::*;
    check_cmd_args("a/b/c/d.h", &[&[Parent(1)], &[Parent(1)]], &["a/b"]);
}

#[test]
fn test_opts_parent2_in_parent1() {
    use CmdArgsOpt::*;
    check_cmd_args("a/b/c/d.h", &[&[Parent(3)]], &["./a"]);
    check_cmd_args("a/b/c/d.h", &[&[Parent(2)], &[Parent(1)]], &["./a"]);
}

#[test]
fn test_opts_parent_in_noop() {
    use CmdArgsOpt::*;
    check_cmd_args("a/b/c/d.h", &[&[Parent(1)], &[Parent(0)]], &["a/b/c"]);
}

#[test]
fn test_opts_parent_after_suffix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "foo/bar/baz/qux.h",
        &[&[AbsoluteSuffix("!"), Parent(1)]],
        &["foo/bar/baz!"],
    );
}

#[test]
fn test_opts_parent_before_suffix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "foo/bar/baz/qux.h",
        &[&[Parent(2), AbsoluteSuffix("!")]],
        &["foo/bar!"],
    );
}

#[test]
fn test_opts_parent_before_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[Parent(1)], &[AbsolutePrefix("P/")]],
        &["P/a/b"],
    );
}

#[test]
fn test_opts_parent_after_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[AbsolutePrefix("P/")], &[Parent(1)]],
        &["P/a/b"],
    );
}

#[test]
fn test_opts_parent_error_too_many() {
    use CmdArgsOpt::*;
    check_cmd_args_error("foo/qux.h", &[&[Parent(3)]], "does not have 3 parent");
}

// ===================================================================
// relative_to
// ===================================================================

#[test]
fn test_opts_relative_to_with_prefix_suffix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "foo/bar/baz/qux.h",
        &[&[
            AbsolutePrefix("$ABSOLUTE/"),
            AbsoluteSuffix("!"),
            RelativeTo("foo/bar/foo", 0),
        ]],
        &["$ABSOLUTE/../baz/qux.h!"],
    );
}

#[test]
fn test_opts_relative_to_with_parent() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "foo/bar/baz/qux.h",
        &[&[RelativeTo("foo/bar/baz", 1)]],
        &["baz/qux.h"],
    );
}

#[test]
fn test_opts_relative_to_propagated_up() {
    use CmdArgsOpt::*;
    // Wrapping in a plain cmd_args preserves the relative_to behavior.
    check_cmd_args(
        "foo/bar.h",
        &[&[RelativeTo("foo/baz.c", 1)], &[]],
        &["./bar.h"],
    );
}

// ===================================================================
// Options nesting (multi-layer, artifact path options)
// ===================================================================

#[test]
fn test_opts_prefix_in_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[&[AbsolutePrefix("B/")], &[AbsolutePrefix("A/")]],
        &["B/a/b.h"],
    );
}

#[test]
fn test_opts_suffix_in_suffix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[&[AbsoluteSuffix("!")], &[AbsoluteSuffix("?")]],
        &["a/b.h!"],
    );
}

#[test]
fn test_opts_prefix_suffix_in_prefix_suffix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[
            &[AbsolutePrefix("B/"), AbsoluteSuffix("!")],
            &[AbsolutePrefix("A/"), AbsoluteSuffix("?")],
        ],
        &["B/a/b.h!"],
    );
}

#[test]
fn test_opts_relative_to_in_relative_to() {
    use CmdArgsOpt::*;
    // Inner relative_to wins; outer is ignored.
    check_cmd_args(
        "a/b.h",
        &[&[RelativeTo("a/c.h", 0)], &[RelativeTo("x/y.h", 0)]],
        &["../b.h"],
    );
}

#[test]
fn test_opts_relative_to_parent0_vs_parent1() {
    use CmdArgsOpt::*;
    // relative_to with parent=0 is relative to the *file*, not its directory.
    // So a/b.h relative to a/c.h means: go up from c.h, then to b.h => ../b.h
    check_cmd_args("a/b.h", &[&[RelativeTo("a/c.h", 0)]], &["../b.h"]);
    // relative_to with parent=1 is relative to the *directory*.
    // So a/b.h relative to a/ => b.h
    check_cmd_args("a/b.h", &[&[RelativeTo("a/c.h", 1)]], &["./b.h"]);
}

#[test]
fn test_opts_parent_in_relative_to() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c/d.h",
        &[&[Parent(1)], &[RelativeTo("a/ref.txt", 1)]],
        &["b/c"],
    );
}

#[test]
fn test_opts_relative_to_in_parent() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c/d.h",
        &[&[RelativeTo("a/ref.txt", 1)], &[Parent(1)]],
        &["b/c"],
    );
}

#[test]
fn test_opts_relative_to_in_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[RelativeTo("a/ref.txt", 1)], &[AbsolutePrefix("P/")]],
        &["P/b/c.h"],
    );
}

#[test]
fn test_opts_prefix_in_relative_to() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[AbsolutePrefix("P/")], &[RelativeTo("a/ref.txt", 1)]],
        &["P/b/c.h"],
    );
}

#[test]
fn test_opts_suffix_in_relative_to() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[AbsoluteSuffix("!")], &[RelativeTo("a/ref.txt", 1)]],
        &["b/c.h!"],
    );
}

// ===================================================================
// Nesting path options outside string options
// ===================================================================

#[test]
fn test_opts_format_in_parent() {
    use CmdArgsOpt::*;
    check_cmd_args("a/b/c.h", &[&[Format("-I{}")], &[Parent(1)]], &["-Ia/b"]);
}

#[test]
fn test_opts_format_in_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[&[Format("-I{}")], &[AbsolutePrefix("P/")]],
        &["-IP/a/b.h"],
    );
}

#[test]
fn test_opts_format_in_suffix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[&[Format("-I{}")], &[AbsoluteSuffix("!")]],
        &["-Ia/b.h!"],
    );
}

#[test]
fn test_opts_format_in_relative_to() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[Format("-I{}")], &[RelativeTo("a/ref.txt", 1)]],
        &["-Ib/c.h"],
    );
}

#[test]
fn test_opts_prepend_in_parent() {
    use CmdArgsOpt::*;
    check_cmd_args("a/b/c.h", &[&[Prepend("-I")], &[Parent(1)]], &["-I", "a/b"]);
}

#[test]
fn test_opts_prepend_in_relative_to() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[&[Prepend("-I")], &[RelativeTo("a/ref.txt", 1)]],
        &["-I", "b/c.h"],
    );
}

#[test]
fn test_opts_quote_in_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[&[Quote], &[AbsolutePrefix("$ROOT/")]],
        &[r#""\$ROOT/a/b.h""#],
    );
}

#[test]
fn test_opts_quote_in_relative_to() {
    use CmdArgsOpt::*;
    // b/c.h has no shell-special characters, so shlex_quote is a no-op.
    check_cmd_args(
        "a/b/c.h",
        &[&[Quote], &[RelativeTo("a/ref.txt", 1)]],
        &["b/c.h"],
    );
}

#[test]
fn test_opts_delimiter_in_parent() {
    use CmdArgsOpt::*;
    check_cmd_args("a/b/c.h", &[&[Delimiter(",")], &[Parent(1)]], &["a/b"]);
}

#[test]
fn test_opts_format_prepend_in_relative_to() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b/c.h",
        &[
            &[Format("-I{}"), Prepend("@")],
            &[RelativeTo("a/ref.txt", 1)],
        ],
        &["@", "-Ib/c.h"],
    );
}

#[test]
fn test_opts_format_quote_in_prefix() {
    use CmdArgsOpt::*;
    check_cmd_args(
        "a/b.h",
        &[&[Format("-I{}"), Quote], &[AbsolutePrefix("$ROOT/")]],
        &[r#""-I\$ROOT/a/b.h""#],
    );
}

// ===================================================================
// format, delimiter, prepend, quote (single-layer and nesting)
// ===================================================================

#[test]
fn test_opts_concat() {
    use CmdArgsOpt::*;

    // Single-layer combinations
    check_cmd_args_str(&["foo", "bar"], &[&[Delimiter("-")]], &["foo-bar"]);
    check_cmd_args_str(
        &["foo", "bar"],
        &[&[Delimiter("-"), Format("({})")]],
        &["(foo)-(bar)"],
    );
    check_cmd_args_str(
        &["foo", "bar"],
        &[&[Delimiter("-"), Prepend("@"), Format("({})")]],
        &["@(foo)-@(bar)"],
    );
    check_cmd_args_str(
        &["foo", "bar"],
        &[&[Delimiter("-"), Prepend("@"), Format("({})"), Quote]],
        &[r#"@"(foo)"-@"(bar)""#],
    );

    // Nesting: order of application.
    // Note that prepend and delimiter together are not separable.
    check_cmd_args_str(
        &["foo", "bar"],
        &[&[Format("({})")], &[Quote], &[Prepend("@"), Delimiter("-")]],
        &[r#"@"(foo)"-@"(bar)""#],
    );
    check_cmd_args_str(
        &["foo", "bar"],
        &[
            &[Format("({})")],
            &[Quote],
            &[Prepend("@")],
            &[Delimiter("-")],
        ],
        &[r#"@-"(foo)"-@-"(bar)""#],
    );
    check_cmd_args_str(
        &["foo", "bar"],
        &[
            &[Format("({})")],
            &[Quote],
            &[Delimiter("-")],
            &[Prepend("@")],
        ],
        &["@", r#""(foo)"-"(bar)""#],
    );
}

#[test]
fn test_format() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args1 = cmd_args(delimiter="", format="format-{}-{}-string")
            args2 = cmd_args(format="format-{}-{}-string")
            args3 = cmd_args()

            args1.add("foo")
            args1.add(",bar")
            args2.add("foo")
            args2.add("bar")
            args3.add(cmd_args(["foo", "bar"], format="format-{}-{}-string", delimiter = ""))
            args3.add(cmd_args(["foo", "bar"], delimiter=",", format="format-{}-{}-string"))
            args3.add(cmd_args("foo", format="format-{}-{}-string1"))
            args3.add(cmd_args("bar", format="format-{}-{}-string2"))

            assert_eq(["format-foo-foo-stringformat-,bar-,bar-string"], get_args(args1))
            assert_eq(
                [
                    "format-foo-foo-string",
                    "format-bar-bar-string",
                ],
                get_args(args2),
            )
            assert_eq(
                [
                    "format-foo-foo-stringformat-bar-bar-string",
                    "format-foo-foo-string,format-bar-bar-string",
                    "format-foo-foo-string1",
                    "format-bar-bar-string2",
                ],
                get_args(args3),
            )
        "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

#[test]
fn test_joined_with_empty_args() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args()
            args.add(cmd_args(["", "foo"], delimiter=",", format="format-{}-string"))
            args.add(cmd_args(["", "", "foo"], delimiter=","))
            assert_eq(
                [
                    "format--string,format-foo-string",
                    ",,foo",
                ],
                get_args(args),
            )
        "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

#[test]
fn test_quote_style_shell() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args()
            args.add(cmd_args("$HELLO", quote = "shell"))
            assert_eq(get_args(args), ["\"\\$HELLO\""])

            args = cmd_args()
            args.add(cmd_args(source_artifact("foo", "bar$qux.h"), quote = "shell"))
            assert_eq(get_args(args), ["\"foo/bar\\$qux.h\""])
            "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

#[test]
fn test_prepend() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args()
            args.add([["foo", "bar"], "baz"])
            assert_eq(["foo", "bar", "baz"], get_args(args))
            "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

#[test]
fn test_list_list() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args()
            args.add(cmd_args(["foo", "bar"], prepend = "-X"))
            assert_eq(["-X", "foo", "-X", "bar"], get_args(args))
            "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

// ===================================================================
// replace_regex
// ===================================================================

#[test]
fn test_replace_regex() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args(
                "$OUT",
                "$OUTPUT",
                "$SRCS",
                format="$OUT: {}",
                replace_regex=[("\\$OUT\\b", "%OUT%"), ("\\$SRCS\\b", "%SRCS%")],
            )
            assert_eq(["$OUT: %OUT%", "$OUT: $OUTPUT", "$OUT: %SRCS%"], get_args(args))

            args = cmd_args(
                "\\n\n",
                replace_regex=[("\\\\n", "\\\n"), ("\\n", "\\n")],
            )
            assert_eq(["\\\\n\\n"], get_args(args))
        "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

#[test]
fn test_replace_regex_old() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args(
                "$OUT",
                "$OUTPUT",
                "$SRCS",
                format="$OUT: {}",
                replace_regex=[("\\$OUT\\b", "%OUT%"), ("\\$SRCS\\b", "%SRCS%")],
            )
            assert_eq(["$OUT: %OUT%", "$OUT: $OUTPUT", "$OUT: %SRCS%"], get_args(args))

            args = cmd_args(
                "\\n\n",
                replace_regex=[("\\\\n", "\\\n"), ("\\n", "\\n")],
            )
            assert_eq(["\\\\n\\n"], get_args(args))
        "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

#[test]
fn test_replace_regex_regex() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args("$OUT", "$OUTPUT", "$SRCS", format="$OUT: {}",
                replace_regex=[(regex("\\$OUT\\b"), "%OUT%"), (regex("\\$SRCS\\b"), "%SRCS%")])
            assert_eq(["$OUT: %OUT%", "$OUT: $OUTPUT", "$OUT: %SRCS%"], get_args(args))

            args = cmd_args("\\n\n",
                replace_regex = [(regex("\\\\n"), "\\\n"), ("\\n", "\\n")])
            assert_eq(["\\\\n\\n"], get_args(args))
        "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}
