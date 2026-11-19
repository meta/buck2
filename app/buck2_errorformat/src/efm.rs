/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::take_while1;
use nom::character::complete::anychar;
use nom::character::complete::char;
use nom::character::complete::one_of;
use nom::character::complete::satisfy;
use nom::combinator::opt;
use nom::combinator::recognize;
use nom::error::Error as NomError;
use nom::error::ErrorKind;
use nom::multi::many0;
use nom::sequence::pair;

use crate::Entry;

/// Represents an error format pattern
#[derive(Debug)]
pub(crate) struct SingleErrorFormatRule {
    /// Regular expression for matching error lines
    pub regex: fancy_regex::Regex,
    pub(crate) format_specifier: Option<FormatSpecifier>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FormatSpecifier {
    pub(crate) specifier: char,
    pub(crate) modifier: PrefixModifier,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MultiLineType {
    Error,
    Warning,
    Info,
    Note,
    Unknown,
}

impl MultiLineType {
    pub(crate) fn error_char(&self) -> char {
        match self {
            MultiLineType::Error => 'E',
            MultiLineType::Warning => 'W',
            MultiLineType::Info => 'I',
            MultiLineType::Note => 'N',
            MultiLineType::Unknown => 'A',
        }
    }
}

impl FormatSpecifier {
    /// https://neovim.io/doc/user/quickfix.html#errorformat-multi-line
    /// %E        start of a multi-line error message
    /// %W        start of a multi-line warning message
    /// %I        start of a multi-line informational message
    /// %N        start of a multi-line note message
    /// %A        start of a multi-line message (unspecified type)
    /// %>        for next line start with current pattern again efm-%>           // TODO: we don't support this yet
    /// %C        continuation of a multi-line message
    /// %Z        end of a multi-line message
    pub(crate) fn is_enter_multi_line(&self) -> bool {
        matches!(self.specifier, 'E' | 'W' | 'I' | 'N' | 'A')
    }

    pub(crate) fn enter_multi_line_type(&self) -> Option<MultiLineType> {
        match self.specifier {
            'E' => Some(MultiLineType::Error),
            'W' => Some(MultiLineType::Warning),
            'I' => Some(MultiLineType::Info),
            'N' => Some(MultiLineType::Note),
            'A' => Some(MultiLineType::Unknown),
            _ => None,
        }
    }

    pub(crate) fn is_continuation_multi_line(&self) -> bool {
        matches!(self.specifier, 'C')
    }

    pub(crate) fn is_end_multi_line(&self) -> bool {
        matches!(self.specifier, 'Z')
    }

    // These prefixes are useful if the file name is given once and multiple messages
    // follow that refer to this file name.
    //
    // %O single-line file message: overread the matched part
    // %P single-line file message: push file %f onto the stack
    // %Q single-line file message: pop the last file from stack
    pub(crate) fn is_push_file_on_stack(&self) -> bool {
        matches!(self.specifier, 'P')
    }

    pub(crate) fn is_pop_file_on_stack(&self) -> bool {
        matches!(self.specifier, 'Q')
    }

    pub(crate) fn is_overread_matched_part(&self) -> bool {
        matches!(self.specifier, 'O')
    }

    pub(crate) fn is_push_dir_on_stack(&self) -> bool {
        matches!(self.specifier, 'D')
    }

    pub(crate) fn is_pop_dir_on_stack(&self) -> bool {
        matches!(self.specifier, 'X')
    }

    pub(crate) fn is_ignore_multi_line(&self) -> bool {
        matches!(self.modifier, PrefixModifier::Minus)
    }

    pub(crate) fn is_generic_message(&self) -> bool {
        matches!(self.specifier, 'G')
    }

    pub(crate) fn capture_entire_line_as_message(&self) -> bool {
        matches!(self.modifier, PrefixModifier::Plus)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PrefixModifier {
    None,
    Plus,
    Minus,
}

impl SingleErrorFormatRule {
    /// Creates a new error format pattern from a format string
    pub fn new(errorformat: &str) -> Result<Self, ParseEfmError> {
        let (_remain_input, res) = parse_efm(errorformat)?;

        Ok(res)
    }

    /// Attempts to match a line against this error format pattern
    /// If return Ok(None) means not match the regex
    /// If Err, means we have error in parse the string to int, etc
    /// Only when m.as_str().parse fails, we return Err, but it should be ok to unwrap
    pub fn match_line(&self, line: &str) -> Result<Option<Entry>, ParseEfmError> {
        if let Some(caps) = self.regex.captures(line)? {
            let mut entry = Entry::new();
            entry.filename = caps
                .name(FILE_PATTERN.capture_group)
                .map(|m| m.as_str().to_owned());

            entry.lnum = caps
                .name(LINE_PATTERN.capture_group)
                .map(|m| m.as_str().parse())
                .transpose()?;

            entry.end_lnum = caps
                .name(END_LINE_PATTERN.capture_group)
                .map(|m| m.as_str().parse())
                .transpose()?;

            entry.col = caps
                .name(COLUMN_PATTERN.capture_group)
                .map(|m| m.as_str().parse())
                .transpose()?;

            entry.end_col = caps
                .name(END_COLUMN_PATTERN.capture_group)
                .map(|m| m.as_str().parse())
                .transpose()?;

            entry.message = caps
                .name(MESSAGE_PATTERN.capture_group)
                .map(|m| m.as_str().to_owned());

            entry.error_type = caps
                .name(ERR_TYPE_PATTERN.capture_group)
                .map(|m| m.as_str().to_owned());

            entry.error_number = caps
                .name(ERR_NUMBER_PATTERN.capture_group)
                .map(|m| m.as_str().parse())
                .transpose()?;

            entry.rest = caps
                .name(REST_PATTERN.capture_group)
                .map(|m| m.as_str().to_owned());

            entry.pointer_line = caps
                .name(POINTER_PATTERN.capture_group)
                .map(|m| m.as_str().to_owned());

            entry.virtual_col = caps
                .name(VIRTUAL_COLUMN_PATTERN.capture_group)
                .map(|m| m.as_str().parse())
                .transpose()?;

            entry.search_text = caps
                .name(SEARCH_PATTERN.capture_group)
                .map(|m| m.as_str().to_owned());

            entry.original_err_lines = vec![line.to_owned()];

            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }
}

/// Represents a single `%`-placeholder pattern used in 'errorformat'.
///
/// Each `EfmPercentPattern` specifies:
/// - `pattern`: the `%` code (e.g. `'f'` for a file name).
/// - `regex_pattern`: the concrete regex fragment that implements the
///   matching logic for this code, with a named capture group.
/// - `capture_group`: the name of the capture group in `regex_pattern`
///   that extracts the matched value.
///
/// Example: in FILE_PATTERN, the `%f` pattern is expanded into a regex that captures
/// file paths. It supports optional Windows drive letters (e.g. `C:`),
/// allows escaped spaces (`\ `), and consumes non-space characters until
/// the next separator. The result is captured under the group `"f"`.
struct EfmPercentPattern {
    pattern: char,
    regex_pattern: &'static str,
    capture_group: &'static str,
}

const FILE_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'f',
    regex_pattern: r"(?<f>(?:[[:alpha:]]:)?(?:\\ |[^ ])+?)",
    capture_group: "f",
};

const ERR_NUMBER_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'n',
    regex_pattern: r"(?<n>\d+)",
    capture_group: "n",
};

const LINE_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'l',
    regex_pattern: r"(?<l>\d+)",
    capture_group: "l",
};

const END_LINE_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'e',
    regex_pattern: r"(?<e>\d+)",
    capture_group: "e",
};

const COLUMN_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'c',
    regex_pattern: r"(?<c>\d+)",
    capture_group: "c",
};

