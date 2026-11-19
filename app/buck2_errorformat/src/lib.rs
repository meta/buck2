/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Cell;
use std::cell::RefCell;

use crate::efm::MultiLineType;
use crate::efm::ParseEfmError;
use crate::efm::SingleErrorFormatRule;

#[cfg(test)]
mod e2e_tests;
pub mod efm;
#[cfg(test)]
mod efm_tests;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum ErrorFormatError {
    #[error("Parse error: {0}")]
    Parse(ParseEfmError),
    // Boxed to reduce enum size and avoid clippy::result_large_err warning
    #[error("Regex error: {0}")]
    Regex(Box<fancy_regex::Error>),
    #[error("Unsupported format specifier: {specifier:?} in state {state}")]
    UnsupportedSpecifier { specifier: String, state: String },
}

impl From<ParseEfmError> for ErrorFormatError {
    fn from(err: ParseEfmError) -> Self {
        ErrorFormatError::Parse(err)
    }
}

impl From<fancy_regex::Error> for ErrorFormatError {
    fn from(err: fancy_regex::Error) -> Self {
        ErrorFormatError::Regex(Box::new(err))
    }
}

/// Represents a parsed error format entry
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Name of the file
    pub filename: Option<String>,
    /// Line number
    pub lnum: Option<usize>,
    /// End line number for multiline entries
    pub end_lnum: Option<usize>,
    /// Column number
    pub col: Option<usize>,
    /// End column number for range entries
    pub end_col: Option<usize>,
    /// Description of the error
    pub message: Option<String>,
    /// Type of the error ('error', 'warning', 'info', etc.)
    pub error_type: Option<String>,
    /// Error number, e.g. 404, 500, etc.
    pub error_number: Option<usize>,
    /// the "rest" of a single-line file message
    pub rest: Option<String>,
    /// pointer line, the line usually show under the error line to point out the specific
    /// location, e.g. ^ or ~~~~. Used to get the specific column, but here right now just store as
    /// a string
    pub pointer_line: Option<String>,
    /// Matches the error's virtual column, which is the visual column position considering tab character expansion.
    pub virtual_col: Option<usize>,
    // Matches a string from the error output that Vim then uses to search for the exact error line within the specified file.
    pub search_text: Option<String>,
    // Original error lines (often one line. more than one line for multi-line
    // errorformat. :h errorformat-multi-line)
    pub original_err_lines: Vec<String>,
}

impl Entry {
    /// Creates a new Entry with default values
    pub fn new() -> Self {
        Entry::default()
    }

    fn combine(&self, other: &Self) -> Self {
        Entry {
            filename: other.filename.clone().or_else(|| self.filename.clone()),
            lnum: other.lnum.or(self.lnum),
            end_lnum: other.end_lnum.or(self.end_lnum),
            col: other.col.or(self.col),
            end_col: other.end_col.or(self.end_col),
            message: match (&self.message, &other.message) {
                (Some(self_msg), Some(other_msg)) => Some(format!("{self_msg}\n{other_msg}")),
                _ => other.message.clone().or_else(|| self.message.clone()),
            },
            error_type: other.error_type.clone().or_else(|| self.error_type.clone()),
            error_number: other.error_number.or(self.error_number),
            rest: other.rest.clone().or_else(|| self.rest.clone()),
            pointer_line: other
                .pointer_line
                .clone()
                .or_else(|| self.pointer_line.clone()),
            virtual_col: other.virtual_col.or(self.virtual_col),
            search_text: other
                .search_text
                .clone()
                .or_else(|| self.search_text.clone()),
            original_err_lines: {
                let mut lines = self.original_err_lines.clone();
                lines.extend(other.original_err_lines.clone());
                lines
            },
        }
    }
}

/// Main error format structure that holds multiple error format patterns
#[derive(Debug)]
pub struct ErrorFormat {
    /// List of error format patterns
    efms: Vec<SingleErrorFormatRule>,
}

