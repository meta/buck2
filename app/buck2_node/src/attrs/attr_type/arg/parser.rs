/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Handles parsing macros out of an attrs.arg()
//!
//! Much of this behavior is inherited from buckv1, which is documented
//! here <https://buck.build/function/string_parameter_macros.html> and here
//! <https://github.com/facebook/buck/blob/5bc82b7c90f1a5c5ac70e2de7d2c2170c289ee79/src/com/facebook/buck/core/macros/MacroFinderAutomaton.java>
//!
//! Many rule attributes that accept strings actually accept an attrs.arg(). These allow
//! users to specify "String parameter macros" that are placeholders that buck will expand
//! to their final values later in the build. Common examples of these would `$(location //some:target)`
//! or `$(exe //some:target)`.
//!
//! This parser handles parsing a string with zero or more string parameter macros into a parsed
//! structure separating the literals from the macros and holding the types and args for each macro.
//!
//! A macro is defined to start at any `$(` with zero or an even number of preceding `\`. I.e. all of `x$(`,
//! `x\\$(`, and `x\\\\$(` start a macro, but none of `x\$(`, `x\\\$(` or `x\\\\\$(` start one.
//!
//! Within an escaped macro (a section starting with `\$(`), we ignore everything until a matching closing `)`.
//! If there are any `(` within that section, they will be matched against `)` first (i.e. we maintain a count
//! of opened parens and closing parens and the escaped macro ends when that reaches 0).
//!
//! Once within a macro, an `@` can appear just before the type to indicate that the value of the macro should
//! be written to a file and the command should be passed `@<filename>` where `<filename>` is the file where the
//! macro value has been written.
//!
//! A macro consists of that optional write-to-file signifier followed by a type and then zero or more
//! space-separated args.
//!
//! A macro type must be non-empty and consists of characters in `[a-zA-Z0-9_]` followed by whitespace or the
//! macro-ending ')'.
//!
//! Macro args are separated by whitespace. There are two types of args, "quoted" and "unquoted".
//!
//! A "quoted" arg starts with either `'` or `"`. It continues until it reaches the first matching
//! non-`\`-escaped `'` or `"` (as appropriate). The value of the arg will replace all `\`-escaped chars
//! with just the char itself. I.e. `\\` becomes `\`, `\"` becomes `"`, `\a` becomes `a`.
//!
//! An unquoted arg is normally terminated by the first whitespace or `)`. However, unquoted args have special
//! behavior when they encounter any `(` that will make them continue until the parens are balanced. While an
//! unquoted arg has seen more `(` than `)`, it will not be terminated by whitespace or `)`.
//!
//! We diverge from buckv1 in a handful of known ways.
//!
//! 1. buck1 allows pretty much any characters to appear in a macro type. We restrict it to alphanumeric and `_`.
//!
//! 2. buck1 disallows spaces entirely within unquoted args. This can be surprising. Unquoted args are generally used for
//!    the query part of query macros, and in other contexts where buck accepts queries it allows whitespace.
//!    Example, the string "$(query_outputs deps(//some:target, 3))" would be rejected by buck1 due to the space before the 3.
//!
//! Some examples:
//!
//! ```console
//! some string $(location //some:target)
//! more $(complex with(some(function(calls))) and(more)) something $(exe)
//! this should have no backslash \$()
//! $(macro "some ( weird arg")
//! ```

// TODO(cjhopman): There's a lot awkward about this definition. IMO, we especially should remove the "unquoted" arg's special
// handling for parens. If someone wants a query macro, they can quote the arg. It's also kind of odd that we only really treat
// the string `\$(` as escaped in the outer string (we replace it with `$(`) but don't just generally treat `\` as an escaping
// character.
use std::result;

