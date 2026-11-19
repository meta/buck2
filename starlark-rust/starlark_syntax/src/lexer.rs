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

use std::char;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Display;

use logos::Logos;
use num_bigint::BigInt;
use num_bigint::BigUint;
use num_bigint::ParseBigIntError;
use num_bigint::Sign;
use num_traits::Num;
use thiserror::Error;

use crate::codemap::CodeMap;
use crate::codemap::Pos;
use crate::codemap::Span;
use crate::cursors::CursorBytes;
use crate::cursors::CursorChars;
use crate::dialect::Dialect;
use crate::eval_exception::EvalException;

#[derive(Error, Debug)]
pub enum LexemeError {
    #[error("Parse error: incorrect indentation")]
    Indentation,
    #[error("Parse error: invalid input `{0}`")]
    InvalidInput(String),
    #[error("Parse error: tabs are not allowed")]
    InvalidTab,
    #[error("Parse error: unfinished string literal")]
    UnfinishedStringLiteral,
    #[error("Parse error: invalid string escape sequence `{0}`")]
    InvalidEscapeSequence(String),
    #[error("Parse error: missing string escape sequence, only saw `\\`")]
    EmptyEscapeSequence,
    #[error("Parse error: cannot use reserved keyword `{0}`")]
    ReservedKeyword(String),
    #[error("Parse error: integer cannot have leading 0, got `{0}`")]
    StartsZero(String),
    #[error("Parse error: failed to parse integer: `{0}`")]
    IntParse(String),
    #[error("Comment span is computed incorrectly (internal error)")]
    CommentSpanComputedIncorrectly,
    #[error("Cannot parse `{0}` as an integer in base {1}")]
    CannotParse(String, u32),
    #[error("Parse error: f-string expression is missing closing `}}`")]
    UnfinishedFStringExpression,
}

impl From<LexemeError> for crate::error::Error {
    fn from(e: LexemeError) -> Self {
        crate::error::Error::new_kind(crate::error::ErrorKind::Parser(anyhow::Error::new(e)))
    }
}

type LexemeT<T> = Result<(usize, T, usize), EvalException>;
type Lexeme = LexemeT<Token>;

fn map_lexeme_t<T1, T2>(lexeme: LexemeT<T1>, f: impl FnOnce(T1) -> T2) -> LexemeT<T2> {
    lexeme.map(|(l, t, r)| (l, f(t), r))
}

/// State for tracking f-string parsing.
#[derive(Debug, Clone)]
struct FStringState {
    /// The quote style used for this f-string.
    quote: FStringQuote,
    /// Whether this is a raw f-string (fr"..." or rf"...").
    raw: bool,
    /// Depth of nested braces within the current expression (0 means we're in string mode).
    /// When > 0, we're lexing an expression and need to track nested `{}`.
    brace_depth: usize,
    /// Depth of nested parentheses within the current expression.
    paren_depth: usize,
    /// Depth of nested square brackets within the current expression.
    bracket_depth: usize,
}