impl ErrorFormat {
    /// Creates a new ErrorFormat with the given error format patterns
    pub fn new(efms: Vec<String>) -> Result<Self, ParseEfmError> {
        let mut error_format = ErrorFormat {
            efms: Vec::with_capacity(efms.len()),
        };

        for efm in efms {
            let e = SingleErrorFormatRule::new(&efm)?;
            tracing::debug!(
                errorformat = %efm,
                regex = %e.regex,
                format_specifier = ?e.format_specifier,
                "Parsed error format to regex"
            );
            error_format.efms.push(e);
        }

        Ok(error_format)
    }

    fn parse_line_with_state(
        &self,
        line: &str,
        state: &dyn ParseMode,
    ) -> Result<Option<(Entry, &SingleErrorFormatRule)>, ErrorFormatError> {
        for efm in &self.efms {
            if let Some(mut entry) = state.parse_line_with_one_efm(line, efm)? {
                entry.filename = state.current_file(entry.filename.as_deref());
                if efm
                    .format_specifier
                    .as_ref()
                    .is_some_and(|s| s.capture_entire_line_as_message())
                {
                    entry.message = Some(line.to_owned());
                }
                return Ok(Some((entry, efm)));
            }
        }
        Ok(None)
    }
}

#[derive(Debug, Clone)]
enum ParseLineResult {
    Valid(Entry),
    Invalid(Entry),
    // the line is on multi-line, but not complete
    MatchingMultiLine {
        // The entry that is combined from the previous multi-line parsing,
        // It is not None if the previous if the previous state is also a multi-line state, and current enter a new multi-line state
        // It is None if the previous state is a single-line state, and current enter a new multi-line state
        valid_entry: Option<Entry>,
        // The current multi-line entry that is not complete
        // It is boxed to avoid inflating the overall enum size (since Entry is large),
        matching_entry: Box<Entry>,
    },
    Ignore(Entry),
    // A plain rule matched while a multi-line message was open: the message so far, if any,
    // then the single-line entry.
    ValidMany(Vec<Entry>),
    // Not match any pattern, but we can still return the entry if the line is on multi-line, if the entry is available.
    NoMatch(Option<Entry>),
}

impl ParseLineResult {
    /// Converts a parse result to Ignore if it matches certain conditions
    /// This is useful when we want to ignore certain entries while preserving the Entry data
    fn mark_as_ignored(self) -> Self {
        match self {
            Self::Valid(entry) => Self::Ignore(entry),
            Self::Invalid(entry) => Self::Ignore(entry),
            Self::MatchingMultiLine {
                valid_entry,
                matching_entry,
            } => Self::Ignore(valid_entry.unwrap_or(*matching_entry)),
            other => other,
        }
    }
}

// --- State Pattern ---
trait ParseMode {
    fn parse_line(
        self: Box<Self>,
        line: &str,
        error_format: &ErrorFormat,
    ) -> (Box<dyn ParseMode>, ParseLineResult);

    fn parse_line_with_one_efm(
        &self,
        line: &str,
        efm: &SingleErrorFormatRule,
    ) -> Result<Option<Entry>, ErrorFormatError>;

    fn shared_context(&self) -> &RefCell<ShareContext>;

    fn pop_file(&self) -> Option<String> {
        self.shared_context().borrow_mut().pop_file()
    }

    fn push_file(&self, file: String) {
        self.shared_context().borrow_mut().push_file(file);
    }

    fn pop_dir(&self) -> Option<String> {
        self.shared_context().borrow_mut().pop_dir()
    }

    fn push_dir(&self, dir: String) {
        self.shared_context().borrow_mut().push_dir(dir);
    }

    fn current_file(&self, current_parsed_file: Option<&str>) -> Option<String> {
        self.shared_context()
            .borrow()
            .current_file(current_parsed_file)
            .map(|s| s.to_owned())
    }