const END_COLUMN_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'k',
    regex_pattern: r"(?<k>\d+)",
    capture_group: "k",
};

const ERR_TYPE_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 't',
    regex_pattern: r"(?<t>.)",
    capture_group: "t",
};

const MESSAGE_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'm',
    regex_pattern: r"(?<m>.+)",
    capture_group: "m",
};

/// Match the rest of the
const REST_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'r',
    regex_pattern: r"(?<r>.*)",
    capture_group: "r",
};

/// pointer line, the line usually show under the error line to point out the specific
/// location, e.g. ^ or ~~~~. Used to get the specific column
const POINTER_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'p',
    regex_pattern: r"(?<p>[-\t .]*)", // Note: Tab characters in Rust strings are interpreted as a single tab character.
    capture_group: "p",
};

/// Matches the error's virtual column, which is the visual column position considering tab character expansion.
const VIRTUAL_COLUMN_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 'v',
    regex_pattern: r"(?<v>\d+)",
    capture_group: "v",
};

/// Matches a string from the error output that Vim then uses to search for the exact error line within the specified file.
const SEARCH_PATTERN: EfmPercentPattern = EfmPercentPattern {
    pattern: 's',
    regex_pattern: r"(?<s>.+)",
    capture_group: "s",
};

macro_rules! generate_efm_percent_percent_utils {
    (
        // const list (X, Y, Z, ...)
        $($pattern_const_ident:ident),+
        $(,)?
    ) => {
        #[inline]
        fn find_efm_format_pattern(input_char: char) -> Option<&'static str> {
            match input_char {
                $(
                    // $pattern_const_ident.pattern
                    ch if ch == $pattern_const_ident.pattern => Some($pattern_const_ident.regex_pattern),
                )+
                _ => None,
            }
        }
    };
}