pub struct Lexer<'a> {
    // Information for spans
    codemap: CodeMap,
    // Other info
    indent_levels: Vec<usize>,
    /// Lexemes that have been generated but not yet returned
    buffer: VecDeque<Lexeme>,
    parens: isize, // Number of parens we have seen
    lexer: logos::Lexer<'a, Token>,
    done: bool,
    /// Stack of f-string states (for nested f-strings).
    fstring_stack: Vec<FStringState>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str, _dialect: &Dialect, codemap: CodeMap) -> Self {
        let lexer = Token::lexer(input);
        let mut lexer2 = Self {
            codemap,
            // Aim to size all the buffers such that they never resize
            indent_levels: Vec::with_capacity(20),
            buffer: VecDeque::with_capacity(10),
            lexer,
            parens: 0,
            done: false,
            fstring_stack: Vec::new(),
        };
        if let Err(e) = lexer2.calculate_indent() {
            lexer2.buffer.push_back(Err(e));
        }
        lexer2
    }

    fn err_pos<T>(&self, msg: LexemeError, pos: usize) -> Result<T, EvalException> {
        self.err_span(msg, pos, pos)
    }

    fn err_span<T>(&self, msg: LexemeError, start: usize, end: usize) -> Result<T, EvalException> {
        Err(EvalException::new(
            msg.into(),
            Span::new(Pos::new(start as u32), Pos::new(end as u32)),
            &self.codemap,
        ))
    }

    fn err_now<T>(&self, msg: fn(String) -> LexemeError) -> Result<T, EvalException> {
        self.err_span(
            msg(self.lexer.slice().to_owned()),
            self.lexer.span().start,
            self.lexer.span().end,
        )
    }

    /// Comment tokens are produced by either logos for comments after code,
    /// or explicitly on lines which are only comments. This functions is used in the latter case.
    #[allow(clippy::manual_strip)]
    fn make_comment(&self, start: usize, end: usize) -> Lexeme {
        let comment = &self.codemap.source()[start..end];
        if !comment.starts_with('#') {
            return self.err_pos(LexemeError::CommentSpanComputedIncorrectly, start);
        }
        // Remove the `#`.
        let comment = &comment[1..];
        // Remove the trailing `\r` if it exists.
        // Note comments do not contain `\n`.
        if comment.ends_with('\r') {
            let end = end - 1;
            let comment = &comment[..comment.len() - 1];
            Ok((start, Token::Comment(comment.to_owned()), end))
        } else {
            Ok((start, Token::Comment(comment.to_owned()), end))
        }
    }

    /// We have just seen a newline, read how many indents we have
    /// and then set self.indent properly
    fn calculate_indent(&mut self) -> Result<(), EvalException> {
        // consume tabs and spaces, output the indentation levels
        let mut it = CursorBytes::new(self.lexer.remainder());
        let mut spaces = 0;
        let mut tabs = 0;
        let mut indent_start = self.lexer.span().end;
        loop {
            match it.next_char() {
                None => {
                    self.lexer.bump(it.pos());
                    return Ok(());
                }
                Some(' ') => {
                    spaces += 1;
                }
                Some('\t') => {
                    tabs += 1;
                }
                Some('\n') => {
                    // A line that is entirely blank gets emitted as a newline, and then
                    // we don't consume the subsequent newline character.
                    self.lexer.bump(it.pos() - 1);
                    return Ok(());
                }
                Some('\r') => {
                    // We just ignore these entirely
                }
                Some('#') => {
                    // A line that is all comments, only emits comment tokens.
                    // Skip until the next newline
                    // Remove skip now, so we can freely add it on later
                    spaces = 0;
                    tabs = 0;
                    let start = self.lexer.span().end + it.pos() - 1;
                    loop {
                        match it.next_char() {
                            None => {
                                let end = self.lexer.span().end + it.pos();
                                self.buffer.push_back(self.make_comment(start, end));
                                self.lexer.bump(it.pos());
                                return Ok(());
                            }
                            Some('\n') => break, // only the inner loop
                            Some(_) => {}
                        }
                    }
                    let end = self.lexer.span().end + it.pos() - 1;
                    self.buffer.push_back(self.make_comment(start, end));
                    indent_start = self.lexer.span().end + it.pos();
                }
                _ => break,
            }
        }
        self.lexer.bump(it.pos() - 1); // last character broke us out the loop
        let indent = spaces + tabs * 8;
        if tabs > 0 {
            return self.err_pos(LexemeError::InvalidTab, self.lexer.span().start);
        }
        let now = self.indent_levels.last().copied().unwrap_or(0);

        if indent > now {
            self.indent_levels.push(indent);
            let span = self.lexer.span();
            self.buffer
                .push_back(Ok((indent_start, Token::Indent, span.end)));
        } else if indent < now {
            let mut dedents = 1;
            self.indent_levels.pop().unwrap();
            loop {
                let now = self.indent_levels.last().copied().unwrap_or(0);
                if now == indent {
                    break;
                } else if now > indent {
                    dedents += 1;
                    self.indent_levels.pop().unwrap();
                } else {
                    let pos = self.lexer.span();
                    return self.err_span(LexemeError::Indentation, pos.start, pos.end);
                }
            }
            for _ in 0..dedents {
                // We must declare each dedent is only a position, so multiple adjacent dedents don't overlap
                self.buffer
                    .push_back(Ok((indent_start, Token::Dedent, indent_start)))
            }
        }
        Ok(())
    }

    fn wrap(&mut self, token: Token) -> Option<Lexeme> {
        let span = self.lexer.span();
        Some(Ok((span.start, token, span.end)))
    }

    // We've potentially seen one character, now consume between min and max elements of iterator
    // and treat it as an int in base radix
    fn escape_char(it: &mut CursorChars, min: usize, max: usize, radix: u32) -> Result<char, ()> {
        let mut value = 0u32;
        let mut count = 0;
        while count < max {
            match it.next() {
                None => {
                    if count >= min {
                        break;
                    } else {
                        return Err(());
                    }
                }
                Some(c) => match c.to_digit(radix) {
                    None => {
                        if count >= min {
                            it.unnext(c);
                            break;
                        } else {
                            return Err(());
                        }
                    }
                    Some(v) => {
                        count += 1;
                        value = (value * radix) + v;
                    }
                },
            }
        }
        char::from_u32(value).ok_or(())
    }

    // We have seen a '\' character, now parse what comes next
    fn escape(it: &mut CursorChars, res: &mut String) -> Result<(), ()> {
        match it.next() {
            Some('n') => res.push('\n'),
            Some('r') => res.push('\r'),
            Some('t') => res.push('\t'),
            Some('a') => res.push('\x07'),
            Some('b') => res.push('\x08'),
            Some('f') => res.push('\x0C'),
            Some('v') => res.push('\x0B'),
            Some('\n') => {}
            Some('\r') => {
                // Windows newline incoming, we expect a \n next, which we can ignore
                if it.next() != Some('\n') {
                    // A random \r character happened, let's declare an error, but we're just confused here
                    return Err(());
                }
            }
            Some('x') => res.push(Self::escape_char(it, 2, 2, 16)?),
            Some('u') => res.push(Self::escape_char(it, 4, 4, 16)?),
            Some('U') => res.push(Self::escape_char(it, 8, 8, 16)?),
            Some(c) => match c {
                '0'..='7' => {
                    it.unnext(c);
                    res.push(Self::escape_char(it, 1, 3, 8)?)
                }
                '"' | '\'' | '\\' => res.push(c),
                _ => {
                    res.push('\\');
                    res.push(c);
                }
            },
            None => {
                return Err(());
            }
        };
        Ok(())
    }

    /// Parse a String. Return the String, and the offset where it starts.
    // String parsing is a hot-spot, so parameterise by a `stop` function which gets
    // specialised for each variant
    fn string(
        &mut self,
        triple: bool,
        raw: bool,
        mut stop: impl FnMut(char) -> bool,
    ) -> LexemeT<(String, usize)> {
        // We have seen an opening quote, which is either ' or "
        // If triple is true, it was a triple quote
        // stop lets us know when a string ends.

        // Before the first quote character
        let string_start = self.lexer.span().start;
        // After the first quote character, but before any contents or it tracked stuff
        let mut string_end = self.lexer.span().end;

        let mut it = CursorBytes::new(self.lexer.remainder());
        let it2;

        if triple {
            it.next();
            it.next();
        }
        let contents_start = it.pos();

        // Take the fast path as long as the result is a slice of the original, with no changes.
        let mut res;
        loop {
            match it.next_char() {
                None => {
                    return self.err_span(
                        if self.in_fstring_expr_mode() {
                            LexemeError::UnfinishedFStringExpression
                        } else {
                            LexemeError::UnfinishedStringLiteral
                        },
                        string_start,
                        string_end + it.pos(),
                    );
                }
                Some(c) => {
                    if stop(c) {
                        let contents_end = it.pos() - if triple { 3 } else { 1 };
                        let contents = &self.lexer.remainder()[contents_start..contents_end];
                        self.lexer.bump(it.pos());
                        return Ok((
                            string_start,
                            (contents.to_owned(), contents_start),
                            string_end + it.pos(),
                        ));
                    } else if c == '\\' || c == '\r' || (c == '\n' && !triple) {
                        res = String::with_capacity(it.pos() + 10);
                        res.push_str(&self.lexer.remainder()[contents_start..it.pos() - 1]);
                        it2 = CursorChars::new_offset(self.lexer.remainder(), it.pos() - 1);
                        break;
                    }
                }
            }
        }

        // We bailed out of the fast path, that means we now accumulate character by character,
        // might have an error or be dealing with escape characters.
        let mut it = it2;
        while let Some(c) = it.next() {
            if stop(c) {
                self.lexer.bump(it.pos());
                if triple {
                    res.truncate(res.len() - 2);
                }
                return Ok((string_start, (res, contents_start), string_end + it.pos()));
            }
            match c {
                '\n' if !triple => {
                    // Will raise an error about out of chars.
                    // But don't include the final \n in the count.
                    string_end -= 1;
                    break;
                }
                '\r' => {
                    // We just ignore these in all modes
                }
                '\\' => {
                    if raw {
                        match it.next() {
                            Some(c) => {
                                if c != '\'' && c != '"' {
                                    res.push('\\');
                                }
                                res.push(c);
                            }
                            _ => break, // Out of chars
                        }
                    } else {
                        let pos = it.pos();
                        if Self::escape(&mut it, &mut res).is_err() {
                            let bad = self.lexer.remainder()[pos..it.pos()].to_owned();
                            return self.err_span(
                                if bad.is_empty() {
                                    LexemeError::EmptyEscapeSequence
                                } else {
                                    LexemeError::InvalidEscapeSequence(bad)
                                },
                                string_end + pos - 1,
                                string_end + it.pos(),
                            );
                        }
                    }
                }
                c => res.push(c),
            }
        }

        // We ran out of characters
        self.err_span(
            LexemeError::UnfinishedStringLiteral,
            string_start,
            string_end + it.pos(),
        )
    }

    /// Like `escape`, but appends bytes to `res: &mut Vec<u8>`.
    /// Non-ASCII chars are encoded as UTF-8. `\uXXXX` / `\UHHHHHHHH` also encode as UTF-8 bytes.
    fn escape_bytes(it: &mut CursorChars, res: &mut Vec<u8>) -> Result<(), ()> {
        match it.next() {
            Some('n') => res.push(b'\n'),
            Some('r') => res.push(b'\r'),
            Some('t') => res.push(b'\t'),
            Some('a') => res.push(b'\x07'),
            Some('b') => res.push(b'\x08'),
            Some('f') => res.push(b'\x0C'),
            Some('v') => res.push(b'\x0B'),
            Some('\n') => {}
            Some('\r') => {
                if it.next() != Some('\n') {
                    return Err(());
                }
            }
            Some('x') => {
                let c = Self::escape_char(it, 2, 2, 16)?;
                // \xHH: store the low byte directly (Latin-1 / bytes-literal semantics).
                res.push(c as u32 as u8);
            }
            Some('u') => {
                let c = Self::escape_char(it, 4, 4, 16)?;
                let mut buf = [0u8; 4];
                res.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            Some('U') => {
                let c = Self::escape_char(it, 8, 8, 16)?;
                let mut buf = [0u8; 4];
                res.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            Some(c) => match c {
                '0'..='7' => {
                    it.unnext(c);
                    let c = Self::escape_char(it, 1, 3, 8)?;
                    if c as u32 > 255 {
                        return Err(());
                    }
                    res.push(c as u32 as u8);
                }
                '"' | '\'' | '\\' => res.push(c as u8),
                _ => {
                    res.push(b'\\');
                    let mut buf = [0u8; 4];
                    res.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            },
            None => return Err(()),
        }
        Ok(())
    }

    /// Parse a bytes literal. Return the bytes content and the offset where it starts.
    fn bytes_string(
        &mut self,
        triple: bool,
        raw: bool,
        mut stop: impl FnMut(char) -> bool,
    ) -> LexemeT<(Vec<u8>, usize)> {
        let string_start = self.lexer.span().start;
        let mut string_end = self.lexer.span().end;

        let mut it = CursorBytes::new(self.lexer.remainder());
        let it2;

        if triple {
            it.next();
            it.next();
        }
        let contents_start = it.pos();

        // Fast path: accumulate bytes as-is until we hit something special
        let mut res: Vec<u8>;
        loop {
            match it.next_char() {
                None => {
                    return self.err_span(
                        LexemeError::UnfinishedStringLiteral,
                        string_start,
                        string_end + it.pos(),
                    );
                }
                Some(c) => {
                    if stop(c) {
                        let contents_end = it.pos() - if triple { 3 } else { 1 };
                        let contents =
                            &self.lexer.remainder().as_bytes()[contents_start..contents_end];
                        self.lexer.bump(it.pos());
                        return Ok((
                            string_start,
                            (contents.to_vec(), contents_start),
                            string_end + it.pos(),
                        ));
                    } else if c == '\\' || c == '\r' || (c == '\n' && !triple) {
                        res = Vec::with_capacity(it.pos() + 10);
                        res.extend_from_slice(
                            &self.lexer.remainder().as_bytes()[contents_start..it.pos() - 1],
                        );
                        it2 = CursorChars::new_offset(self.lexer.remainder(), it.pos() - 1);
                        break;
                    }
                }
            }
        }

        let mut it = it2;
        while let Some(c) = it.next() {
            if stop(c) {
                self.lexer.bump(it.pos());
                if triple {
                    // Remove the two extra quote chars already pushed
                    let len = res.len();
                    res.truncate(len.saturating_sub(2));
                }
                return Ok((string_start, (res, contents_start), string_end + it.pos()));
            }
            match c {
                '\n' if !triple => {
                    string_end -= 1;
                    break;
                }
                '\r' => {}
                '\\' => {
                    if raw {
                        match it.next() {
                            Some(c) => {
                                if c != '\'' && c != '"' {
                                    res.push(b'\\');
                                }
                                let mut buf = [0u8; 4];
                                res.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                            }
                            _ => break,
                        }
                    } else {
                        let pos = it.pos();
                        if Self::escape_bytes(&mut it, &mut res).is_err() {
                            let bad = self.lexer.remainder()[pos..it.pos()].to_owned();
                            return self.err_span(
                                if bad.is_empty() {
                                    LexemeError::EmptyEscapeSequence
                                } else {
                                    LexemeError::InvalidEscapeSequence(bad)
                                },
                                string_end + pos - 1,
                                string_end + it.pos(),
                            );
                        }
                    }
                }
                c => {
                    let mut buf = [0u8; 4];
                    res.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }

        self.err_span(
            LexemeError::UnfinishedStringLiteral,
            string_start,
            string_end + it.pos(),
        )
    }

    fn parse_double_quoted_bytes_string(&mut self, raw: bool) -> Option<LexemeT<(Vec<u8>, usize)>> {
        if self.lexer.remainder().starts_with("\"\"") {
            let mut qs = 0;
            Some(self.bytes_string(true, raw, |c| {
                if c == '\"' {
                    qs += 1;
                    qs == 3
                } else {
                    qs = 0;
                    false
                }
            }))
        } else {
            Some(self.bytes_string(false, raw, |c| c == '\"'))
        }
    }

    fn parse_single_quoted_bytes_string(&mut self, raw: bool) -> Option<LexemeT<(Vec<u8>, usize)>> {
        if self.lexer.remainder().starts_with("''") {
            let mut qs = 0;
            Some(self.bytes_string(true, raw, |c| {
                if c == '\'' {
                    qs += 1;
                    qs == 3
                } else {
                    qs = 0;
                    false
                }
            }))
        } else {
            Some(self.bytes_string(false, raw, |c| c == '\''))
        }
    }

    fn int(&self, s: &str, radix: u32) -> Lexeme {
        let span = self.lexer.span();
        match TokenInt::from_str_radix(s, radix) {
            Ok(i) => Ok((span.start, Token::Int(i), span.end)),
            Err(_) => self.err_now(LexemeError::IntParse),
        }
    }

    pub fn next(&mut self) -> Option<Lexeme> {
        loop {
            // Note that this function doesn't always return - a few branches use `continue`
            // to always go round the loop again.

            // If we're in f-string string mode, use specialized lexing
            if self.in_fstring_string_mode() {
                return self.lex_fstring_content();
            }

            return match self.buffer.pop_front() {
                Some(x) => Some(x),
                _ => {
                    if self.done {
                        None
                    } else {
                        // Check for f-string conversion specifier (!) before regular lexing
                        if self.at_fstring_expr_top_level() {
                            let remainder = self.lexer.remainder();
                            if remainder.starts_with('!') && !remainder.starts_with("!=") {
                                // Emit FStringBang token, let grammar handle the conversion
                                let start = self.lexer.span().end;
                                self.lexer.bump(1); // consume "!"
                                return Some(Ok((start, Token::FStringBang, start + 1)));
                            }
                        }

                        match self.lexer.next() {
                            None => {
                                self.done = true;
                                let pos = self.lexer.span().end;
                                for _ in 0..self.indent_levels.len() {
                                    self.buffer.push_back(Ok((pos, Token::Dedent, pos)))
                                }
                                self.indent_levels.clear();
                                self.wrap(Token::Newline)
                            }
                            Some(Ok(token)) => match token {
                                Token::Tabs => {
                                    self.buffer.push_back(
                                        self.err_pos(
                                            LexemeError::InvalidTab,
                                            self.lexer.span().start,
                                        ),
                                    );
                                    continue;
                                }
                                Token::Newline => {
                                    if self.parens == 0 && !self.in_fstring_expr_mode() {
                                        let span = self.lexer.span();
                                        if let Err(e) = self.calculate_indent() {
                                            return Some(Err(e));
                                        }
                                        Some(Ok((span.start, Token::Newline, span.end)))
                                    } else {
                                        continue;
                                    }
                                }
                                Token::Reserved => Some(self.err_now(LexemeError::ReservedKeyword)),
                                Token::RawDecInt => {
                                    let s = self.lexer.slice();
                                    if s.len() > 1 && &s[0..1] == "0" {
                                        return Some(self.err_now(LexemeError::StartsZero));
                                    }
                                    Some(self.int(s, 10))
                                }
                                Token::RawOctInt => {
                                    let s = self.lexer.slice();
                                    assert!(s.starts_with("0o") || s.starts_with("0O"));
                                    Some(self.int(&s[2..], 8))
                                }
                                Token::RawHexInt => {
                                    let s = self.lexer.slice();
                                    assert!(s.starts_with("0x") || s.starts_with("0X"));
                                    Some(self.int(&s[2..], 16))
                                }
                                Token::RawBinInt => {
                                    let s = self.lexer.slice();
                                    assert!(s.starts_with("0b") || s.starts_with("0B"));
                                    Some(self.int(&s[2..], 2))
                                }
                                Token::Int(..) => unreachable!("Lexer does not produce Int tokens"),
                                Token::RawDoubleQuote => {
                                    let raw = self.lexer.span().len() == 2;
                                    self.parse_double_quoted_string(raw).map(|lex| {
                                        map_lexeme_t(lex, |(s, _offset)| Token::String(s))
                                    })
                                }
                                Token::RawSingleQuote => {
                                    let raw = self.lexer.span().len() == 2;
                                    self.parse_single_quoted_string(raw).map(|lex| {
                                        map_lexeme_t(lex, |(s, _offset)| Token::String(s))
                                    })
                                }
                                Token::String(_) => {
                                    unreachable!("The lexer does not produce String")
                                }
                                Token::RawFStringDoubleQuote => {
                                    let span_len = self.lexer.span().len();
                                    let raw = span_len == 3;
                                    let is_triple = self.lexer.remainder().starts_with("\"\"");
                                    if is_triple {
                                        self.lexer.bump(2); // consume the extra quotes
                                    }
                                    let quote = if is_triple {
                                        FStringQuote::TripleDouble
                                    } else {
                                        FStringQuote::Double
                                    };
                                    Some(self.start_fstring(quote, raw))
                                }
                                Token::RawFStringSingleQuote => {
                                    let span_len = self.lexer.span().len();
                                    let raw = span_len == 3;
                                    let is_triple = self.lexer.remainder().starts_with("''");
                                    if is_triple {
                                        self.lexer.bump(2); // consume the extra quotes
                                    }
                                    let quote = if is_triple {
                                        FStringQuote::TripleSingle
                                    } else {
                                        FStringQuote::Single
                                    };
                                    Some(self.start_fstring(quote, raw))
                                }
                                Token::FStringStart(_)
                                | Token::FStringText(_)
                                | Token::FStringExprStart
                                | Token::FStringExprEnd
                                | Token::FStringBang
                                | Token::FStringEnd => {
                                    unreachable!("The lexer does not produce these tokens directly")
                                }
                                Token::OpeningCurly => {
                                    self.parens += 1;
                                    // Track nested braces in f-string expressions
                                    if self.in_fstring_expr_mode() {
                                        if let Some(state) = self.fstring_stack.last_mut() {
                                            state.brace_depth += 1;
                                        }
                                    }
                                    self.wrap(token)
                                }
                                Token::RawByteDoubleQuote => {
                                    // span len: b" = 2, br" = 3, rb" = 3
                                    let raw = self.lexer.span().len() >= 3;
                                    self.parse_double_quoted_bytes_string(raw).map(|lex| {
                                        map_lexeme_t(lex, |(b, _offset)| Token::Bytes(b))
                                    })
                                }
                                Token::RawByteSingleQuote => {
                                    // span len: b' = 2, br' = 3, rb' = 3
                                    let raw = self.lexer.span().len() >= 3;
                                    self.parse_single_quoted_bytes_string(raw).map(|lex| {
                                        map_lexeme_t(lex, |(b, _offset)| Token::Bytes(b))
                                    })
                                }
                                Token::Bytes(_) => {
                                    unreachable!("The lexer does not produce Bytes directly")
                                }
                                Token::OpeningRound => {
                                    self.parens += 1;
                                    self.track_fstring_paren(true);
                                    self.wrap(token)
                                }
                                Token::OpeningSquare => {
                                    self.parens += 1;
                                    self.track_fstring_bracket(true);
                                    self.wrap(token)
                                }
                                Token::ClosingCurly => {
                                    // Check if this closes an f-string expression
                                    if self.at_fstring_expr_top_level() {
                                        if let Some(state) = self.fstring_stack.last_mut() {
                                            state.brace_depth = 0;
                                        }
                                        let span = self.lexer.span();
                                        return Some(Ok((
                                            span.start,
                                            Token::FStringExprEnd,
                                            span.end,
                                        )));
                                    }
                                    // Nested brace or not in f-string
                                    if let Some(state) = self.fstring_stack.last_mut() {
                                        if state.brace_depth > 0 {
                                            state.brace_depth = state.brace_depth.saturating_sub(1);
                                        }
                                    }
                                    self.parens -= 1;
                                    self.wrap(token)
                                }
                                Token::BangEqual => {
                                    // In f-string expression mode, `!=` is just a comparison operator
                                    self.wrap(token)
                                }
                                Token::ClosingRound => {
                                    self.parens -= 1;
                                    self.track_fstring_paren(false);
                                    self.wrap(token)
                                }
                                Token::ClosingSquare => {
                                    self.parens -= 1;
                                    self.track_fstring_bracket(false);
                                    self.wrap(token)
                                }
                                _ => self.wrap(token),
                            },
                            Some(Err(_)) => Some(self.err_now(LexemeError::InvalidInput)),
                        }
                    }
                }
            };
        }
    }

    fn parse_double_quoted_string(&mut self, raw: bool) -> Option<LexemeT<(String, usize)>> {
        if self.lexer.remainder().starts_with("\"\"") {
            let mut qs = 0;
            Some(self.string(true, raw, |c| {
                if c == '\"' {
                    qs += 1;
                    qs == 3
                } else {
                    qs = 0;
                    false
                }
            }))
        } else {
            Some(self.string(false, raw, |c| c == '\"'))
        }
    }

    fn parse_single_quoted_string(&mut self, raw: bool) -> Option<LexemeT<(String, usize)>> {
        if self.lexer.remainder().starts_with("''") {
            let mut qs = 0;
            Some(self.string(true, raw, |c| {
                if c == '\'' {
                    qs += 1;
                    qs == 3
                } else {
                    qs = 0;
                    false
                }
            }))
        } else {
            Some(self.string(false, raw, |c| c == '\''))
        }
    }

    /// Check if we're currently inside an f-string and in string mode (not expression mode).
    fn in_fstring_string_mode(&self) -> bool {
        self.fstring_stack
            .last()
            .is_some_and(|s| s.brace_depth == 0)
    }

    /// Check if we're currently inside an f-string expression.
    fn in_fstring_expr_mode(&self) -> bool {
        self.fstring_stack.last().is_some_and(|s| s.brace_depth > 0)
    }

    /// Lex f-string content (text between `{}` or from start to first `{` or end).
    /// Returns the next token when in f-string string mode.
    fn lex_fstring_content(&mut self) -> Option<Lexeme> {
        let state = self.fstring_stack.last()?;
        let quote_char = state.quote.quote_char();
        let is_triple = state.quote.is_triple();
        let raw = state.raw;

        let start = self.lexer.span().end;
        let mut it = CursorChars::new_offset(self.lexer.remainder(), 0);
        let mut text = String::new();
        let mut quote_count = 0;

        loop {
            match it.next() {
                None => {
                    // Unterminated f-string
                    return Some(self.err_span(
                        LexemeError::UnfinishedStringLiteral,
                        start,
                        start + it.pos(),
                    ));
                }
                Some('{') => {
                    // Check for escaped brace `{{`
                    match it.next() {
                        Some('{') => {
                            text.push('{');
                            quote_count = 0;
                        }
                        next_char => {
                            // Put back whatever we consumed (if anything)
                            if let Some(c) = next_char {
                                it.unnext(c);
                            }
                            // Position is right after `{`, back up to before it
                            let brace_pos = it.pos() - 1;
                            self.lexer.bump(brace_pos);
                            return Some(self.emit_fstring_expr_start(start, text, brace_pos));
                        }
                    }
                }
                Some('}') => {
                    // Check for escaped brace `}}`
                    match it.next() {
                        Some('}') => {
                            text.push('}');
                            quote_count = 0;
                        }
                        next_char => {
                            // Put back whatever we consumed (if anything)
                            if let Some(c) = next_char {
                                it.unnext(c);
                            }
                            // Standalone `}` outside expression - error
                            let brace_pos = it.pos() - 1;
                            return Some(self.err_span(
                                LexemeError::InvalidInput("}".to_owned()),
                                start + brace_pos,
                                start + brace_pos + 1,
                            ));
                        }
                    }
                }
                Some(c) if c == quote_char => {
                    quote_count += 1;
                    if is_triple {
                        if quote_count == 3 {
                            // End of triple-quoted f-string
                            // Remove the last 2 quotes from text (we added them)
                            text.pop();
                            text.pop();
                            self.lexer.bump(it.pos());
                            let end = start + it.pos();
                            return Some(self.emit_fstring_end(start, text, end, 3));
                        } else {
                            text.push(c);
                        }
                    } else {
                        // End of single-quoted f-string
                        self.lexer.bump(it.pos());
                        let end = start + it.pos();
                        return Some(self.emit_fstring_end(start, text, end, 1));
                    }
                }
                Some('\\') if !raw => {
                    quote_count = 0;
                    // Handle escape sequences
                    if Self::escape(&mut it, &mut text).is_err() {
                        return Some(self.err_span(
                            LexemeError::InvalidEscapeSequence("\\".to_owned()),
                            start + it.pos() - 1,
                            start + it.pos(),
                        ));
                    }
                }
                Some('\\') if raw => {
                    quote_count = 0;
                    // In raw mode, backslash is literal except before quotes
                    match it.next() {
                        Some(c) if c == quote_char => {
                            text.push(c);
                        }
                        Some(c) => {
                            text.push('\\');
                            text.push(c);
                        }
                        None => {
                            text.push('\\');
                        }
                    }
                }
                Some('\n') if !is_triple => {
                    // Newline in non-triple string - error
                    return Some(self.err_span(
                        if self.in_fstring_expr_mode() {
                            LexemeError::UnfinishedFStringExpression
                        } else {
                            LexemeError::UnfinishedStringLiteral
                        },
                        start,
                        start + it.pos(),
                    ));
                }
                Some('\r') => {
                    // Ignore carriage return
                    quote_count = 0;
                }
                Some(c) => {
                    quote_count = 0;
                    text.push(c);
                }
            }
        }
    }

    /// Track nested delimiters in f-string expression mode.
    fn track_fstring_delimiter(&mut self, depth_fn: impl FnOnce(&mut FStringState)) {
        if let Some(state) = self.fstring_stack.last_mut() {
            if state.brace_depth > 0 {
                depth_fn(state);
            }
        }
    }

    /// Emit the end of an f-string, optionally with preceding text.
    /// `quote_len` is the number of quote characters (1 for single, 3 for triple).
    fn emit_fstring_end(
        &mut self,
        start: usize,
        text: String,
        end: usize,
        quote_len: usize,
    ) -> Lexeme {
        self.fstring_stack.pop();
        let quote_start = end - quote_len;
        if !text.is_empty() {
            self.buffer
                .push_back(Ok((quote_start, Token::FStringEnd, end)));
            Ok((start, Token::FStringText(text), quote_start))
        } else {
            Ok((quote_start, Token::FStringEnd, end))
        }
    }

    /// Emit the start of an f-string expression, optionally with preceding text.
    fn emit_fstring_expr_start(&mut self, start: usize, text: String, brace_pos: usize) -> Lexeme {
        // Enter expression mode
        if let Some(state) = self.fstring_stack.last_mut() {
            state.brace_depth = 1;
        }
        self.lexer.bump(1); // consume the `{`

        let brace_start = start + brace_pos;
        if !text.is_empty() {
            self.buffer
                .push_back(Ok((brace_start, Token::FStringExprStart, brace_start + 1)));
            Ok((start, Token::FStringText(text), brace_start))
        } else {
            Ok((brace_start, Token::FStringExprStart, brace_start + 1))
        }
    }

    /// Track parentheses in f-string expression mode.
    fn track_fstring_paren(&mut self, is_open: bool) {
        self.track_fstring_delimiter(|state| {
            if is_open {
                state.paren_depth += 1;
            } else {
                state.paren_depth = state.paren_depth.saturating_sub(1);
            }
        });
    }

    /// Track square brackets in f-string expression mode.
    fn track_fstring_bracket(&mut self, is_open: bool) {
        self.track_fstring_delimiter(|state| {
            if is_open {
                state.bracket_depth += 1;
            } else {
                state.bracket_depth = state.bracket_depth.saturating_sub(1);
            }
        });
    }

    /// Start an f-string. Returns the FStringStart token.
    fn start_fstring(&mut self, quote: FStringQuote, raw: bool) -> Lexeme {
        let span = self.lexer.span();
        self.fstring_stack.push(FStringState {
            quote,
            raw,
            brace_depth: 0,
            paren_depth: 0,
            bracket_depth: 0,
        });
        Ok((span.start, Token::FStringStart(quote), span.end))
    }

    /// Check if we're at the top level of an f-string expression (can accept conversion specifier).
    fn at_fstring_expr_top_level(&self) -> bool {
        self.fstring_stack
            .last()
            .is_some_and(|s| s.brace_depth == 1 && s.paren_depth == 0 && s.bracket_depth == 0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, derive_more::Display)]
pub enum TokenInt {
    I32(i32),
    /// Only if larger than `i32`.
    BigInt(BigInt),
}

impl TokenInt {
    pub fn from_str_radix(s: &str, base: u32) -> crate::Result<TokenInt> {
        if let Ok(i) = i32::from_str_radix(s, base) {
            Ok(TokenInt::I32(i))
        } else if s.contains('_') {
            // `BigInt::from_str_radix` skips `_`, which is not part of Starlark integer syntax.
            Err(LexemeError::CannotParse(s.to_owned(), base).into())
        } else {
            match big_int_from_str_radix(s, base) {
                Ok(i) => Ok(TokenInt::BigInt(i)),
                Err(_) => Err(LexemeError::CannotParse(s.to_owned(), base).into()),
            }
        }
    }
}

/// Number of digits up to which `big_int_from_str_radix` leaves parsing to `num-bigint`.
///
/// Splitting first computes a power of the base, so below about 12000 digits it is no faster.
const BIG_INT_SPLIT_DIGITS: usize = 8192;

/// `BigInt::from_str_radix`, in subquadratic time.
///
/// For a base that is not a power of two, `num-bigint` multiplies the whole number parsed so far
/// by the base once per word of digits, which is quadratic in the length: a few million decimal
/// digits take seconds. Longer strings of plain digits are split instead, each part parsed on its
/// own, and the parts joined with one multiplication by a power of the base, which `num-bigint`
/// does in subquadratic time.
fn big_int_from_str_radix(s: &str, base: u32) -> Result<BigInt, ParseBigIntError> {
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'-') => (Sign::Minus, &s[1..]),
        Some(b'+') => (Sign::Plus, &s[1..]),
        _ => (Sign::Plus, s),
    };
    if base.is_power_of_two()
        || digits.len() <= BIG_INT_SPLIT_DIGITS
        || !digits.bytes().all(|b| char::from(b).is_digit(base))
    {
        // Linear already, short, or not plain digits: `num-bigint` gives the result or the error.
        return BigInt::from_str_radix(s, base);
    }
    // `pows[k]` is `base` to the power `BIG_INT_SPLIT_DIGITS << k`.
    let mut pows = vec![BigUint::from(base).pow(BIG_INT_SPLIT_DIGITS as u32)];
    while BIG_INT_SPLIT_DIGITS << pows.len() < digits.len() {
        let last = &pows[pows.len() - 1];
        pows.push(last * last);
    }
    let magnitude = big_uint_from_digits(digits, base, &pows)?;
    Ok(BigInt::from_biguint(sign, magnitude))
}

/// Parse `digits`, which are all valid in `base` and at most
/// `BIG_INT_SPLIT_DIGITS << pows.len()` long.
fn big_uint_from_digits(
    digits: &str,
    base: u32,
    pows: &[BigUint],
) -> Result<BigUint, ParseBigIntError> {
    // Split off the largest `BIG_INT_SPLIT_DIGITS << k` low digits that leave some high digits.
    // The high part is then no longer than the low part, so both fit in `pows[..k]`.
    match (0..pows.len())
        .rev()
        .find(|&k| BIG_INT_SPLIT_DIGITS << k < digits.len())
    {
        None => BigUint::from_str_radix(digits, base),
        Some(k) => {
            let (high, low) = digits.split_at(digits.len() - (BIG_INT_SPLIT_DIGITS << k));
            let high = big_uint_from_digits(high, base, &pows[..k])?;
            let low = big_uint_from_digits(low, base, &pows[..k])?;
            Ok(high * &pows[k] + low)
        }
    }
}

/// Quote style for f-strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FStringQuote {
    /// Single quote: `f'...'`
    Single,
    /// Double quote: `f"..."`
    Double,
    /// Triple single quote: `f'''...'''`
    TripleSingle,
    /// Triple double quote: `f"""..."""`
    TripleDouble,
}

impl FStringQuote {
    /// Returns the quote character for this f-string style.
    fn quote_char(self) -> char {
        match self {
            FStringQuote::Single | FStringQuote::TripleSingle => '\'',
            FStringQuote::Double | FStringQuote::TripleDouble => '"',
        }
    }

    /// Returns whether this is a triple-quoted f-string.
    fn is_triple(self) -> bool {
        matches!(
            self,
            FStringQuote::TripleSingle | FStringQuote::TripleDouble
        )
    }
}

/// Conversion specifier for f-string expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FStringConv {
    /// No conversion (default): `{expr}`
    #[default]
    None,
    /// String conversion: `{expr!s}`
    Str,
    /// Repr conversion: `{expr!r}`
    Repr,
}

/// All token that can be generated by the lexer
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r" +")] // whitespace
#[logos(skip r"\\\n")] // Escaped newline
#[logos(skip r"\\\r\n")] // Escaped newline (Windows line ending)
pub enum Token {
    /// Comment as token.
    /// Span includes the leading `#`, but the content does not.
    #[regex(r#"#[^\r\n]*"#, |lex| lex.slice()[1..].to_owned(), allow_greedy = true)]
    Comment(String),

