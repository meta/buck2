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

use std::collections::HashMap;
use std::fs;
use std::mem;
use std::path::Path;

use derivative::Derivative;
use dupe::Dupe;

use crate::codemap::CodeMap;
use crate::codemap::FileSpan;
use crate::codemap::Pos;
use crate::codemap::Span;
use crate::codemap::Spanned;
use crate::lexer::Lexer;
use crate::lexer::Token;
use crate::syntax::AstLoad;
use crate::syntax::Dialect;
use crate::syntax::ast::ArgumentP;
use crate::syntax::ast::AstExpr;
use crate::syntax::ast::AstStmt;
use crate::syntax::ast::CallArgsP;
use crate::syntax::ast::ExprP;
use crate::syntax::ast::IdentP;
use crate::syntax::ast::LoadArgP;
use crate::syntax::ast::Stmt;
use crate::syntax::lint_suppressions::LintSuppressions;
use crate::syntax::lint_suppressions::LintSuppressionsBuilder;
use crate::syntax::parser_rd;
use crate::syntax::state::ParserState;
use crate::syntax::validate::validate_module;

/// A representation of a Starlark module abstract syntax tree.
///
/// Created with either [`parse`](AstModule::parse) or [`parse_file`](AstModule::parse_file),
/// and evaluated with `Evaluator::eval_module`.
///
/// The internal details (statements/expressions) are deliberately omitted, as they change
/// more regularly. A few methods to obtain information about the AST are provided.
#[derive(Derivative)]
#[derivative(Debug, Clone)]
pub struct AstModule {
    #[derivative(Debug = "ignore")]
    pub(crate) codemap: CodeMap,
    pub(crate) statement: AstStmt,
    pub(crate) dialect: Dialect,
    /// Opt-in typecheck.
    /// Specified with `@starlark-rust: typecheck`.
    pub(crate) typecheck: bool,
    /// Lint issues suppressed in this module using inline comments of shape
    /// # starlark-lint-disable <ISSUE_NAME>, <ISSUE_NAME>, ...
    lint_suppressions: LintSuppressions,
    /// Spans of all comments found in the source, in source order.
    /// Each span includes the leading `#`.
    comment_spans: Vec<Span>,
}

/// This trait is not exported as public API of starlark.
pub trait AstModuleFields: Sized {
    fn codemap(&self) -> &CodeMap;

    fn statement(&self) -> &AstStmt;

    fn dialect(&self) -> &Dialect;

    fn into_parts(self) -> (CodeMap, AstStmt, Dialect, bool);
}

impl AstModuleFields for AstModule {
    fn codemap(&self) -> &CodeMap {
        &self.codemap
    }

    fn statement(&self) -> &AstStmt {
        &self.statement
    }

    fn dialect(&self) -> &Dialect {
        &self.dialect
    }

    fn into_parts(self) -> (CodeMap, AstStmt, Dialect, bool) {
        (self.codemap, self.statement, self.dialect, self.typecheck)
    }
}

impl AstModule {
    fn create(
        codemap: CodeMap,
        statement: AstStmt,
        dialect: &Dialect,
        typecheck: bool,
        lint_suppressions: LintSuppressions,
        comment_spans: Vec<Span>,
    ) -> crate::Result<AstModule> {
        let mut errors = Vec::new();
        validate_module(
            &statement,
            &mut ParserState {
                codemap: &codemap,
                dialect,
                errors: &mut errors,
            },
        );
        // We need the first error, so we don't use `.pop()`.
        if let Some(err) = errors.into_iter().next() {
            return Err(err.into_error());
        }
        Ok(AstModule {
            codemap,
            statement,
            dialect: dialect.clone(),
            typecheck,
            lint_suppressions,
            comment_spans,
        })
    }

    /// Parse a file stored on disk. For details see [`parse`](AstModule::parse).
    pub fn parse_file(path: &Path, dialect: &Dialect) -> crate::Result<Self> {
        let content = fs::read_to_string(path).map_err(anyhow::Error::new)?;
        Self::parse(&path.to_string_lossy(), content, dialect)
    }

