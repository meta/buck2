/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::fmt::Write;

use crate::golden_test_template::golden_test_template;
use crate::slice_vec_ext::SliceExt;
use crate::syntax::AstModule;
use crate::syntax::Dialect;
use crate::syntax::DialectTypes;
use crate::syntax::ast::Expr;
use crate::syntax::ast::Stmt;

fn assert_parse_fails(program: &str, dialect: &Dialect) {
    if let Ok(ast) = AstModule::parse("assert.bzl", program.to_owned(), dialect) {
        let ast_str = ast.statement.to_string();
        panic!("Expected parse failure, but parser accepted:\n{program}\nAST: {ast_str}");
    }
}

fn parse_fails_with_dialect(name: &str, dialect: &Dialect, programs: &[&str]) {
    let mut out = String::new();

    for (i, program) in programs.iter().enumerate() {
        if i != 0 {
            writeln!(out).unwrap();
        }

        let program = program.trim();
        let err = match AstModule::parse(name, program.to_owned(), dialect) {
            Ok(ast) => {
                let ast_str = ast.statement.to_string();
                panic!("Expected parse failure for:\n{program}\nGot AST: {ast_str}");
            }
            Err(e) => e,
        };

        writeln!(out, "Program:").unwrap();
        writeln!(out, "{program}").unwrap();
        writeln!(out).unwrap();
        writeln!(out, "Error:").unwrap();
        writeln!(out, "{err}").unwrap();
    }

    golden_test_template(&format!("src/syntax/grammar_tests/{name}.golden"), &out);
}

fn parse_fail_with_dialect(name: &str, dialect: &Dialect, program: &str) {
    parse_fails_with_dialect(name, dialect, &[program]);
}

fn parse_fail(name: &str, program: &str) {
    parse_fail_with_dialect(name, &Dialect::AllOptionsInternal, program);
}

fn parse_fails(name: &str, programs: &[&str]) {
    parse_fails_with_dialect(name, &Dialect::AllOptionsInternal, programs);
}

fn assert_parse_fails_program(program: &str) {
    assert_parse_fails(program, &Dialect::AllOptionsInternal);
}

fn assert_parse_fails_programs(programs: &[&str]) {
    for program in programs {
        assert_parse_fails_program(program);
    }
}

fn parse_err(program: &str) -> crate::Error {
    AstModule::parse(
        "assert.bzl",
        program.to_owned(),
        &Dialect::AllOptionsInternal,
    )
    .unwrap_err()
}

fn assert_parse_error_span_source(program: &str, expected_source: &str) {
    let err = parse_err(program);
    let span = err
        .span()
        .unwrap_or_else(|| panic!("Expected parse error with span for:\n{program}\nError: {err}"));
    assert_eq!(
        span.source_span(),
        expected_source,
        "Expected parse error to point at `{expected_source}` for:\n{program}\nError: {err}",
    );
}

fn assert_parse_error_contains(program: &str, expected: &str) {
    let err = parse_err(program);
    let err_text = err.to_string();
    assert!(
        err_text.contains(expected),
        "Expected parse error containing `{expected}` for:\n{program}\nActual error:\n{err_text}",
    );
}

fn assert_assignment_rhs_is_slice(
    program: &str,
    expected_base: &str,
    expected_start: Option<&str>,
    expected_stop: Option<&str>,
    expected_step: Option<&str>,
) {
    let ast = parse_ast(program);
    let rhs = match &ast.statement.node {
        Stmt::Assign(assign) => &assign.rhs.node,
        _ => panic!("Expected assignment for:\n{program}"),
    };
    let (base, start, stop, step) = match rhs {
        Expr::Slice(base, start, stop, step) => (base, start, stop, step),
        _ => panic!("Expected slice RHS for:\n{program}"),
    };

    assert_eq!(base.node.to_string(), expected_base);
    assert_eq!(
        start.as_ref().map(|expr| expr.node.to_string()).as_deref(),
        expected_start,
    );
    assert_eq!(
        stop.as_ref().map(|expr| expr.node.to_string()).as_deref(),
        expected_stop,
    );
    assert_eq!(
        step.as_ref().map(|expr| expr.node.to_string()).as_deref(),
        expected_step,
    );
}

