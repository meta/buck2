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

//! LALRPOP-backed [`Parser`] implementation.

use std::fmt::Write;

use lalrpop_util as lu;

use crate::codemap::Pos;
use crate::codemap::Span;
use crate::eval_exception::EvalException;
use crate::lexer::Token;
use crate::syntax::ast::AstStmt;
use crate::syntax::grammar::StarlarkParser;
use crate::syntax::parse_error::ParseError;
use crate::syntax::parser::Lexeme;
use crate::syntax::parser::Parser;
use crate::syntax::state::ParserState;

fn one_of(expected: &[String]) -> String {
    let mut result = String::new();
    for (i, e) in expected.iter().enumerate() {
        let sep = match i {
            0 => "one of",
            _ if i < expected.len() - 1 => ",",
            _ => " or",
        };
        write!(result, "{sep} {e}").unwrap();
    }
    result
}

/// Convert a LALRPOP parse error into our common [`ParseError`].
fn lalrpop_error_to_parse_error(
    err: lu::ParseError<usize, Token, EvalException>,
    eof_pos: usize,
) -> ParseError {
    match err {
        lu::ParseError::InvalidToken { location } => ParseError::Error {
            message: "Parse error: invalid token".to_owned(),
            span: Span::new(Pos::new(location as u32), Pos::new(location as u32)),
        },
        lu::ParseError::UnrecognizedToken {
            token: (x, t, y),
            expected,
        } => ParseError::Error {
            message: format!(
                "Parse error: unexpected {} here, expected {}",
                t,
                one_of(&expected)
            ),
            span: Span::new(Pos::new(x as u32), Pos::new(y as u32)),
        },
        lu::ParseError::UnrecognizedEof { .. } => ParseError::Error {
            message: "Parse error: unexpected end of file".to_owned(),
            span: Span::new(Pos::new(eof_pos as u32), Pos::new(eof_pos as u32)),
        },
        lu::ParseError::ExtraToken { token: (x, t, y) } => ParseError::Error {
            message: format!("Parse error: extraneous token {t}"),
            span: Span::new(Pos::new(x as u32), Pos::new(y as u32)),
        },
        lu::ParseError::User { error } => ParseError::EvalException(error),
    }
}

/// LALRPOP-backed parser.
pub(crate) struct LalrpopParser;

impl Parser for LalrpopParser {
    fn parse_module<I: Iterator<Item = Lexeme>>(
        state: &mut ParserState<'_>,
        tokens: I,
        eof_pos: usize,
    ) -> Result<AstStmt, ParseError> {
        StarlarkParser::new()
            .parse(state, tokens)
            .map_err(|e| lalrpop_error_to_parse_error(e, eof_pos))
    }
}

#[cfg(test)]
mod tests {
    use lalrpop_util as lu;

    use super::lalrpop_error_to_parse_error;
    use crate::codemap::CodeMap;
    use crate::lexer::Token;

    #[test]
    fn test_lalrpop_error_to_parse_error() {
        let codemap = CodeMap::new("test.bzl".to_owned(), "pass".to_owned());

        let assert_parse_error = |parse_error, eof_pos, want_message, want_span| {
            let err = lalrpop_error_to_parse_error(parse_error, eof_pos).into_crate_error(&codemap);
            assert_eq!(format!("{}", err.without_diagnostic()), want_message);
            assert_eq!(err.span().unwrap().to_string(), want_span);
        };

        assert_parse_error(
            lu::ParseError::InvalidToken { location: 2 },
            4,
            "Parse error: invalid token",
            "test.bzl:1:3",
        );
        assert_parse_error(
            lu::ParseError::UnrecognizedEof {
                location: 1,
                expected: vec![],
            },
            4,
            "Parse error: unexpected end of file",
            "test.bzl:1:5",
        );
        assert_parse_error(
            lu::ParseError::ExtraToken {
                token: (1, Token::ClosingRound, 2),
            },
            4,
            "Parse error: extraneous token symbol ')'",
            "test.bzl:1:2-3",
        );
    }
}