#[derive(Debug, PartialEq)]
pub struct ParsedMacro {
    /// Indicates that the value of the macro should be written to a file and the command should be passed `@<filename>` where
    /// `<filename>` is the file where the macro value has been written.
    pub write_to_file: bool,
    /// "type" of the macro. Ex. "location", "exe".
    pub macro_type: String,
    /// Macro args. Ex $(some_macro //a:b //c:d) would have ["//a:b", "//c:d"] as the args.
    pub args: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum ArgItem {
    String(Box<str>),
    Macro(ParsedMacro),
}

#[derive(Debug, PartialEq)]
pub struct ParsedArg(pub(crate) Vec<ArgItem>);

impl ParsedArg {
    pub fn into_items(self) -> Vec<ArgItem> {
        self.0
    }
}

/// Parsed a string into a structure with macros and their types and args identified.
pub fn parse_macros(input: &str) -> buck2_error::Result<ParsedArg> {
    match read(input) {
        Ok((value, remaining)) => {
            assert!(
                remaining.is_empty(),
                "somehow had remaining stuff after a successful macro parse. Had `{remaining}` remaining."
            );

            Ok(value)
        }
        Err((remaining, err)) => {
            // remaining indicates what we were parsing when something went wrong. We process strictly left-to-right
            // and so it's easy to compute where that is in input.
            let error_start = input.len() - remaining.len();

            // We truncate the string to 60 bytes, max 15 before, cutting only at char boundaries.
            let message_start = input.floor_char_boundary(error_start.saturating_sub(15));
            let message_end =
                input.floor_char_boundary(std::cmp::min(input.len(), message_start + 60));

            // We'll print a line of `^` to indicate where the processing failed.
            let pointer = error_start - message_start;

            Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "E:{}\n    V:{}\n    M:{}{}\n",
                err,
                &input[message_start..message_end],
                " ".repeat(pointer),
                "^".repeat(message_end - message_start - pointer)
            ))
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ArgParseError {
    #[error("Unfinished quoted arg, expected a `{0}`")]
    UnfinishedQuotedArg(char),
    #[error("Unfinished non-quoted arg")]
    UnfinishedUnquotedArg,
    #[error("Unfinished macro invocation")]
    UnfinishedMacroInvocation,
    #[error("Invalid character in macro type, expected [a-zA-Z0-9_]")]
    MacroTypeInvalidChar,
    #[error("Unfinished macro type, expected a ' ' or ')'")]
    MacroTypeUnfinished,
    #[error("Macro type was missing")]
    MacroTypeMissing,
}

/// An Error includes the slice where the error occurred. As we process strictly
/// left-to-right, this slice allows us to easily identify the location of the error in the original input.
type Error<'a> = (&'a str, ArgParseError);

/// A Result includes both some parsed type and a slice of what remains to be parsed.
type Result<'a, T> = result::Result<(T, &'a str), Error<'a>>;

// We diverge slightly from buckv1 here.
//
// See https://www.internalfb.com/diffs/D3917438?transaction_fbid=1890520777849195 where that decision in v1 was questioned.

fn consume_whitespace(input: &str) -> &str {
    input.trim_start_matches(|c: char| c.is_ascii_whitespace())
}

fn unescape(input: &str) -> String {
    let mut fixed = String::with_capacity(input.len());
    let mut escaped = false;
    for c in input.chars() {
        if escaped {
            fixed.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            fixed.push(c);
        }
    }
    fixed
}

// An unquoted arg has tricky behavior to support more complex query macros. While parsing the arg, if we encounter
// any (unescaped) left-paren, the arg will be considered to continue until we've seen a balanced set of left and
// right parens (it will end immediately at that point).
//
// Examples:
// ```ignore
// $(macro deps(123)) -> arg1 == "deps(123)"
// $(macro deps(123)abc) -> arg1 == "deps(123)", arg2=abc
// $(macro a(b(c(d)))) -> arg1 == "a(b(b(d))))"
// ```
// TODO: that second case seems like a bug in buckv1 and it should be just a single arg. We've preserved the v1 behavior.
fn read_unquoted_arg(input: &str) -> Result<'_, String> {
    let mut has_escapes = false;
    let mut paren_count = 0;
    let mut char_indices = input.char_indices();
    let mut pos = char_indices.next();

    while let Some((_, c)) = pos {
        match c {
            ')' if paren_count == 0 => {
                break;
            }
            ')' => {
                paren_count -= 1;
            }
            '(' => {
                paren_count += 1;
            }
            '\\' => {
                has_escapes = true;
                char_indices.next();
            }
            v if v.is_ascii_whitespace() => {
                if paren_count == 0 {
                    break;
                }
            }
            _ => {}
        }
        pos = char_indices.next();
    }

    match pos {
        None => Err((input, ArgParseError::UnfinishedUnquotedArg)),
        Some((pos, _)) => {
            let (arg, rest) = (&input[0..pos], &input[pos..]);
            // The common case, by far, is that nothing is escaped.
            if !has_escapes {
                Ok((arg.to_owned(), rest))
            } else {
                Ok((unescape(arg), rest))
            }
        }
    }
}