    // pop the valid entry if the current state still contains valid entry
    // In multi-line state, we will pop the valid entry if the current state have valid entries
    fn pop_valid_entry(&mut self) -> Option<Entry> {
        None
    }
}

#[derive(Default)]
struct SingleLineState(RefCell<ShareContext>);

struct MultiLineState {
    multi_line_type: MultiLineType,
    prev_entries: Vec<Entry>,
    ctx: RefCell<ShareContext>,
    // it is multi-line ignore flag
    ignore: Cell<bool>,
}

impl MultiLineState {
    fn combine_entries(&mut self) -> Entry {
        let mut entry = Entry::new();
        for e in self.prev_entries.iter() {
            entry = entry.combine(e);
        }
        // We respect the error_type from the entery multi-line type specifier
        entry.error_type = Some(self.multi_line_type.error_char().to_string());
        self.prev_entries.clear();
        entry
    }
}

impl ParseMode for SingleLineState {
    fn parse_line(
        self: Box<Self>,
        line: &str,
        error_format: &ErrorFormat,
    ) -> (Box<dyn ParseMode>, ParseLineResult) {
        if let Ok(Some((entry, efm))) = error_format.parse_line_with_state(line, self.as_ref()) {
            let (next_state, result) = match efm.format_specifier {
                None => (self as Box<dyn ParseMode>, ParseLineResult::Valid(entry)),
                Some(ref specifier) if specifier.is_generic_message() => {
                    (self as Box<dyn ParseMode>, ParseLineResult::Valid(entry))
                }
                Some(ref specifier) if specifier.is_enter_multi_line() => {
                    let multi_line_type = specifier.enter_multi_line_type().unwrap();
                    let state = MultiLineState {
                        multi_line_type,
                        prev_entries: vec![entry.clone()],
                        ctx: self.0,
                        ignore: Cell::new(specifier.is_ignore_multi_line()),
                    };
                    (
                        Box::new(state) as Box<dyn ParseMode>,
                        ParseLineResult::MatchingMultiLine {
                            valid_entry: None,
                            matching_entry: Box::new(entry),
                        },
                    )
                }
                // Cannot happen in this case now, since we just ignore in parse_line_with_one_efm
                // Some(ref specifier) if specifier.is_end_multi_line() => {
                // }
                Some(ref specifier) if specifier.is_push_file_on_stack() => {
                    if let Some(file) = &entry.filename {
                        self.push_file(file.clone());
                    }
                    (self as Box<dyn ParseMode>, ParseLineResult::Invalid(entry))
                }
                Some(ref specifier) if specifier.is_pop_file_on_stack() => {
                    self.pop_file();
                    (self as Box<dyn ParseMode>, ParseLineResult::Invalid(entry))
                }
                Some(ref specifier) if specifier.is_push_dir_on_stack() => {
                    if let Some(dir) = &entry.filename {
                        self.push_dir(dir.clone());
                    }
                    (self as Box<dyn ParseMode>, ParseLineResult::Invalid(entry))
                }
                Some(ref specifier) if specifier.is_pop_dir_on_stack() => {
                    self.pop_dir();
                    (self as Box<dyn ParseMode>, ParseLineResult::Invalid(entry))
                }
                Some(ref specifier) if specifier.is_overread_matched_part() => {
                    (self as Box<dyn ParseMode>, ParseLineResult::Invalid(entry))
                }
                _ => {
                    // Unsupported format specifier in single line state
                    // Log the issue for debugging but don't panic
                    eprintln!(
                        "Warning: Unsupported format specifier {:?} in SingleLineState for line: {}",
                        efm.format_specifier, line
                    );
                    (self as Box<dyn ParseMode>, ParseLineResult::NoMatch(None))
                }
            };
            if efm
                .format_specifier
                .as_ref()
                .is_some_and(|s| s.is_ignore_multi_line())
            {
                (next_state, result.mark_as_ignored())
            } else {
                (next_state, result)
            }
        } else {
            (self as Box<dyn ParseMode>, ParseLineResult::NoMatch(None))
        }
    }