#[test]
fn test_empty() {
    assert_eq!(parse("\n"), "");
    assert_eq!(parse(""), "");
    assert_eq!(parse("    "), "");
    assert_eq!(parse("    \n"), "");
    // Empty trailing bits
    assert_eq!(parse("x = 1\n   "), "x = 1\n");
    assert_eq!(parse("x = 1\n   \n"), "x = 1\n");
}

#[test]
fn test_assign_op() {
    assert_eq!(parse("x = 1\n"), "x = 1\n");
    assert_eq!(parse("x += 1\n"), "x += 1\n");
    assert_eq!(parse("x -= 1\n"), "x -= 1\n");
    assert_eq!(parse("x *= 1\n"), "x *= 1\n");
    assert_eq!(parse("x /= 1\n"), "x /= 1\n");
    assert_eq!(parse("x //= 1\n"), "x //= 1\n");
    assert_eq!(parse("x %= 1\n"), "x %= 1\n");
    assert_eq!(parse("x &= 1\n"), "x &= 1\n");
    assert_eq!(parse("x |= 1\n"), "x |= 1\n");
    assert_eq!(parse("x ^= 1\n"), "x ^= 1\n");
    assert_eq!(parse("x <<= 1\n"), "x <<= 1\n");
    assert_eq!(parse("x >>= 1\n"), "x >>= 1\n");
}

#[test]
fn test_top_level_comment() {
    assert_eq!(parse("# Test"), "");
}

#[test]
fn test_top_level_load() {
    let want = "load(\"//top/level/load.bzl\", \"top-level\")\n";
    assert_eq!(
        parse("\nload(\"//top/level/load.bzl\", \"top-level\")\n"),
        want
    );
    assert_eq!(
        parse("\nload(\"//top/level/load.bzl\", \"top-level\")"),
        want
    );
    assert_eq!(
        parse("\nload(\n  \"//top/level/load.bzl\",\n  \"top-level\",\n)\n"),
        want
    );
    assert_eq!(parse(want), want);
}

#[test]
fn test_top_level_assignation() {
    assert_eq!(
        parse("\n_ASSIGNATION = 'top-level'\n"),
        "_ASSIGNATION = \"top-level\"\n"
    );
}

#[test]
fn test_top_level_docstring() {
    assert_eq!(
        parse("\n\"\"\"Top-level docstring\"\"\"\n"),
        "\"Top-level docstring\"\n"
    );
}

#[test]
fn test_top_level_def() {
    assert_eq!(parse("def toto():\n  pass\n"), "def toto():\n  pass\n");
    parse_fail_with_dialect(
        "top_level_def",
        &Dialect {
            enable_def: false,
            ..Dialect::AllOptionsInternal
        },
        "def toto():\n  pass",
    );
    // no new line at end of file
    assert_eq!(parse("def toto():\n  pass"), "def toto():\n  pass\n");
    assert_eq!(
        parse("def toto():\n  pass\ndef titi(): return 1"),
        "def toto():\n  pass\ndef titi():\n  return 1\n"
    );
    assert_eq!(
        parse("def toto():\n  pass\n\ndef titi(): return 1"),
        "def toto():\n  pass\ndef titi():\n  return 1\n"
    );
    assert_eq!(parse("def t():\n\n  pass"), "def t():\n  pass\n");
}

#[test]
fn test_top_level_statements() {
    let no_top_leve_stmt = Dialect {
        enable_top_level_stmt: false,
        ..Dialect::AllOptionsInternal
    };
    parse_fails_with_dialect(
        "top_level_statements",
        &no_top_leve_stmt,
        &[
            "x = 1\nif x == 1:\n  x = 2\nx = 3",
            "x = 1\nfor x in []:\n   pass\n",
        ],
    );
    let mut dialect = Dialect::Standard;
    dialect.enable_top_level_stmt = false;
    assert_eq!(parse_with_dialect("pass", &dialect), "pass\n");

    assert_eq!(parse("if x == 1:\n  x = 2"), "if (x == 1):\n  x = 2\n");
    assert_eq!(parse("for x in []:\n   pass\n"), "for x in []:\n  pass\n");
    assert_eq!(parse("pass"), "pass\n");
}

