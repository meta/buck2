/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Structured Starlark fuzzer for starlark_fmt using quickcheck.
//!
//! Generates syntactically plausible Starlark source code — with `load()`
//! statements, rule calls, function definitions, dict/list/tuple literals,
//! string expressions, comments, and `# fmt: off` pragmas — then verifies
//! the formatter doesn't panic and produces idempotent output.
//!
//! Uses quickcheck for automatic test case shrinking: when a failure is found,
//! it minimizes the input to the smallest reproducing case.

use quickcheck::Arbitrary;
use quickcheck::Gen;
use quickcheck_arbitrary_derive::Arbitrary;

// ── AST types ────────────────────────────────────────────────────────────
//
// Each type implements `Arbitrary` with shrinking so quickcheck can minimize
// failing inputs automatically.

/// A complete Starlark source file.
#[derive(Debug, Clone)]
struct StarlarkSource(Vec<Stmt>);

/// A top-level statement.
#[derive(Debug, Clone)]
enum Stmt {
    Load(LoadStmt),
    RuleCall(RuleCallStmt),
    FuncDef(FuncDefStmt),
    Assignment(Ident, Expr),
    ForLoop(Ident, Expr),
    IfBlock(Expr, bool),
    Comment(CommentKind),
    BlankLine,
}

#[derive(Debug, Clone)]
struct LoadStmt {
    pkg: LoadPkg,
    symbols: Vec<(Ident, bool)>, // (name, aliased?)
}

#[derive(Debug, Clone)]
struct RuleCallStmt {
    name: Ident,
    kwargs: Vec<(Ident, Expr)>,
    trailing_comma: bool,
}

#[derive(Debug, Clone)]
struct FuncDefStmt {
    name: Ident,
    params: Vec<Param>,
    body: Vec<BodyStmt>,
}

#[derive(Debug, Clone, Arbitrary)]
enum Param {
    Simple(Ident),
    Default(Ident, Expr),
    Kwargs(Ident),
}

#[derive(Debug, Clone, Arbitrary)]
enum BodyStmt {
    Assign(Ident, Expr),
    Return(Expr),
    Expr(Expr),
    Pass,
}