    #[regex("\t+")] // Tabs (might be an error)
    Tabs,

    // Indentation block & meaningful spaces
    Indent, // New indentation block
    Dedent, // Leaving an indentation block
    #[regex(r"(\r)?\n")]
    Newline, // Newline outside a string

    // Some things the lexer can't deal with well, so we step in and generate
    // things ourselves
    #[token("'")]
    #[token("r'")]
    RawSingleQuote,
    #[token("\"")]
    #[token("r\"")]
    RawDoubleQuote,

    /// The start of a single-quoted f-string.
    #[token("f'")]
    #[token("fr'")]
    RawFStringSingleQuote,
    /// The start of a double-quoted f-string.
    #[token("f\"")]
    #[token("fr\"")]
    RawFStringDoubleQuote,

    /// The start of a single-quoted bytes literal.
    #[token("b'")]
    #[token("br'")]
    #[token("rb'")]
    RawByteSingleQuote,
    /// The start of a double-quoted bytes literal.
    #[token("b\"")]
    #[token("br\"")]
    #[token("rb\"")]
    RawByteDoubleQuote,

    Bytes(Vec<u8>), // A bytes literal

    #[regex(
        "as|\
        assert|\
        async|\
        await|\
        class|\
        del|\
        except|\
        finally|\
        from|\
        global|\
        import|\
        is|\
        nonlocal|\
        raise|\
        try|\
        while|\
        with|\
        yield"
    )]
    Reserved, // One of the reserved keywords

    #[regex(
        "[a-zA-Z_][a-zA-Z0-9_]*"
    , |lex| lex.slice().to_owned())]
    Identifier(String), // An identifier

    #[regex("[0-9]+")]
    RawDecInt,
    #[regex("0[xX][A-Fa-f0-9]+")]
    RawHexInt,
    #[regex("0[bB][01]+")]
    RawBinInt,
    #[regex("0[oO][0-7]+")]
    RawOctInt,

    Int(TokenInt), // An integer literal (123, 0x1, 0b1011, 0o755, ...)

    // Returns closest f64. https://doc.rust-lang.org/std/primitive.f64.html#method.from_str
    #[regex("[0-9]+\\.[0-9]*([eE][-+]?[0-9]+)?", |lex| lex.slice().parse::<f64>().ok())]
    #[regex("[0-9]+[eE][-+]?[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    #[regex("\\.[0-9]+([eE][-+]?[0-9]+)?", |lex| lex.slice().parse::<f64>().ok())]
    Float(f64), // A float literal (3.14, .3, 1e6, 0.)

    String(String), // A string literal

    // F-string tokens for proper expression parsing
    /// Start of an f-string (the `f"` or `f'` prefix). Contains info about quote style.
    FStringStart(FStringQuote),
    /// A text chunk within an f-string (between `{}`).
    FStringText(String),
    /// The `{` that starts an expression within an f-string.
    FStringExprStart,
    /// The `}` that ends an expression within an f-string.
    FStringExprEnd,
    /// The `!` before a conversion specifier in an f-string expression.
    FStringBang,
    /// End of an f-string (the closing quote).
    FStringEnd,

    // Keywords
    #[token("and")]
    And,
    #[token("break")]
    Break,
    #[token("continue")]
    Continue,
    #[token("def")]
    Def,
    #[token("elif")]
    Elif,
    #[token("else")]
    Else,
    #[token("for")]
    For,
    #[token("if")]
    If,
    #[token("in")]
    In,
    #[token("lambda")]
    Lambda,
    #[token("load")]
    Load,
    #[token("not")]
    Not,
    #[token("or")]
    Or,
    #[token("pass")]
    Pass,
    #[token("return")]
    Return,
    // Symbols
    #[token(",")]
    Comma,
    #[token(";")]
    Semicolon,
    #[token(":")]
    Colon,
    #[token("+=")]
    PlusEqual,
    #[token("-=")]
    MinusEqual,
    #[token("*=")]
    StarEqual,
    #[token("/=")]
    SlashEqual,
    #[token("//=")]
    SlashSlashEqual,
    #[token("%=")]
    PercentEqual,
    #[token("==")]
    EqualEqual,
    #[token("!=")]
    BangEqual,
    #[token("<=")]
    LessEqual,
    #[token(">=")]
    GreaterEqual,
    #[token("**")]
    StarStar,
    #[token("->")]
    MinusGreater,
    #[token("=")]
    Equal,
    #[token("<")]
    LessThan,
    #[token(">")]
    GreaterThan,
    #[token("-")]
    Minus,
    #[token("+")]
    Plus,
    #[token("*")]
    Star,
    #[token("%")]
    Percent,
    #[token("/")]
    Slash,
    #[token("//")]
    SlashSlash,
    #[token(".")]
    Dot,
    #[token("&")]
    Ampersand,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("<<")]
    LessLess,
    #[token(">>")]
    GreaterGreater,
    #[token("~")]
    Tilde,
    #[token("&=")]
    AmpersandEqual,
    #[token("|=")]
    PipeEqual,
    #[token("^=")]
    CaretEqual,
    #[token("<<=")]
    LessLessEqual,
    #[token(">>=")]
    GreaterGreaterEqual,
    #[token("...")]
    Ellipsis,

    // Brackets
    #[token("[")]
    OpeningSquare,
    #[token("{")]
    OpeningCurly,
    #[token("(")]
    OpeningRound,
    #[token("]")]
    ClosingSquare,
    #[token("}")]
    ClosingCurly,
    #[token(")")]
    ClosingRound,
}