#[test]
fn test_top_level_def_with_docstring() {
    assert_eq!(
        parse(
            "\"\"\"Top-level docstring\"\"\"

def toto():
  pass
"
        ),
        "\"Top-level docstring\"\ndef toto():\n  pass\n"
    );
}

#[test]
fn test_ifelse() {
    assert_eq!(
        parse("def d():\n  if True:\n    a\n  else:\n    b"),
        "def d():\n  if True:\n    a\n  else:\n    b\n"
    );
}

#[test]
fn test_kwargs_passing() {
    assert_eq!(
        parse("f(x, *a, **b); f(x, *a, **{a:b}); f(x, *[a], **b)"),
        "f(x, *a, **b)\nf(x, *a, **{a: b})\nf(x, *[a], **b)\n"
    );
}

#[test]
fn test_unary_op() {
    assert_eq!(parse("a = -1"), "a = -1\n");
    assert_eq!(parse("a = +1"), "a = +1\n");
    assert_eq!(parse("a = -a"), "a = -a\n");
    assert_eq!(parse("a = +a"), "a = +a\n");
}

#[test]
fn test_tuples() {
    assert_eq!(parse("a = (-1)"), "a = -1\n"); // Not a tuple
    assert_eq!(parse("a = (+1,)"), "a = (+1,)\n"); // But this is one
    assert_eq!(parse("a = ()"), "a = ()\n");
}