#[derive(Debug, Clone)]
enum Expr {
    Str(StrContent, StrStyle),
    Int(i16),
    Bool(bool),
    None,
    Ident(Ident),
    List(Vec<Expr>, bool), // (items, multiline?)
    Dict(Vec<(Expr, Expr)>),
    Tuple(Vec<Expr>),
    Call(Ident, Vec<Expr>, Vec<(Ident, Expr)>),
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    Select(Vec<(Expr, Expr)>),
    Glob(Vec<StrContent>, Vec<StrContent>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    ListComp(Box<Expr>, Ident, Box<Expr>),
}

#[derive(Debug, Clone, Copy, Arbitrary)]
enum BinOp {
    Add,
    And,
    Or,
    Eq,
}

#[derive(Debug, Clone, Copy, Arbitrary)]
enum CommentKind {
    Todo,
    Fixme,
    Note,
    FmtOff,
    FmtOn,
}

// ── Leaf enums with fixed pools ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, Arbitrary)]
struct Ident(u8); // index into IDENTS

const IDENTS: &[&str] = &[
    "name",
    "deps",
    "srcs",
    "visibility",
    "labels",
    "default_target_platform",
    "headers",
    "exported_deps",
    "tests",
    "cxx_library",
    "rust_binary",
    "python_library",
    "genrule",
    "ocaml_library",
    "sh_test",
    "filegroup",
    "export_file",
    "http_archive",
    "rule_a",
    "rule_b",
    "foo_bar",
    "baz_qux",
    "my_func",
    "helper",
    "util",
    "config",
    "platform",
    "selector",
    "select",
    "glob",
];

#[derive(Debug, Clone, Copy, Arbitrary)]
struct LoadPkg(u8); // index into LOAD_PACKAGES

const LOAD_PACKAGES: &[&str] = &[
    "//core:defs.bzl",
    "//tools/build_defs:rules.bzl",
    "//common/lib:utils.bzl",
    "@fbsource//tools/build_defs:rust_binary.bzl",
    "@fbsource//tools/build_defs:cxx_library.bzl",
    ":local_defs.bzl",
    "//third-party:defs.bzl",
    "//build/infra:macros.bzl",
];

#[derive(Debug, Clone, Copy, Arbitrary)]
struct StrContent(u8); // index into STRING_CONTENTS

const STRING_CONTENTS: &[&str] = &[
    "//some/package:target",
    "//foo/bar:baz",
    "src/main.rs",
    "include/header.h",
    "**/*.rs",
    "**/*.py",
    "test_*.py",
    "",
    "x",
    "PUBLIC",
    "DEFAULT",
    "//platforms:linux",
    "//platforms:macos",
    "//platforms:windows",
    "fbsource//third-party/rust:anyhow",
    "fbsource//third-party/rust:serde",
];

#[derive(Debug, Clone, Copy, Arbitrary)]
enum StrStyle {
    Double,
    Single,
    Triple,
}

// ── Arbitrary impls ──────────────────────────────────────────────────────
//
// Leaf types (Ident, LoadPkg, StrContent, StrStyle, BinOp, CommentKind,
// Param, BodyStmt) use #[derive(Arbitrary)] from quickcheck_arbitrary_derive.
// Only Expr, Stmt, and StarlarkSource need manual impls for depth control
// and non-empty invariants.

impl Arbitrary for Expr {
    fn arbitrary(g: &mut Gen) -> Self {
        let size = g.size();
        if size <= 1 {
            // Base cases only at small sizes
            return match u8::arbitrary(g) % 5 {
                0 => Expr::Str(StrContent::arbitrary(g), StrStyle::arbitrary(g)),
                1 => Expr::Int(i16::arbitrary(g)),
                2 => Expr::Bool(bool::arbitrary(g)),
                3 => Expr::None,
                _ => Expr::Ident(Ident::arbitrary(g)),
            };
        }

        // Use a smaller generator for recursive children
        let mut smaller = Gen::new(size / 3);

        match u8::arbitrary(g) % 14 {
            0 => Expr::Str(StrContent::arbitrary(g), StrStyle::arbitrary(g)),
            1 => Expr::Int(i16::arbitrary(g)),
            2 => Expr::Bool(bool::arbitrary(g)),
            3 => Expr::None,
            4 => Expr::Ident(Ident::arbitrary(g)),
            5 => {
                let items: Vec<Expr> = Arbitrary::arbitrary(&mut smaller);
                Expr::List(items, bool::arbitrary(g))
            }
            6 => {
                let entries: Vec<(Expr, Expr)> = Arbitrary::arbitrary(&mut smaller);
                Expr::Dict(entries)
            }
            7 => {
                let items: Vec<Expr> = Arbitrary::arbitrary(&mut smaller);
                Expr::Tuple(items)
            }
            8 => Expr::Call(
                Ident::arbitrary(g),
                Arbitrary::arbitrary(&mut smaller),
                Arbitrary::arbitrary(&mut smaller),
            ),
            9 => Expr::BinOp(
                Box::new(Expr::arbitrary(&mut smaller)),
                BinOp::arbitrary(g),
                Box::new(Expr::arbitrary(&mut smaller)),
            ),
            10 => {
                let arms: Vec<(Expr, Expr)> = Arbitrary::arbitrary(&mut smaller);
                Expr::Select(if arms.is_empty() {
                    vec![(Expr::Str(StrContent(0), StrStyle::Double), Expr::None)]
                } else {
                    arms
                })
            }
            11 => Expr::Glob(
                Arbitrary::arbitrary(&mut smaller),
                Arbitrary::arbitrary(&mut smaller),
            ),
            12 => Expr::Ternary(
                Box::new(Expr::arbitrary(&mut smaller)),
                Box::new(Expr::arbitrary(&mut smaller)),
                Box::new(Expr::arbitrary(&mut smaller)),
            ),
            _ => Expr::ListComp(
                Box::new(Expr::arbitrary(&mut smaller)),
                Ident::arbitrary(g),
                Box::new(Expr::arbitrary(&mut smaller)),
            ),
        }
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        match self {
            // Terminal nodes don't shrink further (except to simpler terminals)
            Expr::None => Box::new(std::iter::empty()),
            Expr::Bool(_) => Box::new(std::iter::once(Expr::None)),
            Expr::Int(_) => Box::new(std::iter::once(Expr::None)),
            Expr::Str(_, _) => Box::new(std::iter::once(Expr::None)),
            Expr::Ident(_) => Box::new(std::iter::once(Expr::None)),

            // Compound nodes shrink by removing children or simplifying to children
            Expr::List(items, ml) => {
                let ml = *ml;
                let items = items.clone();
                Box::new(
                    std::iter::once(Expr::None)
                        .chain(items.shrink().map(move |i| Expr::List(i, ml))),
                )
            }
            Expr::Dict(entries) => {
                let entries = entries.clone();
                Box::new(std::iter::once(Expr::None).chain(entries.shrink().map(Expr::Dict)))
            }
            Expr::Tuple(items) => {
                let items = items.clone();
                Box::new(std::iter::once(Expr::None).chain(items.shrink().map(Expr::Tuple)))
            }
            Expr::Call(name, args, kwargs) => {
                let name = *name;
                let args = args.clone();
                let kwargs = kwargs.clone();
                Box::new(
                    std::iter::once(Expr::None).chain(
                        (args, kwargs)
                            .shrink()
                            .map(move |(a, k)| Expr::Call(name, a, k)),
                    ),
                )
            }
            Expr::BinOp(l, _, r) => {
                let l = l.clone();
                let r = r.clone();
                Box::new(vec![Expr::None, *l, *r].into_iter())
            }
            Expr::Select(arms) => {
                let arms = arms.clone();
                Box::new(
                    std::iter::once(Expr::None)
                        .chain(arms.shrink().filter(|a| !a.is_empty()).map(Expr::Select)),
                )
            }
            Expr::Glob(pats, excl) => {
                let pats = pats.clone();
                let excl = excl.clone();
                Box::new(
                    std::iter::once(Expr::None)
                        .chain((pats, excl).shrink().map(|(p, e)| Expr::Glob(p, e))),
                )
            }
            Expr::Ternary(t, c, f) => {
                let t = t.clone();
                let c = c.clone();
                let f = f.clone();
                Box::new(vec![Expr::None, *t, *c, *f].into_iter())
            }
            Expr::ListComp(e, _, iter) => {
                let e = e.clone();
                let iter = iter.clone();
                Box::new(vec![Expr::None, *e, *iter].into_iter())
            }
        }
    }
}

impl Arbitrary for Stmt {
    fn arbitrary(g: &mut Gen) -> Self {
        let mut smaller = Gen::new(g.size() / 2);
        match u8::arbitrary(g) % 9 {
            0 => Stmt::Load(LoadStmt {
                pkg: LoadPkg::arbitrary(g),
                symbols: {
                    let mut syms: Vec<(Ident, bool)> = Arbitrary::arbitrary(&mut smaller);
                    if syms.is_empty() {
                        syms.push((Ident::arbitrary(g), false));
                    }
                    syms
                },
            }),
            1 | 2 => Stmt::RuleCall(RuleCallStmt {
                name: Ident::arbitrary(g),
                kwargs: {
                    let mut kw: Vec<(Ident, Expr)> = Arbitrary::arbitrary(&mut smaller);
                    if kw.is_empty() {
                        kw.push((Ident::arbitrary(g), Expr::arbitrary(&mut smaller)));
                    }
                    kw
                },
                trailing_comma: bool::arbitrary(g),
            }),
            3 => Stmt::FuncDef(FuncDefStmt {
                name: Ident::arbitrary(g),
                params: Arbitrary::arbitrary(&mut smaller),
                body: {
                    let mut body: Vec<BodyStmt> = Arbitrary::arbitrary(&mut smaller);
                    if body.is_empty() {
                        body.push(BodyStmt::Pass);
                    }
                    body
                },
            }),
            4 => Stmt::Assignment(Ident::arbitrary(g), Expr::arbitrary(&mut smaller)),
            5 => Stmt::ForLoop(Ident::arbitrary(g), Expr::arbitrary(&mut smaller)),
            6 => Stmt::IfBlock(Expr::arbitrary(&mut smaller), bool::arbitrary(g)),
            7 => Stmt::Comment(CommentKind::arbitrary(g)),
            _ => Stmt::BlankLine,
        }
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        match self {
            Stmt::BlankLine => Box::new(std::iter::empty()),
            Stmt::Comment(_) => Box::new(std::iter::once(Stmt::BlankLine)),
            Stmt::Assignment(id, expr) => {
                let id = *id;
                let expr = expr.clone();
                Box::new(
                    std::iter::once(Stmt::BlankLine)
                        .chain(expr.shrink().map(move |e| Stmt::Assignment(id, e))),
                )
            }
            Stmt::Load(load) => {
                let load = load.clone();
                Box::new(
                    std::iter::once(Stmt::BlankLine).chain(
                        load.symbols
                            .shrink()
                            .filter(|s| !s.is_empty())
                            .map(move |syms| {
                                Stmt::Load(LoadStmt {
                                    pkg: load.pkg,
                                    symbols: syms,
                                })
                            }),
                    ),
                )
            }
            Stmt::RuleCall(rc) => {
                let rc = rc.clone();
                Box::new(std::iter::once(Stmt::BlankLine).chain(
                    rc.kwargs.shrink().filter(|k| !k.is_empty()).map(move |kw| {
                        Stmt::RuleCall(RuleCallStmt {
                            name: rc.name,
                            kwargs: kw,
                            trailing_comma: rc.trailing_comma,
                        })
                    }),
                ))
            }
            Stmt::FuncDef(fd) => {
                let fd = fd.clone();
                Box::new(
                    std::iter::once(Stmt::BlankLine).chain(
                        (fd.params.clone(), fd.body.clone()).shrink().map(
                            move |(params, mut body)| {
                                if body.is_empty() {
                                    body.push(BodyStmt::Pass);
                                }
                                Stmt::FuncDef(FuncDefStmt {
                                    name: fd.name,
                                    params,
                                    body,
                                })
                            },
                        ),
                    ),
                )
            }
            Stmt::ForLoop(id, expr) => {
                let id = *id;
                let expr = expr.clone();
                Box::new(
                    std::iter::once(Stmt::BlankLine)
                        .chain(expr.shrink().map(move |e| Stmt::ForLoop(id, e))),
                )
            }
            Stmt::IfBlock(expr, has_else) => {
                let has_else = *has_else;
                let expr = expr.clone();
                Box::new(
                    std::iter::once(Stmt::BlankLine)
                        .chain(expr.shrink().map(move |e| Stmt::IfBlock(e, has_else))),
                )
            }
        }
    }
}

impl Arbitrary for StarlarkSource {
    fn arbitrary(g: &mut Gen) -> Self {
        let mut stmts: Vec<Stmt> = Arbitrary::arbitrary(g);
        if stmts.is_empty() {
            stmts.push(Stmt::Assignment(Ident(0), Expr::None));
        }
        // Cap at 15 statements to keep generated code reasonable
        stmts.truncate(15);
        StarlarkSource(stmts)
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        let stmts = self.0.clone();
        Box::new(stmts.shrink().filter(|s| !s.is_empty()).map(StarlarkSource))
    }
}

// ── Rendering ────────────────────────────────────────────────────────────

impl Ident {
    fn as_str(self) -> &'static str {
        IDENTS[self.0 as usize % IDENTS.len()]
    }
}