    fn parse_line_with_one_efm(
        &self,
        line: &str,
        efm: &SingleErrorFormatRule,
    ) -> Result<Option<Entry>, ErrorFormatError> {
        match efm.format_specifier {
            Some(ref specifier)
                if specifier.is_enter_multi_line()
                    || specifier.is_push_file_on_stack()
                    || specifier.is_pop_file_on_stack()
                    || specifier.is_push_dir_on_stack()
                    || specifier.is_pop_dir_on_stack()
                    || specifier.is_generic_message() =>
            {
                efm.match_line(line).map_err(ErrorFormatError::from)
            }
            Some(ref specifier) if specifier.is_continuation_multi_line() => {
                // Current state is SingleLineState, if efm is a continuation multi-line, we just ignore this efm
                Ok(None)
            }
            Some(ref specifier) if specifier.is_end_multi_line() => {
                // Current state is SingleLineState, if efm is a end multi-line, we just ignore this efm
                Ok(None)
            }
            None => efm.match_line(line).map_err(ErrorFormatError::from),
            _ => Err(ErrorFormatError::UnsupportedSpecifier {
                specifier: format!("{:?}", efm.format_specifier),
                state: "SingleLineState".to_owned(),
            }),
        }
    }

    fn shared_context(&self) -> &RefCell<ShareContext> {
        &self.0
    }
}

impl ParseMode for MultiLineState {
    fn parse_line(
        mut self: Box<Self>,
        line: &str,
        error_format: &ErrorFormat,
    ) -> (Box<dyn ParseMode>, ParseLineResult) {
        if let Ok(Some((entry, efm))) = error_format.parse_line_with_state(line, self.as_ref()) {
            // The open message's own flag decides whether it is emitted; a `-` rule excludes
            // only the line it matched.
            let ignore = self.ignore.get();
            let line_ignored = efm
                .format_specifier
                .as_ref()
                .is_some_and(|s| s.is_ignore_multi_line());
            let (next_state, result) = match efm.format_specifier {
                Some(ref specifier) if specifier.is_enter_multi_line() => {
                    // New a new state for multi-line
                    let multi_line_type = specifier.enter_multi_line_type().unwrap();
                    let prev_muti_line_entry = if self.prev_entries.is_empty() {
                        None
                    } else {
                        Some(self.combine_entries())
                    };
                    let state = MultiLineState {
                        multi_line_type,
                        prev_entries: vec![entry.clone()],
                        ctx: self.ctx,
                        ignore: Cell::new(specifier.is_ignore_multi_line()),
                    };
                    (
                        Box::new(state) as Box<dyn ParseMode>,
                        ParseLineResult::MatchingMultiLine {
                            valid_entry: prev_muti_line_entry,
                            matching_entry: Box::new(entry),
                        },
                    )
                }
                Some(ref specifier) if specifier.is_continuation_multi_line() => {
                    self.prev_entries.push(entry.clone());
                    (
                        self as Box<dyn ParseMode>,
                        ParseLineResult::MatchingMultiLine {
                            valid_entry: None,
                            matching_entry: Box::new(entry),
                        },
                    )
                }
                Some(ref specifier) if specifier.is_end_multi_line() => {
                    self.prev_entries.push(entry);
                    let final_entry = self.combine_entries();
                    let result = if line_ignored {
                        ParseLineResult::Ignore(final_entry)
                    } else {
                        ParseLineResult::Valid(final_entry)
                    };
                    (
                        Box::new(SingleLineState(self.ctx)) as Box<dyn ParseMode>,
                        result,
                    )
                }
                Some(ref specifier) if specifier.is_generic_message() => {
                    let result = if line_ignored {
                        ParseLineResult::Ignore(entry)
                    } else {
                        ParseLineResult::Valid(entry)
                    };
                    (self as Box<dyn ParseMode>, result)
                }
                None => {
                    // A plain rule ends the open message and stands on its own.
                    let mut entries = Vec::new();
                    if !ignore && !self.prev_entries.is_empty() {
                        entries.push(self.combine_entries());
                    }
                    entries.push(entry);
                    (
                        Box::new(SingleLineState(self.ctx)) as Box<dyn ParseMode>,
                        ParseLineResult::ValidMany(entries),
                    )
                }
                _ => {
                    // Unsupported format specifier in multi-line state
                    // Log the issue for debugging but don't panic
                    eprintln!(
                        "Warning: Unsupported format specifier {:?} in MultiLineState",
                        efm.format_specifier
                    );
                    (
                        Box::new(SingleLineState(self.ctx)) as Box<dyn ParseMode>,
                        ParseLineResult::NoMatch(None),
                    )
                }
            };
            if ignore {
                (next_state, result.mark_as_ignored())
            } else {
                (next_state, result)
            }
        } else {
            // Exit MultiLineState when line doesn't match any pattern, same behavior in vim/neovim implementation
            // This allows the multi-line parsing to continue waiting for continuation or end patterns
            if !self.prev_entries.is_empty() && !self.ignore.get() {
                let entry = self.combine_entries();
                (
                    Box::new(SingleLineState(self.ctx)) as Box<dyn ParseMode>,
                    ParseLineResult::NoMatch(Some(entry)),
                )
            } else {
                (
                    Box::new(SingleLineState(self.ctx)) as Box<dyn ParseMode>,
                    ParseLineResult::NoMatch(None),
                )
            }
        }
    }