// A quoted arg is simple, we will read until we find a matching un-escaped quote.
fn read_quoted_arg(input: &str, quote: char) -> Result<'_, String> {
    let mut has_escapes = false;
    let mut char_indices = input.char_indices();
    let mut pos = char_indices.next();
    while let Some((_, c)) = pos {
        match c {
            v if v == quote => {
                break;
            }
            '\\' => {
                has_escapes = true;
                char_indices.next();
            }
            _ => {}
        }
        pos = char_indices.next();
    }

    match pos {
        None => Err((input, ArgParseError::UnfinishedQuotedArg(quote))),
        Some((pos, _)) => {
            let (arg, rest) = (&input[0..pos], &input[(pos + 1)..]);
            // The common case, by far, is that nothing is escaped.
            if !has_escapes {
                Ok((arg.to_owned(), rest))
            } else {
                Ok((unescape(arg), rest))
            }
        }
    }
}

fn read_macro_arg(input: &str) -> Result<'_, String> {
    if let Some(rest) = input.strip_prefix('\'') {
        read_quoted_arg(rest, '\'')
    } else if let Some(rest) = input.strip_prefix('"') {
        read_quoted_arg(rest, '"')
    } else {
        read_unquoted_arg(input)
    }
}

// This is much stricter than buckv1. v1 allows nearly any character in the macro type. We allow only alphanumeric, '-', and '_'.
fn read_macro_type(input: &str) -> Result<'_, String> {
    match input.find(|c: char| c.is_whitespace() || c == ')') {
        None => Err((input, ArgParseError::MacroTypeUnfinished)),
        Some(0) => Err((input, ArgParseError::MacroTypeMissing)),
        Some(pos) => {
            let macro_type = &input[..pos];
            if let Some(pos) =
                macro_type.find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
            {
                Err((&input[pos..], ArgParseError::MacroTypeInvalidChar))
            } else {
                Ok((macro_type.to_owned(), &input[pos..]))
            }
        }
    }
}

// A macro consists of a macro name followed by any number of space separated macro args eventually terminated by a closing paren.
fn read_macro(input: &str) -> Result<'_, ParsedMacro> {
    // We only take the leading `$(` so that error message point to that instead of the beginning of the type.
    let working = input.strip_prefix("$(").unwrap_or_else(|| {
        panic!(
            "the caller should've ensure that we're actually at the start of a macro. Got `{input}`",
        )
    });
    let (write_to_file, working) = match working.strip_prefix('@') {
        Some(working) => (true, working),
        None => (false, working),
    };
    let (macro_type, working) = read_macro_type(working)?;
    let mut working = consume_whitespace(working);

    let mut args = Vec::new();
    while let Some(c) = working.chars().next() {
        if c == ')' {
            return Ok((
                ParsedMacro {
                    write_to_file,
                    macro_type,
                    args,
                },
                &working[1..],
            ));
        }

        let (arg, remaining) = read_macro_arg(working)?;
        working = consume_whitespace(remaining);
        args.push(arg);
    }

    Err((input, ArgParseError::UnfinishedMacroInvocation))
}

fn read_literal_opt(input: &str) -> Result<'_, Option<Box<str>>> {
    // To match v1's approach to this is non-trivial. To start a macro requires a `$(` with an
    // even number of preceding `\` (as the first of each pair escapes the second). If there's
    // an odd number of preceding `\`, one of them should be removed.

    // Fast check that there are no macro refs in the string, which is the common case.
    // We can do better than `memchr` (given our strings are short), but not much.
    match memchr::memchr2(b'$', b'\\', input.as_bytes()) {
        None => Ok((Some(input.into()), "")),
        Some(pos) => read_literal_opt_slow(input, pos),
    }
}