generate_efm_percent_percent_utils!(
    FILE_PATTERN,
    ERR_NUMBER_PATTERN,
    LINE_PATTERN,
    END_LINE_PATTERN,
    COLUMN_PATTERN,
    END_COLUMN_PATTERN,
    ERR_TYPE_PATTERN,
    MESSAGE_PATTERN,
    REST_PATTERN,
    POINTER_PATTERN,
    VIRTUAL_COLUMN_PATTERN,
    SEARCH_PATTERN,
);

// Nom parser combinators for errorformat parsing
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum ParseEfmError {
    #[error("Invalid format string: {0}")]
    InvalidFormat(String),
    #[error("Regex error: {0}")]
    // Boxed to reduce enum size and avoid clippy::result_large_err warning
    Regex(Box<fancy_regex::Error>),
    #[error("Parse int error: {0}")]
    ParseInt(std::num::ParseIntError),
    #[error("NomParse error: {0}")]
    NomParse(nom::Err<NomError<String>>),
}

impl From<fancy_regex::Error> for ParseEfmError {
    fn from(err: fancy_regex::Error) -> Self {
        ParseEfmError::Regex(Box::new(err))
    }
}

impl From<std::num::ParseIntError> for ParseEfmError {
    fn from(err: std::num::ParseIntError) -> Self {
        ParseEfmError::ParseInt(err)
    }
}

impl<'a> From<nom::Err<NomError<&'a str>>> for ParseEfmError {
    fn from(value: nom::Err<NomError<&'a str>>) -> Self {
        ParseEfmError::NomParse(value.to_owned())
    }
}

fn parse_efm(input: &str) -> Result<(&str, SingleErrorFormatRule), ParseEfmError> {
    let (input, format_specifier) = opt(parse_format_specifier).parse(input)?;
    let (input, regpats) = many0(parse_regpat).parse(input)?;

    if !input.is_empty() {
        return Err(ParseEfmError::InvalidFormat(format!(
            "Unrecognized errorformat pattern: '{}'",
            input
        )));
    }

    let repgat = regpats.join("");

    let final_repgat = format!("^{repgat}$");
    let regex = fancy_regex::Regex::new(&final_repgat)?;
    let efm = SingleErrorFormatRule {
        regex,
        format_specifier,
    };

    Ok((input, efm))
}

