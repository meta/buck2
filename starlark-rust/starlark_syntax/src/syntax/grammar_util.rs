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

//! Code called by the parser to handle complex cases not handled by the grammar.

use crate::codemap::CodeMap;
use crate::codemap::Pos;
use crate::codemap::Span;
use crate::codemap::Spanned;
use crate::eval_exception::EvalException;
use crate::lexer::FStringConv as LexerFStringConv;
use crate::slice_vec_ext::VecExt;
use crate::syntax::DialectTypes;
use crate::syntax::ast::AssignIdentP;
use crate::syntax::ast::AssignOp;
use crate::syntax::ast::AssignP;
use crate::syntax::ast::AssignTarget;
use crate::syntax::ast::AssignTargetP;
use crate::syntax::ast::AstAssignTarget;
use crate::syntax::ast::AstExpr;
use crate::syntax::ast::AstFString;
use crate::syntax::ast::AstStmt;
use crate::syntax::ast::AstString;
use crate::syntax::ast::AstTypeExpr;
use crate::syntax::ast::Expr;
use crate::syntax::ast::FStringP;
use crate::syntax::ast::LoadP;
use crate::syntax::ast::Stmt;
use crate::syntax::ast::StmtP;
use crate::syntax::ast::ToAst;
use crate::syntax::ast::TypeExpr;
use crate::syntax::ast::TypeExprP;
use crate::syntax::state::ParserState;
use crate::syntax::type_expr::TypeExprUnpackP;

#[derive(Debug, thiserror::Error)]
enum GrammarUtilError {
    #[error("left-hand-side of assignment must take the form `a`, `a.b` or `a[b]`")]
    InvalidLhs,
    #[error("left-hand-side of modifying assignment cannot be a list or tuple")]
    InvalidModifyLhs,
    #[error("type annotations not allowed on augmented assignments")]
    TypeAnnotationOnAssignOp,
    #[error("type annotations not allowed on multiple assignments")]
    TypeAnnotationOnTupleAssign,
    #[error("`load` statement requires at least two arguments")]
    LoadRequiresAtLeastTwoArguments,
    #[error("unparenthesized tuple with trailing comma")]
    UnparenthesizedTupleTrailingComma,
}

/// Ensure we produce normalised Statements, rather than singleton Statements
pub fn statements(mut xs: Vec<AstStmt>, begin: usize, end: usize) -> AstStmt {
    if xs.len() == 1 {
        xs.pop().unwrap()
    } else {
        StmtP::Statements(xs).ast(begin, end)
    }
}

pub fn check_assign(codemap: &CodeMap, x: AstExpr) -> Result<AstAssignTarget, EvalException> {
    Ok(Spanned {
        span: x.span,
        node: match x.node {
            Expr::Tuple(xs) | Expr::List(xs) => {
                AssignTarget::Tuple(xs.into_try_map(|x| check_assign(codemap, x))?)
            }
            Expr::Dot(a, b) => AssignTarget::Dot(a, b),
            Expr::Index(a_b) => AssignTarget::Index(a_b),
            Expr::Identifier(x) => AssignTarget::Identifier(x.map(|s| AssignIdentP {
                ident: s.ident,
                payload: (),
            })),
            _ => {
                return Err(EvalException::new_anyhow(
                    GrammarUtilError::InvalidLhs.into(),
                    x.span,
                    codemap,
                ));
            }
        },
    })
}

pub fn check_assignment(
    codemap: &CodeMap,
    lhs: AstExpr,
    ty: Option<Box<AstTypeExpr>>,
    op: Option<AssignOp>,
    rhs: AstExpr,
) -> Result<Stmt, EvalException> {
    if op.is_some() {
        // for augmented assignment, Starlark doesn't allow tuple/list
        match &lhs.node {
            Expr::Tuple(_) | Expr::List(_) => {
                return Err(EvalException::new_anyhow(
                    GrammarUtilError::InvalidModifyLhs.into(),
                    lhs.span,
                    codemap,
                ));
            }
            _ => {}
        }
    }
    let lhs = check_assign(codemap, lhs)?;
    if let Some(ty) = &ty {
        let err = if op.is_some() {
            Some(GrammarUtilError::TypeAnnotationOnAssignOp)
        } else if matches!(lhs.node, AssignTargetP::Tuple(_)) {
            Some(GrammarUtilError::TypeAnnotationOnTupleAssign)
        } else {
            None
        };
        if let Some(err) = err {
            return Err(EvalException::new_anyhow(err.into(), ty.span, codemap));
        }
    }
    Ok(match op {
        None => Stmt::Assign(AssignP {
            lhs,
            ty: ty.map(|ty| *ty),
            rhs,
        }),
        Some(op) => Stmt::AssignModify(lhs, op, Box::new(rhs)),
    })
}