fn read_literal_opt_slow(input: &str, pos: usize) -> Result<'_, Option<Box<str>>> {
    let mut char_indices = input.bytes().enumerate().skip(pos);

    let mut indices_to_drop = Vec::new();
    enum State {
        Searching,
        Escaped,
        Dollar,
        EscapedDollar,
        // Tracks the number of left parens that need a matching right paren.
        EscapedMacro(usize),
    }

    use State::*;

    let mut state = Searching;

    let mut pos = char_indices.next();
    while let Some((idx, c)) = pos {
        state = match (state, c) {
            (Searching, b'\\') => Escaped,
            (Dollar, b'\\') => Escaped,
            (EscapedDollar, b'\\') => Escaped,

            (Escaped, b'$') => EscapedDollar,
            (Escaped, _) => Searching,

            (Searching, b'$') => Dollar,
            (Searching, _) => Searching,

            (Dollar, b'(') => {
                // found a macro
                break;
            }
            (EscapedDollar, b'(') => {
                // Indicates we hit the sequence `\$(` and we need to drop the `\`
                indices_to_drop.push(idx - 2);
                EscapedMacro(1)
            }
            (EscapedMacro(1), b')') => Searching,
            (EscapedMacro(n), b')') => EscapedMacro(n - 1),
            (EscapedMacro(n), b'(') => EscapedMacro(n + 1),
            (EscapedMacro(n), _) => EscapedMacro(n),
            // Note that '(' and '\' is handled for both of thes above.
            (Dollar, _) => Searching,
            (EscapedDollar, _) => Searching,
        };
        pos = char_indices.next();
    }

    let literal_end = pos.map_or_else(|| input.len(), |p| p.0 - 1);

    if literal_end > 0 {
        // TODO: fixup escaped things.
        let literal = if indices_to_drop.is_empty() {
            input[..literal_end].to_owned()
        } else {
            let mut literal = String::with_capacity(literal_end);
            literal.push_str(&input[..indices_to_drop[0]]);
            for window in indices_to_drop.windows(2) {
                literal.push_str(&input[(window[0] + 1)..window[1]]);
            }
            literal.push_str(&input[(indices_to_drop.last().unwrap() + 1)..literal_end]);
            literal
        };
        Ok((Some(literal.into_boxed_str()), &input[literal_end..]))
    } else {
        Ok((None, input))
    }
}