    /// Parse a Starlark module to produce an [`AstModule`], or an error if there are syntax errors.
    /// The `filename` is for error messages only, and does not have to be a valid file.
    /// The [`Dialect`] selects which Starlark constructs are valid.
    ///
    /// The returned error may contain diagnostic information. For example:
    ///
    /// ```
    /// use starlark_syntax::codemap::FileSpan;
    /// use starlark_syntax::syntax::AstModule;
    /// use starlark_syntax::syntax::Dialect;
    ///
    /// let err: starlark_syntax::Error =
    ///     AstModule::parse("filename", "\n(unmatched".to_owned(), &Dialect::Standard).unwrap_err();
    /// let span: &FileSpan = err.span().unwrap();
    /// assert_eq!(span.to_string(), "filename:2:11");
    /// ```
    pub fn parse(filename: &str, content: String, dialect: &Dialect) -> crate::Result<Self> {
        let typecheck = content.contains("@starlark-rust: typecheck");
        let codemap = CodeMap::new(filename.to_owned(), content);
        let lexer = Lexer::new(codemap.source(), dialect, codemap.dupe());
        // Store lint suppressions found during parsing
        let mut lint_suppressions_builder = LintSuppressionsBuilder::new();
        // Keep track of block of comments, used for accumulating lint suppressions
        let mut in_comment_block = false;
        let mut comment_spans = Vec::new();
        let mut errors = Vec::new();

        let filtered = lexer.filter(|token| match token {
            Ok((start, Token::Comment(comment), end)) => {
                lint_suppressions_builder.parse_comment(&codemap, comment, *start, *end);
                comment_spans.push(Span::new(Pos::new(*start as u32), Pos::new(*end as u32)));
                in_comment_block = true;
                false
            }
            _ => {
                if in_comment_block {
                    lint_suppressions_builder.end_of_comment_block(&codemap);
                    in_comment_block = false;
                }
                true
            }
        });

        let mut state = ParserState {
            codemap: &codemap,
            dialect,
            errors: &mut errors,
        };

        match parser_rd::parse_module(&mut state, filtered) {
            Ok(v) => {
                if let Some(err) = errors.into_iter().next() {
                    return Err(err.into_error());
                }
                Ok(AstModule::create(
                    codemap,
                    v,
                    dialect,
                    typecheck,
                    lint_suppressions_builder.build(),
                    comment_spans,
                )?)
            }
            Err(e) => Err(e.into_error()),
        }
    }

    /// Return the file names of all the `load` statements in the module.
    /// If the [`Dialect`] had [`enable_load`](Dialect::enable_load) set to [`false`] this will be an empty list.
    pub fn loads(&self) -> Vec<AstLoad<'_>> {
        // We know that `load` statements must be at the top-level, so no need to descend inside `if`, `for`, `def` etc.
        // There is a suggestion that `load` statements should be at the top of a file, but we tolerate that not being true.
        fn f<'a>(ast: &'a AstStmt, codemap: &CodeMap, vec: &mut Vec<AstLoad<'a>>) {
            match &ast.node {
                Stmt::Load(load) => vec.push(AstLoad {
                    span: FileSpan {
                        file: codemap.dupe(),
                        span: load.module.span,
                    },
                    module_id: &load.module.node,
                    symbols: load
                        .args
                        .iter()
                        .map(|LoadArgP { local, their, .. }| {
                            (local.node.ident.as_str(), their.node.as_str())
                        })
                        .collect(),
                }),
                Stmt::Statements(stmts) => {
                    for s in stmts {
                        f(s, codemap, vec);
                    }
                }
                _ => {}
            }
        }

        let mut loads = Vec::new();
        f(&self.statement, &self.codemap, &mut loads);
        loads
    }

    /// Look up a [`Span`] contained in this module to a [`FileSpan`].
    pub fn file_span(&self, x: Span) -> FileSpan {
        self.codemap.file_span(x)
    }

    /// Get back the AST statement for the module
    pub fn statement(&self) -> &AstStmt {
        &self.statement
    }

    /// Locations where statements occur.
    pub fn stmt_locations(&self) -> Vec<FileSpan> {
        fn go(x: &AstStmt, codemap: &CodeMap, res: &mut Vec<FileSpan>) {
            match &**x {
                Stmt::Statements(_) => {} // These are not interesting statements that come up
                _ => res.push(FileSpan {
                    span: x.span,
                    file: codemap.dupe(),
                }),
            }
            x.visit_stmt(|x| go(x, codemap, res))
        }

        let mut res = Vec::new();
        go(&self.statement, &self.codemap, &mut res);
        res
    }

    /// Function to help people who want to write deeper AST transformations in Starlark.
    /// Likely to break type checking and LSP support to some extent.
    ///
    /// Replacement must be a map from operator name (e.g. `+` or `==`) to a function name
    /// (e.g. `my_plus` or `my_equals`).
    pub fn replace_binary_operators(&mut self, replace: &HashMap<String, String>) {
        fn f(x: &mut AstExpr, replace: &HashMap<String, String>) {
            let mut temp = ExprP::Tuple(vec![]);
            mem::swap(&mut x.node, &mut temp);
            let mut res = match temp {
                ExprP::Op(lhs, op, rhs) => match replace.get(op.to_string().trim()) {
                    Some(func) => ExprP::Call(
                        Box::new(Spanned {
                            span: x.span,
                            node: ExprP::Identifier(Spanned {
                                span: x.span,
                                node: IdentP {
                                    ident: func.clone(),
                                    payload: (),
                                },
                            }),
                        }),
                        CallArgsP {
                            args: vec![
                                Spanned {
                                    span: lhs.span,
                                    node: ArgumentP::Positional(*lhs),
                                },
                                Spanned {
                                    span: rhs.span,
                                    node: ArgumentP::Positional(*rhs),
                                },
                            ],
                        },
                    ),
                    None => ExprP::Op(lhs, op, rhs),
                },
                _ => temp,
            };
            mem::swap(&mut x.node, &mut res);
            x.visit_expr_mut(|x| f(x, replace));
        }

        self.statement.visit_expr_mut(|x| f(x, replace));
    }

    /// Check if a given Lint short_name and span is suppressed in this module
    pub fn is_suppressed(&self, issue_short_name: &str, issue_span: Span) -> bool {
        self.lint_suppressions
            .is_suppressed(issue_short_name, issue_span)
    }

    /// Returns the spans of all comments in source order. Each span includes
    /// the leading `#`. Use [`file_span`](AstModule::file_span) to obtain a
    /// [`FileSpan`], then [`FileSpan::source_span`] to get the source text.
    pub fn comments(&self) -> &[Span] {
        &self.comment_spans
    }
}

