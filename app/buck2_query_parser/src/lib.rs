/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Parsing query expressions.
//!
//! The parser doesn't do any validation on the function names or function arguments, that is handled
//! during evaluation. The returned parsed tree has locations attached so that the evaluation can
//! produce good error messages.
//!
//! The grammar is something like:
//!
//! ```text
//! # note that set's args are space-separated, not comma-separated and so cannot be treated as a function
//! EXPR ::=
//!          NONE
//!        | WORD
//!        | INTEGER
//!        | '(' EXPR ')'
//!        | 'set(' WORD * ')'
//!        | FUNCTION_NAME '(' EXPR ( ',' EXPR ) * ')'
//!        | EXPR 'intersect' EXPR
//!        | EXPR ' ^ ' EXPR
//!        | EXPR ' union ' EXPR
//!        | EXPR ' + ' EXPR
//!        | EXPR ' except ' EXPR
//!        | EXPR ' - ' EXPR
//!
//! # word is much broader than a normal identifier-like thing would allow since we don't want to require
//! # quoting targets "@fbcode//some:target" or common regexes ".*" or filenames "Foo.java".
//! WORD ::=
//!          '\'' not_single_quote * '\''
//!        | '"' not_double_quote * '"'
//!        | "a-zA-Z0-9*/@._:$#%-" *
//!
//! INTEGER ::= "0" | ("1-9" "0-9"*)
//!
//! FUNCTION_NAME ::= "a-zA-Z_" "a-zA-Z0-9_" *
//! ```

pub mod multi_query;
pub mod placeholder;
pub mod span;
pub mod spanned;

use std::fmt::Display;

use dupe::Dupe;
use enum_map::Enum;
use nom::IResult;
use nom::Parser as _;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::bytes::complete::take_till;
use nom::bytes::complete::take_while1;
use nom::character::complete::alpha1;
use nom::character::complete::alphanumeric1;
use nom::character::complete::char;
use nom::character::complete::digit1;
use nom::character::complete::multispace0;
use nom::character::complete::multispace1;
use nom::combinator::all_consuming;
use nom::combinator::cut;
use nom::combinator::recognize;
use nom::error::ErrorKind;
use nom::error::context;
use nom::multi::many0;
use nom::multi::many1;
use nom::multi::separated_list0;
use nom::sequence::delimited;
use nom::sequence::pair;
use nom::sequence::preceded;
use nom::sequence::terminated;
use nom_language::error::VerboseError;
use nom_language::error::VerboseErrorKind;
use variant_name::VariantName;

use crate::span::Span;
use crate::spanned::Spanned;

// TODO(cjhopman): Add `LET WORD = expr IN expr`

// TODO(cjhopman): We should switch to our own error type here. VerboseError doesn't even allow us to construct
// our own error messages (so, for example, we can't have a good error message for too large integers) and doesn't
// support propagating anyhow or std errors (and since we can't do a custom message, we can't even capture them as a string).
type NomResult<'a, O, E> = IResult<Span<'a>, O, E>;

// Type alias.
trait NomParseError<'a>:
    nom::error::ParseError<Span<'a>> + nom::error::ContextError<Span<'a>> + 'a
{
}

impl<'a, E: nom::error::ParseError<Span<'a>> + nom::error::ContextError<Span<'a>> + 'a>
    NomParseError<'a> for E
{
}

#[derive(Debug, buck2_error::Error)]
enum ParseError {
    #[error("{0}")]
    #[buck2(input)]
    NomError(String),
}

/// This is the main output type of the query parser.
pub type SpannedExpr<'a> = Spanned<Expr<'a>>;