impl StrContent {
    fn as_str(self) -> &'static str {
        STRING_CONTENTS[self.0 as usize % STRING_CONTENTS.len()]
    }
}

impl StarlarkSource {
    fn render(&self) -> String {
        let mut out = String::new();
        for stmt in &self.0 {
            stmt.render(&mut out);
            out.push('\n');
        }
        out
    }
}

impl Stmt {
    fn render(&self, out: &mut String) {
        match self {
            Stmt::Load(load) => {
                let pkg = LOAD_PACKAGES[load.pkg.0 as usize % LOAD_PACKAGES.len()];
                out.push_str(&format!("load(\"{}\"", pkg));
                for (sym, aliased) in &load.symbols {
                    if *aliased {
                        let name = sym.as_str();
                        out.push_str(&format!(", {} = \"{}\"", name, name));
                    } else {
                        out.push_str(&format!(", \"{}\"", sym.as_str()));
                    }
                }
                out.push(')');
            }
            Stmt::RuleCall(rc) => {
                out.push_str(rc.name.as_str());
                out.push_str("(\n");
                for (i, (key, val)) in rc.kwargs.iter().enumerate() {
                    out.push_str("    ");
                    out.push_str(key.as_str());
                    out.push('=');
                    val.render(out, 1);
                    if i < rc.kwargs.len() - 1 || rc.trailing_comma {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push(')');
            }
            Stmt::FuncDef(fd) => {
                out.push_str("def ");
                out.push_str(fd.name.as_str());
                out.push('(');
                for (i, p) in fd.params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    match p {
                        Param::Simple(id) => out.push_str(id.as_str()),
                        Param::Default(id, expr) => {
                            out.push_str(id.as_str());
                            out.push_str(" = ");
                            expr.render(out, 3);
                        }
                        Param::Kwargs(id) => {
                            out.push_str("**");
                            out.push_str(id.as_str());
                        }
                    }
                }
                out.push_str("):\n");
                for stmt in &fd.body {
                    out.push_str("    ");
                    match stmt {
                        BodyStmt::Assign(id, expr) => {
                            out.push_str(id.as_str());
                            out.push_str(" = ");
                            expr.render(out, 1);
                        }
                        BodyStmt::Return(expr) => {
                            out.push_str("return ");
                            expr.render(out, 1);
                        }
                        BodyStmt::Expr(expr) => expr.render(out, 1),
                        BodyStmt::Pass => out.push_str("pass"),
                    }
                    out.push('\n');
                }
            }
            Stmt::Assignment(id, expr) => {
                out.push_str(id.as_str());
                out.push_str(" = ");
                expr.render(out, 0);
            }
            Stmt::ForLoop(id, expr) => {
                out.push_str("for ");
                out.push_str(id.as_str());
                out.push_str(" in ");
                expr.render(out, 0);
                out.push_str(":\n    pass");
            }
            Stmt::IfBlock(expr, has_else) => {
                out.push_str("if ");
                expr.render(out, 0);
                out.push_str(":\n    pass\n");
                if *has_else {
                    out.push_str("else:\n    pass");
                }
            }
            Stmt::Comment(kind) => {
                out.push_str(match kind {
                    CommentKind::Todo => "# TODO: fix this later",
                    CommentKind::Fixme => "# FIXME: broken",
                    CommentKind::Note => "# NOTE: important detail",
                    CommentKind::FmtOff => "# fmt: off",
                    CommentKind::FmtOn => "# fmt: on",
                });
            }
            Stmt::BlankLine => {}
        }
    }
}

impl Expr {
    fn render(&self, out: &mut String, depth: usize) {
        if depth > 6 {
            out.push_str("None");
            return;
        }
        match self {
            Expr::Str(content, style) => {
                let s = content.as_str();
                match style {
                    StrStyle::Double => out.push_str(&format!("\"{}\"", s)),
                    StrStyle::Single => out.push_str(&format!("'{}'", s)),
                    StrStyle::Triple => out.push_str(&format!("\"\"\"{}\"\"\"", s)),
                }
            }
            Expr::Int(n) => out.push_str(&n.to_string()),
            Expr::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
            Expr::None => out.push_str("None"),
            Expr::Ident(id) => out.push_str(id.as_str()),
            Expr::List(items, multiline) => {
                if items.is_empty() {
                    out.push_str("[]");
                } else if *multiline {
                    out.push_str("[\n");
                    for item in items {
                        out.push_str("    ");
                        item.render(out, depth + 1);
                        out.push_str(",\n");
                    }
                    out.push(']');
                } else {
                    out.push('[');
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        item.render(out, depth + 1);
                    }
                    out.push(']');
                }
            }
            Expr::Dict(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                } else {
                    out.push('{');
                    for (i, (k, v)) in entries.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        k.render(out, depth + 1);
                        out.push_str(": ");
                        v.render(out, depth + 1);
                    }
                    out.push('}');
                }
            }
            Expr::Tuple(items) => {
                out.push('(');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.render(out, depth + 1);
                }
                if items.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            }
            Expr::Call(func, args, kwargs) => {
                out.push_str(func.as_str());
                out.push('(');
                let mut first = true;
                for arg in args {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    arg.render(out, depth + 1);
                }
                for (key, val) in kwargs {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(key.as_str());
                    out.push_str(" = ");
                    val.render(out, depth + 1);
                }
                out.push(')');
            }
            Expr::BinOp(left, op, right) => {
                left.render(out, depth + 1);
                out.push_str(match op {
                    BinOp::Add => " + ",
                    BinOp::And => " and ",
                    BinOp::Or => " or ",
                    BinOp::Eq => " == ",
                });
                right.render(out, depth + 1);
            }
            Expr::Select(arms) => {
                out.push_str("select({\n");
                for (k, v) in arms {
                    out.push_str("    ");
                    k.render(out, depth + 1);
                    out.push_str(": ");
                    v.render(out, depth + 1);
                    out.push_str(",\n");
                }
                out.push_str("})");
            }
            Expr::Glob(patterns, exclude) => {
                out.push_str("glob([");
                for (i, p) in patterns.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("\"{}\"", p.as_str()));
                }
                out.push(']');
                if !exclude.is_empty() {
                    out.push_str(", exclude = [");
                    for (i, p) in exclude.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&format!("\"{}\"", p.as_str()));
                    }
                    out.push(']');
                }
                out.push(')');
            }
            Expr::Ternary(t, c, f) => {
                t.render(out, depth + 1);
                out.push_str(" if ");
                c.render(out, depth + 1);
                out.push_str(" else ");
                f.render(out, depth + 1);
            }
            Expr::ListComp(expr, var, iter) => {
                out.push('[');
                expr.render(out, depth + 1);
                out.push_str(" for ");
                out.push_str(var.as_str());
                out.push_str(" in ");
                iter.render(out, depth + 1);
                out.push(']');
            }
        }
    }
}