    fn parse_line_with_one_efm(
        &self,
        line: &str,
        efm: &SingleErrorFormatRule,
    ) -> Result<Option<Entry>, ErrorFormatError> {
        match efm.format_specifier {
            Some(ref specifier) if specifier.is_continuation_multi_line() => {
                efm.match_line(line).map_err(ErrorFormatError::from)
            }
            Some(ref specifier) if specifier.is_end_multi_line() => {
                efm.match_line(line).map_err(ErrorFormatError::from)
            }
            Some(ref specifier) if specifier.is_enter_multi_line() => {
                // Current state is MultiLineState, if efm is a enter multi-line, we just parse it
                efm.match_line(line).map_err(ErrorFormatError::from)
            }
            Some(ref specifier)
                if specifier.is_generic_message()
                    || specifier.is_push_file_on_stack()
                    || specifier.is_pop_file_on_stack()
                    || specifier.is_push_dir_on_stack()
                    || specifier.is_pop_dir_on_stack() =>
            {
                efm.match_line(line).map_err(ErrorFormatError::from)
            }
            None => efm.match_line(line).map_err(ErrorFormatError::from),
            _ => Err(ErrorFormatError::UnsupportedSpecifier {
                specifier: format!("{:?}", efm.format_specifier),
                state: "MultiLineState".to_owned(),
            }),
        }
    }

    fn shared_context(&self) -> &RefCell<ShareContext> {
        &self.ctx
    }

    fn pop_valid_entry(&mut self) -> Option<Entry> {
        if self.prev_entries.is_empty() || self.ignore.get() {
            None
        } else {
            Some(self.combine_entries())
        }
    }
}

#[derive(Default)]
struct ShareContext {
    file_stack: Vec<String>,
    dir_stack: Vec<String>,
}

impl ShareContext {
    fn push_file(&mut self, file: String) {
        self.file_stack.push(file);
    }

    fn pop_file(&mut self) -> Option<String> {
        self.file_stack.pop()
    }

    fn push_dir(&mut self, dir: String) {
        self.dir_stack.push(dir);
    }

    fn pop_dir(&mut self) -> Option<String> {
        self.dir_stack.pop()
    }