#[derive(Debug, VariantName)]
pub enum Expr<'a> {
    None,
    // doesn't need to be Span since the Expr itself is always a SpannedExpr.
    String(&'a str),
    Integer(u64),
    Function {
        function_name: Span<'a>,
        args: Vec<SpannedExpr<'a>>,
    },
    BinaryOpSequence(Box<SpannedExpr<'a>>, Vec<(BinaryOp, SpannedExpr<'a>)>),
    Set(Vec<Span<'a>>),
    FileSet(Vec<Span<'a>>),
}

impl Display for Expr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::None => write!(f, "None")?,
            Expr::String(word) => {
                let quote = if word.contains('\'') { '"' } else { '\'' };
                // The grammar shouldn't allow a word that contains both quote-types, but
                // this assert should help ensure this gets updated if the grammar changes
                // to allow that.
                assert!(!word.contains(quote));
                write!(f, "{quote}{word}{quote}")?;
            }
            Expr::Integer(val) => write!(f, "{val}")?,
            Expr::Function {
                function_name,
                args,
            } => {
                f.write_str(function_name.fragment())?;
                f.write_str("(")?;
                for (i, v) in args.iter().enumerate() {
                    if i != 0 {
                        f.write_str(", ")?;
                    }
                    v.fmt(f)?;
                }
                f.write_str(")")?;
            }
            Expr::BinaryOpSequence(left, exprs) => {
                for _ in exprs {
                    f.write_str("(")?;
                }
                write!(f, " {left}")?;
                for e in exprs {
                    write!(f, " {} {})", e.0, e.1)?;
                }
            }
            Expr::Set(vals) => {
                f.write_str("set(")?;
                for (i, v) in vals.iter().enumerate() {
                    if i != 0 {
                        f.write_str(" ")?;
                    }
                    f.write_str(v.fragment())?;
                }
                f.write_str(")")?;
            }
            Expr::FileSet(vals) => {
                f.write_str("fileset(")?;
                for (i, v) in vals.iter().enumerate() {
                    if i != 0 {
                        f.write_str(" ")?;
                    }
                    f.write_str(v.fragment())?;
                }
                f.write_str(")")?;
            }
        }
        Ok(())
    }
}

const NONE: &str = "None";
const INTERSECT: &str = "^";
const EXCEPT: &str = "-";
const UNION: &str = "+";

#[derive(Debug, Enum, Copy, Dupe, Clone)]
pub enum BinaryOp {
    Intersect,
    Except,
    Union,
}

impl Display for BinaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BinaryOp::Intersect => INTERSECT,
            BinaryOp::Except => EXCEPT,
            BinaryOp::Union => UNION,
        })
    }
}