impl Token {
    /// Used for testing
    #[cfg(test)]
    pub fn unlex(&self) -> String {
        match self {
            Token::Indent => "\t".to_owned(),
            Token::Newline => "\n".to_owned(),
            Token::Dedent => "#dedent".to_owned(),
            Token::String(x) => {
                // The Rust {:?} is unstable, so changes between versions,
                // instead use the JSON standard for string escapes.
                // Reuse the StarlarkValue implementation since it's close to hand.
                serde_json::to_string(x).unwrap()
            }
            _ => {
                let s = self.to_string();
                // Out display is often: keyword 'lambda'
                // so strip out the bit in single quotes
                let first = s.find('\'');
                match first {
                    Some(first) if s.ends_with('\'') && first != s.len() - 1 => {
                        s[first + 1..s.len() - 1].to_owned()
                    }
                    _ => s,
                }
            }
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Indent => write!(f, "new indentation block"),
            Token::Dedent => write!(f, "end of indentation block"),
            Token::Newline => write!(f, "new line"),
            Token::And => write!(f, "keyword 'and'"),
            Token::Else => write!(f, "keyword 'else'"),
            Token::Load => write!(f, "keyword 'load'"),
            Token::Break => write!(f, "keyword 'break'"),
            Token::For => write!(f, "keyword 'for'"),
            Token::Not => write!(f, "keyword 'not'"),
            Token::Continue => write!(f, "keyword 'continue'"),
            Token::If => write!(f, "keyword 'if'"),
            Token::Or => write!(f, "keyword 'or'"),
            Token::Def => write!(f, "keyword 'def'"),
            Token::In => write!(f, "keyword 'in'"),
            Token::Pass => write!(f, "keyword 'pass'"),
            Token::Elif => write!(f, "keyword 'elif'"),
            Token::Return => write!(f, "keyword 'return'"),
            Token::Lambda => write!(f, "keyword 'lambda'"),
            Token::Comma => write!(f, "symbol ','"),
            Token::Semicolon => write!(f, "symbol ';'"),
            Token::Colon => write!(f, "symbol ':'"),
            Token::PlusEqual => write!(f, "symbol '+='"),
            Token::MinusEqual => write!(f, "symbol '-='"),
            Token::StarEqual => write!(f, "symbol '*='"),
            Token::SlashEqual => write!(f, "symbol '/='"),
            Token::SlashSlashEqual => write!(f, "symbol '//='"),
            Token::PercentEqual => write!(f, "symbol '%='"),
            Token::EqualEqual => write!(f, "symbol '=='"),
            Token::BangEqual => write!(f, "symbol '!='"),
            Token::LessEqual => write!(f, "symbol '<='"),
            Token::GreaterEqual => write!(f, "symbol '>='"),
            Token::StarStar => write!(f, "symbol '**'"),
            Token::MinusGreater => write!(f, "symbol '->'"),
            Token::Equal => write!(f, "symbol '='"),
            Token::LessThan => write!(f, "symbol '<'"),
            Token::GreaterThan => write!(f, "symbol '>'"),
            Token::Minus => write!(f, "symbol '-'"),
            Token::Plus => write!(f, "symbol '+'"),
            Token::Star => write!(f, "symbol '*'"),
            Token::Percent => write!(f, "symbol '%'"),
            Token::Slash => write!(f, "symbol '/'"),
            Token::SlashSlash => write!(f, "symbol '//'"),
            Token::Dot => write!(f, "symbol '.'"),
            Token::Ampersand => write!(f, "symbol '&'"),
            Token::Pipe => write!(f, "symbol '|'"),
            Token::Caret => write!(f, "symbol '^'"),
            Token::LessLess => write!(f, "symbol '<<'"),
            Token::GreaterGreater => write!(f, "symbol '>>'"),
            Token::Tilde => write!(f, "symbol '~'"),
            Token::AmpersandEqual => write!(f, "symbol '&='"),
            Token::PipeEqual => write!(f, "symbol '|='"),
            Token::CaretEqual => write!(f, "symbol '^='"),
            Token::LessLessEqual => write!(f, "symbol '<<='"),
            Token::GreaterGreaterEqual => write!(f, "symbol '>>='"),
            Token::Ellipsis => write!(f, "symbol '...'"),
            Token::OpeningSquare => write!(f, "symbol '['"),
            Token::OpeningCurly => write!(f, "symbol '{{'"),
            Token::OpeningRound => write!(f, "symbol '('"),
            Token::ClosingSquare => write!(f, "symbol ']'"),
            Token::ClosingCurly => write!(f, "symbol '}}'"),
            Token::ClosingRound => write!(f, "symbol ')'"),
            Token::Reserved => write!(f, "reserved keyword"),
            Token::Identifier(s) => write!(f, "identifier '{s}'"),
            Token::Int(i) => write!(f, "integer literal '{i}'"),
            Token::RawDecInt => write!(f, "decimal integer literal"),
            Token::RawHexInt => write!(f, "hexadecimal integer literal"),
            Token::RawOctInt => write!(f, "octal integer literal"),
            Token::RawBinInt => write!(f, "binary integer literal"),
            Token::Float(n) => write!(f, "float literal '{n}'"),
            Token::String(s) => write!(f, "string literal {s:?}"),
            Token::RawSingleQuote => write!(f, "starting '"),
            Token::RawDoubleQuote => write!(f, "starting \""),
            Token::RawFStringDoubleQuote => write!(f, "starting f'"),
            Token::RawFStringSingleQuote => write!(f, "starting f\""),
            Token::FStringStart(_) => write!(f, "f-string start"),
            Token::FStringText(s) => write!(f, "f-string text {:?}", s),
            Token::FStringExprStart => write!(f, "f-string expression start '{{'"),
            Token::FStringExprEnd => write!(f, "f-string expression end '}}'"),
            Token::FStringBang => write!(f, "f-string bang '!'"),
            Token::FStringEnd => write!(f, "f-string end"),
            Token::RawByteDoubleQuote => write!(f, "starting b\""),
            Token::RawByteSingleQuote => write!(f, "starting b'"),
            Token::Bytes(b) => write!(f, "bytes literal ({} bytes)", b.len()),
            Token::Comment(c) => write!(f, "comment '{c}'"),
            Token::Tabs => Ok(()),
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Lexeme;

    fn next(&mut self) -> Option<Self::Item> {
        self.next()
    }
}

pub fn lex_exactly_one_identifier(s: &str) -> Option<String> {
    let mut lexer = Token::lexer(s);
    match (lexer.next(), lexer.next()) {
        (Some(Ok(Token::Identifier(ident))), None) => Some(ident),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;
    use num_traits::Num;

    use crate::codemap::CodeMap;
    use crate::dialect::Dialect;
    use crate::lexer::BIG_INT_SPLIT_DIGITS;
    use crate::lexer::Lexer;
    use crate::lexer::TokenInt;
    use crate::lexer::lex_exactly_one_identifier;

    #[test]
    fn test_token_int_from_str_radix_long() {
        // Lengths on either side of where the digits are split, in bases that get split.
        let n = BIG_INT_SPLIT_DIGITS;
        let mut seed = 1u64;
        for base in [10, 3, 36] {
            for len in [n, n + 1, 2 * n, 2 * n + 1, 3 * n + 5] {
                let digits: String = (0..len)
                    .map(|i| {
                        seed = seed
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        let d = if i == 0 {
                            1
                        } else {
                            (seed >> 33) % u64::from(base)
                        };
                        char::from_digit(d as u32, base).unwrap()
                    })
                    .collect();
                for s in [digits.clone(), format!("-{digits}"), format!("+{digits}")] {
                    assert_eq!(
                        TokenInt::BigInt(BigInt::from_str_radix(&s, base).unwrap()),
                        TokenInt::from_str_radix(&s, base).unwrap(),
                        "base {base}, {len} digits",
                    );
                }
            }
        }

        // A long string that is not all digits is still rejected.
        let digits = "7".repeat(2 * n + 1);
        for s in [
            format!("{digits}a"),
            format!("{digits}+{digits}"),
            format!("{digits}-{digits}"),
            format!("{digits}_7"),
            format!("--{digits}"),
            format!(" {digits}"),
        ] {
            assert!(TokenInt::from_str_radix(&s, 10).is_err());
        }
    }

    #[test]
    fn test_is_valid_identifier() {
        assert_eq!(lex_exactly_one_identifier("foo").as_deref(), Some("foo"));
        assert_eq!(lex_exactly_one_identifier(" foo ").as_deref(), Some("foo"));
        assert_eq!(lex_exactly_one_identifier("foo bar"), None);
        assert_eq!(lex_exactly_one_identifier("not"), None);
        assert_eq!(lex_exactly_one_identifier("123"), None);
    }

    #[test]
    fn test_bytes_literal_octal_overflow_is_error() {
        // \400 = 256 in octal, which exceeds 255 and should be a lex error.
        let result: Vec<_> =
            Lexer::new(r#"b"\400""#, &Dialect::Standard, CodeMap::default()).collect();
        assert!(
            result.iter().any(|r| r.is_err()),
            "expected a lex error for b\"\\400\" but got: {:?}",
            result
        );
    }
}