    fn current_file(&self, current_parsed_file: Option<&str>) -> Option<String> {
        let file_path = current_parsed_file.or(self.file_stack.last().map(|s| s.as_str()));
        if let Some(file) = file_path {
            if std::path::Path::new(file).is_absolute() {
                Some(file.to_owned())
            } else if let Some(dir) = self.dir_stack.last() {
                let path = std::path::Path::new(dir).join(file);
                Some(path.to_string_lossy().into_owned())
            } else {
                Some(file.to_owned())
            }
        } else {
            self.dir_stack.last().map(|s| s.to_owned())
        }
    }
}

struct ErrorFormatParser {
    state: Box<dyn ParseMode>,
    error_format: ErrorFormat,
}

impl ErrorFormatParser {
    fn new(error_format: ErrorFormat) -> Self {
        ErrorFormatParser {
            state: Box::new(SingleLineState::default()),
            error_format,
        }
    }

    fn parse_line(&mut self, line: &str) -> ParseLineResult {
        // Use mem::replace to temporarily move state out for parsing
        let state = std::mem::replace(&mut self.state, Box::new(SingleLineState::default()));
        let (next_state, result) = state.parse_line(line, &self.error_format);
        self.state = next_state;
        result
    }

    fn pop_valid_entry(&mut self) -> Option<Entry> {
        self.state.pop_valid_entry()
    }
}

pub fn parse_error_format(
    error_format: Vec<String>,
    lines: Vec<String>,
) -> Result<Vec<Entry>, ErrorFormatError> {
    let error_format = ErrorFormat::new(error_format)?;
    let mut parser = ErrorFormatParser::new(error_format);
    let mut entries = Vec::new();
    for line in lines {
        let result = parser.parse_line(&line);
        match result {
            ParseLineResult::Valid(entry) => entries.push(entry),
            ParseLineResult::ValidMany(many) => entries.extend(many),
            ParseLineResult::MatchingMultiLine {
                valid_entry,
                matching_entry: _,
            } => {
                if let Some(entry) = valid_entry {
                    entries.push(entry);
                }
            }
            ParseLineResult::Invalid(_entry) => {}
            ParseLineResult::Ignore(_entry) => {}
            ParseLineResult::NoMatch(entry) => {
                // The case when we have the entry available is when in multi-line state, but the following line don't match any pattern
                if let Some(entry) = entry {
                    entries.push(entry);
                }
            }
        }
    }
    // pop the valid entry if the current state still contains valid entry
    if let Some(entry) = parser.pop_valid_entry() {
        entries.push(entry);
    }
    // parser.state
    Ok(entries)
}

/// Split a string into lines, handling both Unix and Windows line endings
pub fn split_lines(input: &str) -> Vec<String> {
    input.lines().map(|s| s.to_owned()).collect()
}

/// Split an errorformat string by commas, ignoring whitespace after commas
/// For example: "%f:%l,%t:%m, %r" -> ["%f:%l", "%t:%m", "%r"]
#[allow(dead_code)]
pub fn split_errorformat(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn test_split_lines() {
        let input = "line1\nline2\r\nline3";
        let lines = split_lines(input);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_split_errorformat() {
        let input = "%f:%l, %t:%m,  %r";
        let parts = split_errorformat(input);
        assert_eq!(parts, vec!["%f:%l", "%t:%m", "%r"]);
    }

    #[test]
    fn test_split_errorformat_with_blanks() {
        let input = "%f:%l,   %t:%m,\t%r,    %n";
        let parts = split_errorformat(input);
        assert_eq!(parts, vec!["%f:%l", "%t:%m", "%r", "%n"]);

        let input_with_empty = "%f:%l,,   ,%t:%m,  ,  %r";
        let parts = split_errorformat(input_with_empty);
        assert_eq!(parts, vec!["%f:%l", "%t:%m", "%r"]);
    }
}