/// Wraps a parser producing `O` with one producing `Spanned<O>` by marking the output with a span covering all of the consumed input.
fn spanned<'a, O, E, F>(mut func: F) -> impl FnMut(Span<'a>) -> IResult<Span<'a>, Spanned<O>, E>
where
    O: 'a,
    E: NomParseError<'a>,
    F: FnMut(Span<'a>) -> IResult<Span<'a>, O, E> + 'a,
{
    move |original_input| {
        let start = original_input.location_offset();
        let (input, value) = func(original_input)?;
        let end = input.location_offset();
        Ok((
            input,
            Spanned {
                position: start..end,
                value,
            },
        ))
    }
}

/// The line of `input` that contains `offset`, and below it a caret that points at `offset`. Of a
/// long line, only the part around `offset` is shown.
fn excerpt_at(input: &str, offset: usize) -> String {
    const CONTEXT_LEN: usize = 100;
    const OMITTED: &str = "<<omitted>>";

    let (before, after) = input.split_at(offset);
    let before = before.rfind('\n').map_or(before, |i| &before[i + 1..]);
    let after = after.find('\n').map_or(after, |i| &after[..i]).trim_end();

    let before_start = before.ceil_char_boundary(before.len().saturating_sub(CONTEXT_LEN));
    let after_end = after.floor_char_boundary(CONTEXT_LEN);
    let omitted_before = if before_start > 0 { OMITTED } else { "" };
    let omitted_after = if after_end < after.len() { OMITTED } else { "" };
    let before = &before[before_start..];
    let after = &after[..after_end];

    let caret_indent = " ".repeat(omitted_before.len() + before.chars().count());
    format!("{omitted_before}{before}{after}{omitted_after}\n{caret_indent}^\n")
}

/// Renders a parse error in the format of `nom_language::error::convert_error`.
fn render_error(input: &str, err: &VerboseError<Span<'_>>) -> String {
    // `convert_error` itself is not usable on queries, which can be megabytes long: it shows the
    // whole line that an error is on, and it panics on an error that is more than `u16::MAX`
    // bytes into its line.
    err.errors
        .iter()
        .enumerate()
        .map(|(i, (span, kind))| {
            if input.is_empty() {
                return match kind {
                    VerboseErrorKind::Char(c) => {
                        format!("{i}: expected '{c}', got empty input\n\n")
                    }
                    VerboseErrorKind::Context(s) => format!("{i}: in {s}, got empty input\n\n"),
                    VerboseErrorKind::Nom(e) => format!("{i}: in {e:?}, got empty input\n\n"),
                };
            }

            let offset = span.location_offset();
            let line_number = input[..offset].matches('\n').count() + 1;
            let excerpt = excerpt_at(input, offset);
            match kind {
                VerboseErrorKind::Char(c) => match span.fragment().chars().next() {
                    Some(actual) => format!(
                        "{i}: at line {line_number}:\n{excerpt}expected '{c}', found {actual}\n\n"
                    ),
                    None => format!(
                        "{i}: at line {line_number}:\n{excerpt}expected '{c}', got end of input\n\n"
                    ),
                },
                VerboseErrorKind::Context(s) => {
                    format!("{i}: at line {line_number}, in {s}:\n{excerpt}\n")
                }
                VerboseErrorKind::Nom(e) => {
                    format!("{i}: at line {line_number}, in {e:?}:\n{excerpt}\n")
                }
            }
        })
        .collect()
}

/// Parses a query string into a SpannedExpr. Requires that the entire input is consumed.
pub fn parse_expr(input: &str) -> buck2_error::Result<SpannedExpr<'_>> {
    let span = Span::new(input);
    // Parse with fast error (`()`) first,
    // and on error reparse again with `VerboseError` to get detailed errors.
    match all_consuming(expr).parse(span) {
        Ok((_, value)) => Ok(value),
        Err(nom::Err::Failure(())) | Err(nom::Err::Error(())) => {
            match all_consuming(expr).parse(span) {
                Ok(..) => unreachable!(
                    "if fast parse didn't succeed, slow parse should not succeed as well"
                ),
                Err(nom::Err::Failure(err)) | Err(nom::Err::Error(err)) => {
                    // TODO(cjhopman): Do better error message conversion. The nom format is rather verbose and references line
                    // numbers (which generally are meaningless in this context where we have a single line). It'd also be nice if we
                    // could keep the error information rather than just stringifying it here.
                    Err(ParseError::NomError(render_error(input, &err)).into())
                }
                Err(nom::Err::Incomplete(..)) => unreachable!(),
            }
        }
        Err(nom::Err::Incomplete(..)) => unreachable!(),
    }
}

// Parses a non-infix op expression. This is split out so that we can parse a sequence of infix operators without recursion.
fn single_expr<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    // The ordering here is a little important, the first three of these all have a pattern of identifying
    // some prefix (ex, expr_set will identify "set(") and then failing if it doesn't match.
    //
    // The important bit there is probably around set/function/word since we don't want to, for example, parse out a
    //  word from "set(" (we'd only then get an error by the caller requiring that we consumed everything).
    //
    // The infix binary operators require left-recursion, so we can't just handle that like the others. Instead we
    // parse an expression from the beginning of the input and check after if there's a "trailing" infix operator.
    let (input, left_expr) = alt((
        preceded(char('('), cut(terminated(expr, char(')')))),
        expr_set,
        expr_fileset,
        expr_function,
        expr_int,
        expr_word,
    ))
    .parse(input)?;

    Ok((input, left_expr))
}