fn read(input: &str) -> Result<'_, ParsedArg> {
    let (literal, remaining) = read_literal_opt(input)?;
    let mut working = remaining;

    if working.is_empty() {
        return Ok((
            ParsedArg(vec![ArgItem::String(literal.unwrap_or_else(|| "".into()))]),
            "",
        ));
    }

    let mut complex = Vec::new();
    if let Some(literal) = literal {
        complex.push(ArgItem::String(literal));
    }

    while !working.is_empty() {
        let (literal, remaining) = read_literal_opt(working)?;
        working = remaining;
        if let Some(literal) = literal {
            complex.push(ArgItem::String(literal));
        }

        if !working.is_empty() {
            // we must be at the beginning of a macro.
            let (parsed_macro, remaining) = read_macro(working)?;
            working = remaining;
            complex.push(ArgItem::Macro(parsed_macro));
        }
    }

    Ok((ParsedArg(complex), ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    #[allow(unused)] // for tests debug display
    struct OwnedError {
        remaining: String,
        error: ArgParseError,
    }

    impl From<(&'_ str, ArgParseError)> for OwnedError {
        fn from(err: (&'_ str, ArgParseError)) -> Self {
            OwnedError {
                remaining: err.0.to_owned(),
                error: err.1,
            }
        }
    }

    #[test]
    fn test_unquoted() -> result::Result<(), OwnedError> {
        assert_eq!(read_macro_arg("abcd ")?, ("abcd".to_owned(), " "));
        assert_eq!(read_macro_arg("abcd)")?, ("abcd".to_owned(), ")"));
        assert_eq!(
            read_macro_arg("deps(//some:target))")?,
            ("deps(//some:target)".to_owned(), ")")
        );
        assert_eq!(
            read_macro_arg("deps(//some:target, 3, first_order_deps()) ")?,
            ("deps(//some:target, 3, first_order_deps())".to_owned(), " ")
        );

        Ok(())
    }

    #[test]
    fn test_quoted() -> result::Result<(), OwnedError> {
        assert_eq!(read_macro_arg("' abcd )'")?, (" abcd )".to_owned(), ""));
        assert_eq!(read_macro_arg("' ab%cd )'")?, (" ab%cd )".to_owned(), ""));
        assert_eq!(
            read_macro_arg(r#"" \"\$\\ ")"#)?,
            (r#" "$\ "#.to_owned(), ")")
        );

        Ok(())
    }

    #[test]
    fn test_macro_type() -> result::Result<(), OwnedError> {
        assert_eq!(read_macro_type("name ")?, ("name".to_owned(), " "));
        assert_eq!(read_macro_type("name)")?, ("name".to_owned(), ")"));
        assert_eq!(
            read_macro_type("platform-name ")?,
            ("platform-name".to_owned(), " ")
        );
        assert!(read_macro_type("platform%name ").is_err());
        assert!(read_macro_type("platform$name ").is_err());
        Ok(())
    }

    #[test]
    fn test_parse_macros_error_window_start_inside_multibyte_char() {
        // The window of the input quoted by the error starts 15 bytes before the error, which
        // would fall inside `\u{e9}` here; it is moved back to the char boundary.
        let input = format!("\u{e9}{}$(", "a".repeat(12));
        let error = parse_macros(&input).unwrap_err().to_string();
        assert!(error.contains(&format!("V:{input}")), "{error}");
    }

    #[test]
    fn test_parse_macros_error_window_end_inside_multibyte_char() {
        let input = format!("a$({}\u{e9}", "a".repeat(56));
        let error = parse_macros(&input).unwrap_err().to_string();
        assert!(error.contains("V:a$(aaaa"), "{error}");
        assert!(!error.contains('\u{e9}'), "{error}");
    }

    #[test]
    fn test_parse_macros() -> buck2_error::Result<()> {
        assert_eq!(
            ParsedArg(vec![ArgItem::String("contains no macros".into())]),
            parse_macros(r#"contains no macros"#)?
        );

        assert_eq!(
            ParsedArg(vec![ArgItem::String("contains no $(macros)".into())]),
            parse_macros(r#"contains no \$(macros)"#)?
        );

        let macro1 = || {
            ArgItem::Macro(ParsedMacro {
                macro_type: "exe".to_owned(),
                args: Vec::new(),
                write_to_file: false,
            })
        };
        let macro2 = || {
            ArgItem::Macro(ParsedMacro {
                macro_type: "location".to_owned(),
                args: vec!["//some:target".to_owned()],
                write_to_file: true,
            })
        };

        assert_eq!(
            ParsedArg(vec![macro1(), ArgItem::String(" and ".into()), macro2()]),
            parse_macros(r#"$(exe) and $(@location //some:target)"#)?
        );

        let macro3 = ArgItem::Macro(ParsedMacro {
            macro_type: "zzz".to_owned(),
            args: vec![
                "1".to_owned(),
                r#"yyy(abc def)"#.to_owned(),
                r#"\\\"#.to_owned(),
            ],
            write_to_file: false,
        });

        assert_eq!(
            ParsedArg(vec![macro1(), macro2(), macro3]),
            parse_macros(
                r#"$(exe)$(@location     //some:target)$(zzz  '\1' yyy(abc def) "\\\\\\")"#
            )?
        );

        assert_eq!(
            ParsedArg(vec![ArgItem::String(r#"$(echo -n "$NEW" | sed -e :a -e "s|^.\{1,$(expr "$(echo -n "$OLD" | wc -c)" - 1)\}$|&/|;ta")"#.into())]),
            parse_macros(r#"\$(echo -n "$NEW" | sed -e :a -e "s|^.\{1,$(expr "$(echo -n "$OLD" | wc -c)" - 1)\}$|&/|;ta")"#)?
        );

        Ok(())
    }
}