// `b"..."` must parse as a bytes literal expression. Driven by both parsers
// via the shared helpers, so RD and LALRPOP must agree on the AST.
#[test]
fn test_bytes_literal() {
    assert_eq!(parse(r#"x = b"hello""#), "x = b\"hello\"\n");
    assert_eq!(parse(r#"x = b'world'"#), "x = b\"world\"\n");
    assert_eq!(parse(r#"x = b"""triple""""#), "x = b\"triple\"\n");
    assert_eq!(parse(r#"x = rb"\n""#), "x = b\"\\\\n\"\n");
    assert_eq!(parse(r#"print(b"hi")"#), "print(b\"hi\")\n");
    assert_eq!(parse(r#"x = (1, b"")"#), "x = (1, b\"\")\n");
    assert_eq!(parse(r#"x = (b"", 1)"#), "x = (b\"\", 1)\n");
    assert_eq!(parse(r#"x = (1, b"", 2)"#), "x = (1, b\"\", 2)\n");
}

#[test]
fn test_return() {
    assert_eq!(parse("def fn(): return 1"), "def fn():\n  return 1\n");
    assert_eq!(parse("def fn(): return a()"), "def fn():\n  return a()\n");
    assert_eq!(parse("def fn(): return"), "def fn():\n  return\n");
}

// Regression test for https://github.com/google/starlark-rust/issues/44.
#[test]
fn test_optional_whitespace() {
    assert_eq!(parse("6 or()"), "(6 or ())\n");
    assert_eq!(parse("6or()"), "(6 or ())\n");
}

// Regression test for https://github.com/google/starlark-rust/issues/56.
#[test]
fn test_optional_whitespace_after_0() {
    assert_eq!(parse("0in[1,2,3]"), "(0 in [1, 2, 3])\n");
}

#[test]
fn test_blank_line() {
    assert_eq!(
        parse(
            r#"
def foo():
  bar

  bar
"#
        ),
        "def foo():\n  bar\n  bar\n"
    )
}

#[test]
fn test_fncall_span() {
    let content = r#"def fn(a):
  fail(a)

fn(1)

fail(2)
"#;
    let ast = parse_ast(content);
    match &ast.statement.node {
        Stmt::Statements(xs) => {
            let lines = xs.map(|x| ast.codemap.resolve_span(x.span).begin.line);
            assert_eq!(lines, vec![0, 3, 5])
        }
        _ => panic!("Expected to parse as statements"),
    }
}

#[test]
fn test_comprehension() {
    assert_eq!(
        parse("[x for x in range(12) if x % 2 == 0 if x % 3 == 0]"),
        "[x for x in range(12) if ((x % 2) == 0) if ((x % 3) == 0)]\n"
    );
    assert_eq!(
        parse("[x + 7 for x in range(12) for y in range(10)]"),
        "[(x + 7) for x in range(12) for y in range(10)]\n"
    );
}

#[test]
fn test_lambda() {
    assert_eq!(parse("x = lambda y: y + 1"), "x = (lambda y: (y + 1))\n");
    parse_fail_with_dialect(
        "lambda",
        &Dialect {
            enable_lambda: false,
            ..Dialect::AllOptionsInternal
        },
        "x = lambda y: y + 1",
    );
    assert_eq!(parse("(lambda y: x == 1)(1)"), "(lambda y: (x == 1))(1)\n");
    assert_eq!(parse("(lambda x: x or 1)(1)"), "(lambda x: (x or 1))(1)\n");
    assert_eq!(
        parse("f = lambda x, y: x * y"),
        "f = (lambda x, y: (x * y))\n"
    );
    assert_eq!(parse("lambda x: True"), "(lambda x: True)\n");
    assert_eq!(parse("lambda: True"), "(lambda : True)\n");
    assert_eq!(
        parse("f(lambda x, y=1, *args, **kwargs: x + y + z)"),
        "f((lambda x, y = 1, *args, **kwargs: ((x + y) + z)))\n"
    );
    assert_eq!(
        parse("[x for x in [1, 2] if (lambda : 3 if True else 4)]"),
        "[x for x in [1, 2] if (lambda : (3 if True else 4))]\n"
    );
    // Note that Python3 and Go Starlark can both parse the line below,
    // but we can't, and the official Python3 grammar doesn't appear to be
    // able to either (we follow it in this position). Fortunately, its
    // a mostly meaningless program.
    // [x for x in [1, 2] if lambda : None]
}

#[test]
fn test_ellipsis() {
    parse_fails_with_dialect(
        "ellipsis",
        &Dialect {
            enable_types: DialectTypes::Disable,
            ..Dialect::AllOptionsInternal
        },
        &["x = ..."],
    );

    assert_eq!(parse("x = ..."), "x = ...\n");
}

#[test]
fn test_nested_def() {
    assert_eq!(
        parse("def foo(x):\n  def bar(y): return y\n  return bar(x)"),
        "def foo(x):\n  def bar(y):\n    return y\n  return bar(x)\n"
    );
}

#[test]
fn test_op_associativity() {
    // Normal operators are left associative
    assert_eq!(parse("1 or 2 or 3 or 4"), "(((1 or 2) or 3) or 4)\n");
    assert_eq!(parse("1 and 2 and 3 and 4"), "(((1 and 2) and 3) and 4)\n");
    assert_eq!(parse("1 | 2 | 3"), "((1 | 2) | 3)\n");
    assert_eq!(parse("1 + 2 + 3"), "((1 + 2) + 3)\n");
    assert_eq!(parse("1 * 2 * 3"), "((1 * 2) * 3)\n");
    // Comparisons are not associative
    // TODO - create a better error message for this case
    let err =
        AstModule::parse("x", "0 <= 1 < 2".to_owned(), &Dialect::AllOptionsInternal).unwrap_err();
    assert!(err.to_string().contains("Parse error"), "{}", err);
}

#[test]
fn test_bad_assignment() {
    parse_fails("bad_assignment", &["[x or y] = 1", "[x] += 1"]);
}

#[test]
fn test_assignment_type_annotation() {
    // We support the same annotation syntax as Python.
    parse("x: int = 1");
    parse("x.y: int = 1");
    parse("x[1]: int = 1");
    parse_fail("assignment_type_annotation", "(x, y): int = foo");
}

#[test]
fn test_test_list_in_index_expr() {
    assert_eq!(parse("x[1, 2]"), "x[1, 2]\n");

    parse_fail("list_in_index_expr", "x[1, 2] = 3");
}

#[test]
fn test_error_unmatched_parens() {
    parse_fails(
        "error_unmatched_parens",
        &[
            "(1 + 2",
            "f(x, y",
            "[1, 2, 3",
            "{1: 2",
            "x = (1 +\n  2 +\n  3",
        ],
    );
}

#[test]
fn test_error_unexpected_token() {
    parse_fails(
        "error_unexpected_token",
        &[
            "x = = 1",
            "_ = *x",
            "def foo(,):\n  pass",
            "f(a=1, *, b=2)",
            "x = [1, , 2]",
            "_ = a + b not c",
            "f(1+2 = 3)",
            "x = {,}",
            "print 1 2",
            "x[1, 2,:]",
            "a = max(range(10)))",
            "def foo(x,,y):\n  pass",
        ],
    );
}

#[test]
fn test_error_unexpected_eof() {
    parse_fails(
        "error_unexpected_eof",
        &[
            "def f():",
            "if True:",
            "if True:\n  pass\nelse:",
            "if True:\n  pass\nelif x:",
            "for x in xs:",
        ],
    );
}

#[test]
fn test_error_missing_colon() {
    parse_fails(
        "error_missing_colon",
        &[
            "if True\n  pass",
            "def foo()\n  pass",
            "for x in []\n  pass",
        ],
    );
}

#[test]
fn test_error_missing_expression() {
    parse_fails(
        "error_missing_expression",
        &["x =", "x +=", "x = 1 +\n2", "return\n1 +"],
    );
}

#[test]
fn test_assignment_regressions_are_rejected() {
    assert_parse_fails_programs(&["x, y\n", "x,\n", "x: int y\n", "x: int\n", "x: int += 1\n"]);
}

#[test]
fn test_error_bad_def() {
    parse_fails(
        "error_bad_def",
        &[
            "def :\n  pass",
            "def pass():\n  pass",
            "def load():\n  pass",
            "def f(load):\n  pass",
            "def f :\n  pass",
            "def foo(x y):\n  pass",
            "def f(a, *-b, c):\n  pass",
            "def foo(**x, y):\n  pass",
        ],
    );
}

#[test]
fn test_error_bad_load() {
    parse_fails(
        "error_bad_load",
        &[
            "load()",
            "load(123)",
            "load(\"foo.bzl\")",
            "load(\"\", 1)",
            "load(1, 2)",
            "load(\"a\", x)",
            "load(\"a\", x2=x)",
        ],
    );
}

#[test]
fn test_error_bad_for() {
    parse_fails("error_bad_for", &["for x + y in []:\n  pass"]);
}

#[test]
fn test_error_chained_comparison() {
    parse_fails(
        "error_chained_comparison",
        &[
            "0 <= 1 < 2",
            "0 == 1 == 2",
            "a == b != c",
            "a < b > c",
            "a in b in c",
            "a in b not in c",
        ],
    );
}

/// `not` only appears at infix position as part of `not in`; the bare
/// sequence `a not b` should be rejected with a focused error message.
#[test]
fn test_error_rd_not_without_in() {
    let name = "error_rd_not_without_in";
    let program = "_ = a not b";
    let err = AstModule::parse(name, program.to_owned(), &Dialect::AllOptionsInternal).unwrap_err();
    let out = format!("Program:\n{program}\n\nError:\n{err}\n");
    golden_test_template(&format!("src/syntax/grammar_tests/{name}.golden"), &out);
}

#[test]
fn test_error_string_unterminated() {
    parse_fails("error_string_unterminated", &["x = \"hello", "x = 'hello"]);
}

#[test]
fn test_error_indentation() {
    parse_fails(
        "error_indentation",
        &[
            "if True:\n    x = 1\n  y = 2",
            "def foo():\n  x = 1\n    y = 2",
        ],
    );
}

#[test]
fn test_error_reserved_keyword() {
    parse_fails(
        "error_reserved_keyword",
        &[
            "class Foo:\n  pass",
            "import os",
            "raise ValueError",
            "load = 1",
            "f(load())",
        ],
    );
}

#[test]
fn test_error_bad_comprehension() {
    parse_fails(
        "error_bad_comprehension",
        &[
            "_ = {x for y in z}",
            "_ = [x for x in 1, 2, 3]",
            "_ = [a for b in c if 1, 2]",
            "_ = [a for b in lambda: c]",
            "_ = [x for x in a if b else c]",
            "[a for b in c else d]",
        ],
    );
}

#[test]
fn test_error_missing_else() {
    parse_fails("error_missing_else", &["_ = a if b"]);
}

#[test]
fn test_error_string_escape() {
    parse_fails("error_string_escape", &["raw = r'a\nb'", "s = \"\\x-0\""]);
}

#[test]
fn test_error_call_arguments() {
    parse_fails(
        "error_call_arguments",
        &[
            "f(x = 1, 2)",
            "f(x = 1, y = 2, 3)",
            "f(x = 1, x = 2)",
            "f(x = 1, x = 2, 3)",
            "f(*args, x = 1)",
            "f(*args, *more_args)",
            "f(**kwargs, **more_kwargs)",
        ],
    );
}

#[test]
fn test_error_precedence() {
    parse_fails(
        "error_precedence",
        &[
            "f(x = 1, 2",
            "def f(**x, y",
            "return 1\n)",
            "def f():\n  load(\"m.bzl\", \"x\")\n)",
        ],
    );
}

#[test]
fn test_error_statement_placement() {
    parse_fails(
        "error_statement_placement",
        &[
            "return 1",
            "break",
            "continue",
            "def f():\n  load(\"m.bzl\", \"x\")",
        ],
    );
}

#[test]
fn test_error_integer_literal() {
    parse_fails("error_integer_literal", &["x = 01", "x = 0123"]);
}

#[test]
fn test_lexer_errors_propagate() {
    assert_parse_error_contains("import os\n", "reserved keyword");
    assert_parse_error_span_source("import os\n", "import");

    assert_parse_error_contains("x = \"hello", "unfinished string literal");
    assert_parse_error_span_source("x = \"hello", "\"hello");
}

#[test]
fn test_parse_errors_point_at_offending_token() {
    assert_parse_error_span_source("x = = 1\n", "=");
    assert_parse_error_span_source("def foo(,):\n  pass\n", ",");
    assert_parse_error_span_source("load(123)\n", "123");
}

#[test]
fn test_chained_comparison_in_call_argument_is_rejected() {
    assert_parse_error_span_source("f(x < y < z)\n", "<");
    assert_parse_error_span_source("f(a in b in c)\n", "in");
}

#[test]
fn test_not_and_not_in_precedence() {
    assert_eq!(parse("x not in y"), "(x not in y)\n");
    assert_eq!(parse("not x in y"), "(not (x in y))\n");
}

#[test]
fn test_identifier_led_call_arguments_continue_parsing() {
    assert_eq!(
        parse("f(x.y, x[0], g(x), x if y else z, x not in y)"),
        "f(x.y, x[0], g(x), (x if y else z), (x not in y))\n"
    );
}

#[test]
fn test_load_alias_forms() {
    assert_display_roundtrip(
        "load(\"m.bzl\", alias = \"real\", \"plain\",)",
        "load(\"m.bzl\", alias = \"real\", \"plain\")\n",
    );
    assert_display_roundtrip("load(\"m.bzl\", \"if\")", "load(\"m.bzl\", \"if\")\n");
}

#[test]
fn test_slice_forms_cover_all_components() {
    assert_assignment_rhs_is_slice("x = a[:2]", "a", None, Some("2"), None);
    assert_assignment_rhs_is_slice("x = a[1:]", "a", Some("1"), None, None);
    assert_assignment_rhs_is_slice("x = a[1:2:3]", "a", Some("1"), Some("2"), Some("3"));
    assert_display_roundtrip("a[1:2]", "a[1:2]\n");
}

#[test]
fn test_f_string_display_roundtrip() {
    assert_display_roundtrip(r#"f"a{x}b""#, "\"a{}b\".format(x)\n");
}

#[test]
fn test_nul_display_roundtrip() {
    assert_display_roundtrip(r#"x = "\x001""#, "x = \"\\x001\"\n");
    assert_display_roundtrip(r#"x = f"\x001{y}""#, "x = \"\\x001{}\".format(y)\n");
    assert_display_roundtrip(
        r#"load("m", symbol = "\x001")"#,
        "load(\"m\", symbol = \"\\x001\")\n",
    );
}

#[test]
fn test_integer_dot_display_roundtrip() {
    assert_display_roundtrip("(1).imag", "(1).imag\n");
}

#[test]
fn test_integer_dot_assignment_display_roundtrip() {
    assert_display_roundtrip("(1).field = value", "(1).field = value\n");
}

#[test]
fn test_unary_assignment_receiver_display_roundtrip() {
    assert_display_roundtrip("(-x).field = value", "(-x).field = value\n");
    assert_display_roundtrip("(+x)[0] = value", "(+x)[0] = value\n");
    assert_display_roundtrip("(~x).field += value", "(~x).field += value\n");
}

#[test]
fn test_unary_postfix_receiver_display_roundtrip() {
    assert_display_roundtrip("(-x).field", "(-x).field\n");
    assert_display_roundtrip("(+f)()", "(+f)()\n");
    assert_display_roundtrip("(~x)[i]", "(~x)[i]\n");
    assert_display_roundtrip("(-x)[i, j]", "(-x)[i, j]\n");
    assert_display_roundtrip("(-x)[a:b:c]", "(-x)[a:b:c]\n");
}

#[test]
fn test_float_display_roundtrip() {
    assert_display_roundtrip("1.0", "1.0\n");
}

#[test]
fn test_infinite_float_display_roundtrip() {
    assert_display_roundtrip("1e400", "1e400\n");
    assert_display_roundtrip("-1e400", "-1e400\n");
}

#[test]
fn test_dict_comprehension_parses() {
    assert_eq!(
        parse("x = {k: v for k in xs if pred(k)}"),
        "x = {k: v for k in xs if pred(k)}\n"
    );
}

#[test]
fn test_if_elif_span_covers_full_chain() {
    let program = "if True:\n  a\nelif False:\n  b\nelse:\n  c\n";
    let ast = parse_ast(program);
    assert_eq!(
        ast.codemap
            .source_span(ast.statement.span)
            .trim_end_matches('\n'),
        "if True:\n  a\nelif False:\n  b\nelse:\n  c",
    );
}

#[test]
fn test_error_tuple_trailing_comma() {
    parse_fails(
        "error_tuple_trailing_comma",
        &[
            "_ = 1,",
            "for k, v, in dict.items():\n  pass",
            "_ = [(v, k) for k, v, in dict.items()]",
            "a, b, = 1, 2",
            "a, b = 1, 2,",
        ],
    );
}

#[test]
fn test_non_ascii() {
    parse_fails("error_non_ascii", &["(\"☃\", 1 +, 1)", "[\"☃\"] += 1"]);
}

pub fn parse(program: &str) -> String {
    parse_ast(program).statement.to_string()
}

fn assert_display_roundtrip(program: &str, expected: &str) {
    let displayed = parse(program);
    assert_eq!(displayed, expected);
    assert_eq!(parse(&displayed), displayed);
}

pub fn parse_ast(program: &str) -> AstModule {
    parse_ast_with_dialect(program, &Dialect::AllOptionsInternal)
}

fn parse_with_dialect(program: &str, dialect: &Dialect) -> String {
    parse_ast_with_dialect(program, dialect)
        .statement
        .to_string()
}

fn parse_ast_with_dialect(program: &str, dialect: &Dialect) -> AstModule {
    AstModule::parse("assert.bzl", program.to_owned(), dialect)
        .unwrap_or_else(|err| panic!("Expected parse success for:\n{program}\nError: {err}"))
}