/// Tries to parse an Expr.
fn expr<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    context(
        "expression",
        delimited(
            multispace0,
            spanned(|input| {
                let (input, left_expr) = single_expr(input)?;

                if let Ok((input, binary_ops)) = trailing_infix(input) {
                    Ok((
                        input,
                        Expr::BinaryOpSequence(Box::new(left_expr), binary_ops),
                    ))
                } else {
                    Ok((input, left_expr.value))
                }
            }),
            multispace0,
        ),
    )
    .parse(input)
}

// trailing infix parses a sequence of trailing infix operators. It's important that we don't recurse for each item in such
// a sequence as in practice we get really long such chains.
fn trailing_infix(input: Span) -> NomResult<Vec<(BinaryOp, SpannedExpr)>, ()> {
    fn single_infix(input: Span) -> NomResult<(BinaryOp, SpannedExpr), ()> {
        let (input, operator) = binary_op(input)?;
        let (input, right) = cut(single_expr).parse(input)?;
        Ok((input, (operator, right)))
    }
    many1(single_infix).parse(input)
}

/// Tries to parse an Expr::Word
fn expr_word<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    spanned(|input| {
        let (remaining, (quoted, word)) = maybe_quoted_word(input)?;

        let word = word.fragment();
        Ok((
            remaining,
            if !quoted && word == NONE {
                Expr::None
            } else {
                Expr::String(word)
            },
        ))
    })
    .parse(input)
}

/// Tries to parse an Expr::Integer
fn expr_int<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    spanned(|input| {
        let (remaining, word) = digit1(input)?;

        // Digits followed by more word characters are a word such as `2to3`, not an integer.
        if remaining.fragment().starts_with(is_word_char) {
            return Err(nom::Err::Error(nom::error::ParseError::from_error_kind(
                input,
                ErrorKind::Digit,
            )));
        }

        let value = match word.fragment() {
            "0" => 0,
            val if val.starts_with('0') => {
                return Err(nom::Err::Failure(nom::error::ParseError::from_error_kind(
                    input,
                    ErrorKind::Digit,
                )));
            }
            val => val.parse::<u64>().map_err(|_| {
                nom::Err::Failure(nom::error::ParseError::from_error_kind(
                    input,
                    ErrorKind::Digit,
                ))
            })?,
        };

        Ok((remaining, Expr::Integer(value)))
    })
    .parse(input)
}

/// Whether an unquoted word may contain `c`; only ASCII letters and digits count, as in the
/// grammar above.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "*/@.-_:$#%".contains(c)
}

/// Whether `s` parses back as itself without quotes. Text that does not must
/// be quoted when printed into a query.
pub fn is_unquoted_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_word_char)
}

fn word<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, Span<'a>, E> {
    maybe_quoted_word(input).map(|(remaining, (_unquoted, x))| (remaining, x))
}

fn maybe_quoted_word<'a, E: NomParseError<'a>>(
    input: Span<'a>,
) -> NomResult<'a, (bool, Span<'a>), E> {
    fn quoted<'a, E: NomParseError<'a>>(
        is_quoted: bool,
        //impl Fn(Span<'a>) -> NomResult<'a, Span<'a>, E>,
        mut inner: impl nom::Parser<Span<'a>, Output = Span<'a>, Error = E>,
    ) -> impl FnMut(Span<'a>) -> NomResult<'a, (bool, Span<'a>), E> {
        move |input| {
            let (remaining, value) = inner.parse(input)?;
            Ok((remaining, (is_quoted, value)))
        }
    }

    fn non_quoted_word<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, Span<'a>, E> {
        take_while1(is_word_char).parse(input)
    }

    alt((
        quoted(
            true,
            preceded(
                char('\''),
                cut(terminated(take_till(|c| c == '\''), char('\''))),
            ),
        ),
        quoted(
            true,
            preceded(
                char('"'),
                cut(terminated(take_till(|c| c == '"'), char('"'))),
            ),
        ),
        quoted(false, non_quoted_word),
    ))
    .parse(input)
}