pub(crate) fn reject_unparenthesized_tuple_trailing_comma<T>(
    codemap: &CodeMap,
    begin: usize,
    end: usize,
) -> Result<T, EvalException> {
    Err(EvalException::new_anyhow(
        GrammarUtilError::UnparenthesizedTupleTrailingComma.into(),
        Span::new(Pos::new(begin as _), Pos::new(end as _)),
        codemap,
    ))
}

pub(crate) fn check_load_0(module: AstString, parser_state: &mut ParserState) -> Stmt {
    parser_state.errors.push(EvalException::new_anyhow(
        GrammarUtilError::LoadRequiresAtLeastTwoArguments.into(),
        module.span,
        parser_state.codemap,
    ));
    Stmt::Load(LoadP {
        module,
        args: Vec::new(),
        payload: (),
    })
}

/// An element of an f-string during parsing - either text or an expression.
pub enum FStringElement {
    /// Literal text in the f-string.
    Text(AstString),
    /// An expression with optional conversion specifier.
    Expr(AstExpr, LexerFStringConv),
}

/// Build an f-string from parsed parts (new parsing approach with proper expression support).
pub(crate) fn fstring_from_parts(
    parts: Vec<FStringElement>,
    begin: usize,
    end: usize,
    parser_state: &mut ParserState,
) -> AstFString {
    if !parser_state.dialect.enable_f_strings {
        parser_state.error(
            Span::new(Pos::new(begin as _), Pos::new(end as _)),
            "Your Starlark dialect must enable f-strings to use them",
        );
    }

    let mut format = String::new();
    let mut expressions = Vec::new();

    for part in parts {
        match part {
            FStringElement::Text(text) => {
                // Escape any literal braces in the text for the format string
                for c in text.node.chars() {
                    match c {
                        '{' => format.push_str("{{"),
                        '}' => format.push_str("}}"),
                        _ => format.push(c),
                    }
                }
            }
            FStringElement::Expr(expr, conv) => {
                expressions.push(expr);
                match conv {
                    LexerFStringConv::None | LexerFStringConv::Str => format.push_str("{}"),
                    LexerFStringConv::Repr => format.push_str("{!r}"),
                }
            }
        }
    }

    format.shrink_to_fit();

    FStringP {
        format: format.ast(begin, end),
        expressions,
    }
    .ast(begin, end)
}

#[derive(thiserror::Error, Debug)]
enum DialectError {
    #[error("type annotations are not allowed in this dialect")]
    Types,
}

fn err<T>(codemap: &CodeMap, span: Span, err: DialectError) -> Result<T, EvalException> {
    Err(EvalException::new_anyhow(err.into(), span, codemap))
}

pub(crate) fn dialect_check_type(
    state: &ParserState,
    x: Spanned<Expr>,
) -> Result<Spanned<TypeExpr>, EvalException> {
    let span = x.span;
    if state.dialect.enable_types == DialectTypes::Disable {
        return err(state.codemap, x.span, DialectError::Types);
    }

    TypeExprUnpackP::unpack(&x, state.codemap).map_err(EvalException::from)?;

    Ok(x.map(|node| TypeExprP {
        expr: Spanned { node, span },
        payload: (),
    }))
}

/// Create an error for an invalid f-string conversion specifier.
pub(crate) fn fstring_conv_error(l: usize, r: usize, state: &ParserState) -> EvalException {
    let span = Span::new(Pos::new(l as u32), Pos::new(r as u32));
    EvalException::parser_error(
        "invalid f-string conversion specifier, expected 's' or 'r'",
        span,
        state.codemap,
    )
}