impl std::fmt::Display for StarlarkSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatting::format_file;

    /// quickcheck property: formatting is idempotent — format(format(x)) == format(x).
    /// When this fails, quickcheck automatically shrinks to the minimal reproducing input.
    #[quickcheck_macros::quickcheck]
    fn format_is_idempotent(source: StarlarkSource) -> quickcheck::TestResult {
        let rendered = source.render();

        let first = match format_file(rendered) {
            Ok(f) => f,
            // Parse errors are expected for some generated inputs — skip them
            Err(_) => return quickcheck::TestResult::discard(),
        };

        let second = match format_file(first.clone()) {
            Ok(f) => f,
            Err(e) => {
                return quickcheck::TestResult::error(format!("second format failed: {}", e));
            }
        };

        if first == second {
            quickcheck::TestResult::passed()
        } else {
            quickcheck::TestResult::error(format!(
                "NOT IDEMPOTENT\n--- first ---\n{}\n--- second ---\n{}",
                first, second,
            ))
        }
    }

    /// Smoke test: a hand-crafted input that exercises multiple autofix passes.
    #[test]
    fn test_complex_starlark_formats_without_panic() {
        let source = r#"
load("//zzz:defs.bzl", "rule_z")
load("//aaa:defs.bzl", "rule_a")
load("//mmm:defs.bzl", "unused_rule")

rule_a(
    visibility = ["PUBLIC"],
    name = "my_target",
    deps = [
        "//zzz:dep_z",
        "//aaa:dep_a",
        "//mmm:dep_m",
    ],
    srcs = glob(["**/*.rs"], exclude = ["**/test_*.rs"]),
    labels = ["generated"],
)

config = select({
    "//platforms:linux": ["linux_dep"],
    "//platforms:macos": ["macos_dep"],
})

def helper(name, deps = [], **kwargs):
    for dep in deps:
        if dep in config:
            pass
    return name + "_processed"
"#;
        let result = format_file(source.to_owned());
        assert!(
            result.is_ok(),
            "complex starlark should format: {:?}",
            result.err()
        );

        let first = result.unwrap();
        let second = format_file(first.clone()).expect("second format failed");
        assert_eq!(first, second, "complex starlark should be idempotent");
    }
}