fn binary_op(input: Span) -> NomResult<BinaryOp, ()> {
    fn keyword(long: &'static str) -> impl Fn(Span) -> NomResult<Span, ()> {
        // keywords require spaces separating from the exprs
        move |span| delimited(multispace1, tag(long), multispace1).parse(span)
    }

    fn symbol(sym: &'static str) -> impl FnMut(Span) -> NomResult<Span, ()> {
        // symbols can have spaces separating from the exprs, but don't require them
        move |span: Span| delimited(multispace0, tag(sym), multispace0).parse(span)
    }

    fn op(
        sym: &'static str,
        word: &'static str,
        op: BinaryOp,
    ) -> impl Fn(Span) -> NomResult<BinaryOp, ()> {
        move |span| Ok((alt((symbol(sym), keyword(word))).parse(span)?.0, op))
    }

    alt((
        op(EXCEPT, "except", BinaryOp::Except),
        op(INTERSECT, "intersect", BinaryOp::Intersect),
        op(UNION, "union", BinaryOp::Union),
    ))
    .parse(input)
}

/// Tries to parse an Expr::Set. Will fail if it detects an unfinished "set("
fn expr_set<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    fn set_args<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, Vec<Span<'a>>, E> {
        separated_list0(multispace1, word).parse(input)
    }

    spanned(|input| {
        let (input, _) = tag("set(").parse(input)?;
        cut(move |input| {
            let (input, args) = delimited(multispace0, set_args, multispace0).parse(input)?;
            let (input, _) = char(')').parse(input)?;
            Ok((input, Expr::Set(args)))
        })
        .parse(input)
    })
    .parse(input)
}

/// Tries to parse an Expr::FileSet. Will fail if it detects an unfinished "fileset("
fn expr_fileset<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    fn set_args<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, Vec<Span<'a>>, E> {
        separated_list0(multispace1, word).parse(input)
    }

    spanned(|input| {
        let (input, _) = tag("fileset(").parse(input)?;
        cut(move |input| {
            let (input, args) = delimited(multispace0, set_args, multispace0).parse(input)?;
            let (input, _) = char(')').parse(input)?;
            Ok((input, Expr::FileSet(args)))
        })
        .parse(input)
    })
    .parse(input)
}