#[cfg(test)]
mod tests {
    use crate::slice_vec_ext::SliceExt;
    use crate::syntax::grammar_tests;

    #[test]
    fn test_locations() {
        fn get(code: &str) -> String {
            grammar_tests::parse_ast(code)
                .stmt_locations()
                .map(|x| x.resolve_span().to_string())
                .join(" ")
        }

        assert_eq!(&get("foo"), "1:1-4");
        assert_eq!(&get("foo\ndef x():\n   pass"), "1:1-4 2:1-3:8 3:4-8");
    }

    #[test]
    fn test_comments_empty() {
        let module = grammar_tests::parse_ast("x = 1");
        assert!(module.comments().is_empty());
    }

    #[test]
    fn test_comments_single() {
        let module = grammar_tests::parse_ast("# hello\nx = 1");
        let comments = module.comments();
        assert_eq!(comments.len(), 1);
        assert_eq!(module.file_span(comments[0]).source_span(), "# hello");
    }

    #[test]
    fn test_comments_multiple() {
        let module = grammar_tests::parse_ast("# first\nx = 1\n# second");
        let comments = module.comments();
        assert_eq!(comments.len(), 2);
        assert_eq!(module.file_span(comments[0]).source_span(), "# first");
        assert_eq!(module.file_span(comments[1]).source_span(), "# second");
    }

    #[test]
    fn test_comments_inline() {
        let module = grammar_tests::parse_ast("x = 1 # inline");
        let comments = module.comments();
        assert_eq!(comments.len(), 1);
        assert_eq!(module.file_span(comments[0]).source_span(), "# inline");
    }

    #[test]
    fn test_comments_span() {
        let module = grammar_tests::parse_ast("# hi\nx = 1");
        let comments = module.comments();
        assert_eq!(comments.len(), 1);
        let resolved = module.file_span(comments[0]).resolve_span();
        assert_eq!(resolved.to_string(), "1:1-5");
    }

    #[test]
    fn test_comments_source_order() {
        let module = grammar_tests::parse_ast("# a\n# b\nx = 1\n# c");
        let file_spans: Vec<_> = module
            .comments()
            .iter()
            .map(|s| module.file_span(*s))
            .collect();
        let texts: Vec<&str> = file_spans.iter().map(|fs| fs.source_span()).collect();
        assert_eq!(texts, vec!["# a", "# b", "# c"]);
    }
}