fn parse_regpat(input: &str) -> IResult<&str, String> {
    alt((parse_percent_pattern, parse_escaped_char, parse_normal_char)).parse(input)
}

pub(crate) fn parse_format_specifier(input: &str) -> IResult<&str, FormatSpecifier> {
    let (input, _) = char('%')(input)?;
    let (input, modifier) = opt(one_of("+-")).parse(input)?;

    let (input, prefix) = one_of("DXAEWINCZGOPQ")(input)?;
    let m = match modifier {
        Some('+') => PrefixModifier::Plus,
        Some('-') => PrefixModifier::Minus,
        None => PrefixModifier::None,
        _ => unreachable!(),
    };

    let res = FormatSpecifier {
        specifier: prefix,
        modifier: m,
    };

    Ok((input, res))
}

// parse %... pattern expect parse_format_specifier(%+E, %E, etc)
fn parse_percent_pattern(input: &str) -> IResult<&str, String> {
    let (input, _) = char('%')(input)?;
    alt((parse_format_pattern, parse_star_pattern, parse_special_char)).parse(input)
}

// The scanf()-like "%*[]" notation, which is supported for backward-compatibility of errorformat.
// Example: "%\\d%\\+" ("\d\+", "any number") is equivalent to "%*\\d".
// Details can be found at https://neovim.io/doc/user/quickfix.html#_7.-the-error-format
//
// start with *
// *[a-z0-9]
// *\D
fn parse_star_pattern(input: &str) -> IResult<&str, String> {
    let (input, _) = char('*')(input)?;
    let (input, pattern) = alt((parse_bracket_pattern, parse_escape_pattern)).parse(input)?;
    // *[a-z0-9] -> [a-z0-9]+
    // *\D -> \D+
    // *\s -> \s+  ...
    Ok((input, format!("{pattern}+")))
}

fn parse_format_pattern(input: &str) -> IResult<&str, String> {
    let (input, c) = anychar(input)?;
    if let Some(pattern) = find_efm_format_pattern(c) {
        Ok((input, pattern.to_owned()))
    } else {
        Err(nom::Err::Error(NomError::new(input, ErrorKind::Char)))
    }
}

// These char is escaped by %, like %% %. %$ which would use as regex char
fn parse_special_char(input: &str) -> IResult<&str, String> {
    let (input, c) = one_of("%.^$?+[#")(input)?;
    if c == '#' {
        // this is the special case, %# -> *
        Ok((input, '*'.to_string()))
    } else {
        Ok((input, c.to_string()))
    }
}

/// parse pattern like [^a-z0-9]
fn parse_bracket_pattern(input: &str) -> IResult<&str, String> {
    let (input, _) = char('[')(input)?;
    let (input, content) = take_while1(|c| c != ']')(input)?;
    let (input, _) = char(']')(input)?;
    Ok((input, format!("[{content}]")))
}

fn parse_escape_pattern(input: &str) -> IResult<&str, String> {
    let (input, matched_slice) = recognize(pair(char('\\'), anychar)).parse(input)?;
    Ok((input, matched_slice.to_owned()))
}

fn parse_escaped_char(input: &str) -> IResult<&str, String> {
    let (input, _) = char('\\')(input)?;
    let (input, c) = anychar(input)?;
    Ok((input, c.to_string()))
}

fn parse_normal_char(input: &str) -> IResult<&str, String> {
    // we should not parse %
    let (input, c) = satisfy(|c| c != '%')(input)?;
    if ".+*()|[{^$".contains(c) {
        Ok((input, format!("\\{c}")))
    } else {
        Ok((input, c.to_string()))
    }
}