/// Tries to parse an Expr::Function. Will fail if it detects an unfinished "func("
// We don't need to worry about "set(" as the outermost expr() ensures that never gets to here.
fn expr_function<'a, E: NomParseError<'a>>(input: Span<'a>) -> NomResult<'a, SpannedExpr<'a>, E> {
    fn function_args<'a, E: NomParseError<'a>>(
        input: Span<'a>,
    ) -> NomResult<'a, Vec<SpannedExpr<'a>>, E> {
        separated_list0(terminated(tag(","), multispace0), expr).parse(input)
    }

    spanned(|input| {
        // N.B. this can parse None() as a function call, rather than an Expr::None followed by an open paren (error). It will still be an error when there is no `None` function...
        let orig_input = input;
        let (input, function_name) = recognize(pair(
            alt((alpha1, tag("_"))),
            many0(alt((alphanumeric1, tag("_")))),
        ))
        .parse(input)?;
        if function_name.fragment() == NONE {
            return Err(nom::Err::Error(nom::error::make_error(
                orig_input,
                ErrorKind::Tag,
            )));
        }
        let (input, _) = char('(').parse(input)?;
        cut(move |input| {
            let (input, args) = terminated(function_args, char(')')).parse(input)?;
            Ok((
                input,
                Expr::Function {
                    function_name,
                    args,
                },
            ))
        })
        .parse(input)
    })
    .parse(input)
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    fn assert_err<'a, E, O: Debug>(res: IResult<Span, O, E>, ctx: &str) -> E
    where
        E: NomParseError<'a> + Debug,
    {
        match res {
            Err(nom::Err::Incomplete(..)) => {
                unreachable!("Only streaming parsing should return 'Err::Incomplete'");
            }
            Err(nom::Err::Error(e)) => e,
            _ => {
                panic!("{ctx} Got `{res:?}`");
            }
        }
    }

    fn assert_fail<'a, E, O: Debug>(res: IResult<Span, O, E>, ctx: &str) -> E
    where
        E: NomParseError<'a> + Debug,
    {
        match res {
            Err(nom::Err::Incomplete(..)) => {
                unreachable!("Only streaming parsing should return 'Err::Incomplete'");
            }
            Err(nom::Err::Failure(e)) => e,
            _ => {
                panic!("{ctx} Got `{res:?}`");
            }
        }
    }

    /// The docs allow an unquoted argument to start with a digit.
    #[test]
    fn test_unquoted_word_starting_with_digit() -> buck2_error::Result<()> {
        assert!(parse_expr("set(2to3)").is_ok());
        let parsed = parse_expr("kind(2to3, //foo:bar)")?;
        assert!(format!("{parsed:?}").contains("2to3"), "{parsed:?}");
        // Plain integers are still integers.
        let parsed = parse_expr("deps(//foo:bar, 2)")?;
        assert!(format!("{parsed:?}").contains("Integer(2)"), "{parsed:?}");
        // Unquoted words are ASCII, whether or not they start with a digit.
        assert!(parse_expr("kind(2é, //foo:bar)").is_err());
        assert!(parse_expr("kind(é, //foo:bar)").is_err());
        assert!(parse_expr("kind('2é', //foo:bar)").is_ok());
        Ok(())
    }

    fn parse_error(input: &str) -> String {
        parse_expr(input).unwrap_err().to_string()
    }

    #[test]
    fn test_parse_error() {
        assert_eq!(
            parse_error("deps(set(a b=c))"),
            "0: at line 1:\n\
             deps(set(a b=c))\n\
             \x20           ^\n\
             expected ')', found =\n\
             \n\
             1: at line 1, in expression:\n\
             deps(set(a b=c))\n\
             \x20    ^\n\
             \n\
             2: at line 1, in expression:\n\
             deps(set(a b=c))\n\
             ^\n\
             \n",
        );
        assert_eq!(
            parse_error("deps(\n  set('é' b=c)\n)"),
            "0: at line 2:\n\
             \x20 set('é' b=c)\n\
             \x20          ^\n\
             expected ')', found =\n\
             \n\
             1: at line 1, in expression:\n\
             deps(\n\
             \x20    ^\n\
             \n\
             2: at line 1, in expression:\n\
             deps(\n\
             ^\n\
             \n",
        );
        assert_eq!(
            parse_error(""),
            "0: in TakeWhile1, got empty input\n\
             \n\
             1: in Alt, got empty input\n\
             \n\
             2: in Alt, got empty input\n\
             \n\
             3: in expression, got empty input\n\
             \n",
        );
    }

    #[test]
    fn test_parse_error_far_into_long_line() {
        let filler = "a ".repeat(50_000);
        let query = format!("set({filler}b=c {filler})");
        let offset = query.find('=').unwrap();
        assert_eq!(
            parse_error(&query),
            format!(
                "0: at line 1:\n\
                 <<omitted>>{}<<omitted>>\n\
                 {}^\n\
                 expected ')', found =\n\
                 \n\
                 1: at line 1, in expression:\n\
                 {}<<omitted>>\n\
                 ^\n\
                 \n",
                &query[offset - 100..offset + 100],
                " ".repeat("<<omitted>>".len() + 100),
                &query[..100],
            ),
        );
    }

    #[test]
    fn test_display_long_binary_op_sequence() -> buck2_error::Result<()> {
        let ops = 70_000;
        let displayed = parse_expr(&format!("a{}", " + a".repeat(ops)))?.to_string();
        assert_eq!(
            displayed,
            format!("{} 'a'{}", "(".repeat(ops), " + 'a')".repeat(ops)),
        );
        Ok(())
    }

    #[test]
    fn test_expr() -> buck2_error::Result<()> {
        run_tests(
            expr,
            &[
                "f",
                "func(f, a)",
                "deps(%Ss) + set(a b c) ^ func(o(a(b, c, d)))",
                "a + b",
                "(a - (b))",
                "123",
            ],
            &[],
            &["func(", "set(", "(a", "01234"],
        );

        match parse_expr("set(a b c)") {
            Ok(Spanned {
                value: Expr::Set(..),
                ..
            }) => {}
            v => panic!("expected set expr, got `{v:?}`"),
        }

        match parse_expr("set(a b c) ^ func()") {
            Ok(Spanned {
                value: Expr::BinaryOpSequence(..),
                ..
            }) => {}
            v => panic!("expected intersect expr, got `{v:?}`"),
        }

        match parse_expr("func(set(a b c))") {
            Ok(Spanned {
                value: Expr::Function { .. },
                ..
            }) => {}
            v => panic!("expected function expr, got `{v:?}`"),
        }

        match parse_expr("//:tgt") {
            Ok(Spanned {
                value: Expr::String("//:tgt"),
                ..
            }) => {}
            v => panic!("expected '//:tgt', got `{v:?}`"),
        }

        Ok(())
    }

    fn run_tests<'a, O: Debug, F: FnMut(Span<'a>) -> IResult<Span<'a>, O, ()> + Copy>(
        mut parser: F,
        good_cases: &'a [&'a str],
        recoverable_cases: &'a [&'a str],
        nonrecoverable_cases: &'a [&'a str],
    ) {
        for case in good_cases {
            // good_cases the parser should consume the whole thing
            // maybe should consider splitting it into two sets of cases for the caller to
            // explicitly indicate the ones that should be all consuming
            all_consuming(parser)
                .parse(Span::new(case))
                .unwrap_or_else(|err| {
                    panic!(
                        "Expected successful, complete parse for input `{case}`. Got error `{err:?}`"
                    )
                });
        }

        for case in recoverable_cases {
            assert_err(
                parser(Span::new(case)),
                &format!("Expected recoverable error for input `{case}`."),
            );
        }

        for case in nonrecoverable_cases {
            assert_fail(
                parser(Span::new(case)),
                &format!("Expected hard failure for input `{case}`."),
            );
        }
    }

    #[test]
    fn test_function() -> buck2_error::Result<()> {
        run_tests(
            expr_function,
            &["a()", "a(a)", "a(a, b, c)", "a(a,   b,   c)"],
            // As long as we don't match "name(", it should be recoverable
            &["xyz", "", "(", ",a(", " a()", "a,a()"],
            // An error after the name + opening paren is non-recoverable
            &["a(", "a(a()", "a(abc, ", "a(abc, ()"],
        );
        Ok(())
    }

    #[test]
    fn test_set() -> buck2_error::Result<()> {
        run_tests(
            expr_set,
            &["set(a)", "set(a b c)", "set(a   b   c)", "set(%s)", "set()"],
            // As long as we don't match "set(", it should be recoverable
            &["set", "", "(", "abc(", " set()", ",set()"],
            // An error after "set" + opening paren is non-recoverable
            &["set(", "set(a,)", "set(abc ", "set(abc ()"],
        );
        Ok(())
    }

    #[test]
    fn test_word() -> buck2_error::Result<()> {
        run_tests(
            expr_word,
            &[
                "@cell//package:target",
                "'quoted'",
                "regex.*$#",
                r#""double^  -quoted""#,
                "%s",
                "%Ss",
            ],
            // there's not a lot of errors
            &["^word", "", ",word"],
            // unfinished quotes are unrecoverable.
            &["'unfinished quote", "'wrong quote end\""],
        );
        Ok(())
    }

    #[test]
    fn test_integer() -> buck2_error::Result<()> {
        run_tests(expr_int, &["0", "1234"], &["w123", ".1", ""], &["0123"]);
        Ok(())
    }

    #[test]
    fn test_trailing_infix() -> buck2_error::Result<()> {
        run_tests(
            trailing_infix,
            &[
                " + b",
                " + (b)",
                " ^ (b)",
                " - (b)- (c) -(d)-(e)",
                " intersect b",
                " union b",
                " except b",
            ],
            // there's not a lot of errors
            &["", "| b", " |b"],
            &[],
        );
        Ok(())
    }
}
