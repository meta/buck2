/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write as _;
use std::mem;
use std::slice;
use std::vec;

use crossterm::Command;
use crossterm::cursor::MoveToColumn;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::Span;
use crate::SpanError;
use crate::vec_as_fmt_write::VecAsFmtWrite;

/// A `Line` is an abstraction for a collection of stylized or unstylized strings.
/// Since each `Span` denotes a portion of a single line, an ordered collection represents a single line of text.
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct Line(
    /// Sequence is normalized.
    /// * All spans are non-empty.
    /// * Adjacent spans have different styles.
    Vec<Span>,
);

impl Line {
    pub fn unstyled(text: &str) -> Result<Line, SpanError> {
        Ok(Line::from_iter([Span::new_unstyled(text)?]))
    }

    pub fn sanitized(text: &str) -> Line {
        Line::from_iter([Span::sanitized(text)])
    }

    /// Return the length of the all words in the line added together.
    pub fn len(&self) -> usize {
        self.0.iter().map(Span::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Adds padding to the right side of the line.
    /// This adds a new unstyled word consisting entirely of the appropriate number of spaces.
    /// If no padding is requested, then no word is added.
    pub fn pad_right(&mut self, amount: usize) {
        self.push(Span::padding(amount));
    }

    /// Same behavior as pad right, but the pad is on the left
    pub fn pad_left(&mut self, amount: usize) {
        self.push_front(Span::padding(amount));
    }

    /// Truncates the right side of the line until it is no wider than `max_width` columns.
    /// This will delete words entirely if they cannot fit. A grapheme that straddles the limit
    /// is dropped, so the result may be narrower than `max_width` but is never wider.
    /// If the line is padded to 0, then it will become an empty line.
    pub fn truncate_line(&mut self, max_width: usize) {
        let mut cur_width = 0;

        for (index, span) in self.0.iter_mut().enumerate() {
            if cur_width >= max_width {
                self.0.truncate(index);
                break;
            }
            let word_width = span.len();
            // if the line is going to overflow
            if word_width + cur_width > max_width {
                let mut kept_width = cur_width;
                let word: String = span
                    .content
                    .graphemes(true)
                    // keep whole graphemes while they fit in the remaining columns
                    .take_while(|grapheme| {
                        let width = grapheme.width();
                        let fits = kept_width + width <= max_width;
                        if fits {
                            kept_width += width;
                        }
                        fits
                    })
                    .collect();

                // overwrite the current word, or drop it when nothing of it fits
                let keep = !word.is_empty();
                span.content = word.into();

                // drop the remaining words
                self.0.truncate(if keep { index + 1 } else { index });

                break;
            }
            cur_width += word_width;
        }
    }

    /// Slices out some middle subline of the Line. Removes the first `start` characters and
    /// keeps `width` characters after that.
    pub fn trim_ends(&mut self, mut start: usize, mut width: usize) {
        let mut owned = Vec::new();
        std::mem::swap(&mut owned, &mut self.0);

        for mut span in owned.into_iter() {
            // TODO(cjhopman): Other code here uses a mix of graphemes count and span.len() for computing the length of
            // a span, but these are totally different approaches. span.len() supposedly uses a more accurate approach,
            // so we should consider switching to that throughout. But, to keep this code self-consistent it uses only
            // the graphemes approach.
            let chars = span.content.graphemes(true);
            let len = chars.clone().count();

            if start > 0 && len < start {
                start -= len;
                continue;
            }

            let end = std::cmp::min(len, start + width);

            if start != 0 || end != len {
                span.content = chars.skip(start).take(end - start).collect();
            }
            self.push(span);

            width -= end - start;
            start = 0;

            if width == 0 {
                break;
            }
        }
    }

    /// Either calls [`pad_right`](Line::pad_right) or [`truncate_line`](Line::truncate_line) until the line is the exact width specified.
    /// This call acts on the right side of the `Line`.
    pub fn to_exact_width(&mut self, exact_width: usize) {
        if self.len() > exact_width {
            self.truncate_line(exact_width);
        }
        // Truncation drops a straddling grapheme, so the line may now need padding.
        let len = self.len();
        if len < exact_width {
            self.pad_right(exact_width - len);
        }
    }

    /// Renders the formatted content of the line to `stdout`.
    /// The buffer must be flushed to produce output.
    pub(crate) fn render_with_clear_and_nl(&self, writer: &mut Vec<u8>) {
        let mut writer = VecAsFmtWrite(writer);

        for word in &self.0 {
            word.render(&mut writer).unwrap();
        }

        Clear(ClearType::UntilNewLine)
            .write_ansi(&mut writer)
            .unwrap();
        writeln!(writer).unwrap();
        MoveToColumn(0).write_ansi(&mut writer).unwrap();
    }

    /// Render the line as a string with ANSI escape codes.
    ///
    /// Without trailing newline or an escape sequence to clear the line.
    pub fn render(&self) -> String {
        let mut result = String::new();
        for word in &self.0 {
            word.render(&mut result)
                .expect("writing to a string should not fail");
        }
        result
    }

    /// Iterate over the spans in the line.
    pub fn iter(&self) -> slice::Iter<'_, Span> {
        self.0.iter()
    }

    /// Concatenate spans, discarding any styling.
    pub fn to_unstyled(&self) -> String {
        self.0.iter().map(|span| span.content.as_ref()).collect()
    }

    /// Append a span to the line.
    pub fn push(&mut self, span: Span) {
        if span.is_empty() {
            return;
        }
        if let Some(last) = self.0.last_mut() {
            if last.is_mergeable_with(&span) {
                last.content.to_mut().push_str(&span.content);
                return;
            }
        }
        self.0.push(span);
    }

    /// Prepend a span to the line.
    pub fn push_front(&mut self, span: Span) {
        let this = mem::take(self);
        self.push(span);
        self.extend(this);
    }

    pub fn fmt_for_test(&self) -> impl Display + '_ {
        struct Impl<'a>(&'a Line);
        impl Display for Impl<'_> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                for span in &self.0.0 {
                    write!(f, "{}", span.fmt_for_test())?;
                }
                Ok(())
            }
        }
        Impl(self)
    }
}

/// Iterate spans in a line.
impl IntoIterator for Line {
    type Item = Span;
    type IntoIter = vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromIterator<Span> for Line {
    fn from_iter<T: IntoIterator<Item = Span>>(iter: T) -> Self {
        let mut line = Line::default();
        line.extend(iter);
        line
    }
}

impl TryFrom<Vec<String>> for Line {
    type Error = SpanError;

    fn try_from(other: Vec<String>) -> Result<Self, Self::Error> {
        other
            .into_iter()
            .map(Span::new_unstyled)
            .collect::<Result<Line, SpanError>>()
    }
}

impl TryFrom<Vec<&str>> for Line {
    type Error = SpanError;

    fn try_from(other: Vec<&str>) -> Result<Self, Self::Error> {
        other
            .into_iter()
            .map(Span::new_unstyled)
            .collect::<Result<Line, SpanError>>()
    }
}

impl Extend<Span> for Line {
    fn extend<T: IntoIterator<Item = Span>>(&mut self, iter: T) {
        for span in iter {
            self.push(span);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::style::Color;

    use super::*;

    #[test]
    fn test_words_len() {
        let normal = Line::from_iter([
            Span::new_colored("test", Color::Black).unwrap(),
            Span::new_colored("hello", Color::Blue).unwrap(),
            Span::new_colored("world", Color::Black).unwrap(),
        ]);

        assert_eq!(normal.len(), 14);

        assert_eq!(Line::default().len(), 0);
    }

    #[test]
    fn test_pad_line_right() {
        let mut test = Line::from_iter([
            Span::new_colored("test", Color::DarkBlue).unwrap(),
            Span::new_colored("ok", Color::DarkCyan).unwrap(),
        ]);
        let mut new_test: Line = test.clone();
        test.push(" ".repeat(4).try_into().unwrap());
        new_test.pad_right(4);
        assert_eq!(test, new_test);

        new_test.pad_right(6);
        test.push(" ".repeat(6).try_into().unwrap());
        assert_eq!(test, new_test);

        new_test.pad_right(10);
        test.push(" ".repeat(10).try_into().unwrap());
        assert_eq!(test, new_test);
    }

    #[test]
    fn test_pad_line_left() {
        let mut test: Line = Line::from_iter([
            Span::new_colored("test", Color::DarkCyan).unwrap(),
            Span::new_colored("ok", Color::Cyan).unwrap(),
        ]);
        let mut new_test: Line = test.clone();
        test.push_front(" ".repeat(4).try_into().unwrap());
        new_test.pad_left(4);
        assert_eq!(test, new_test);

        new_test.pad_left(6);
        test.push_front(" ".repeat(6).try_into().unwrap());
        assert_eq!(test, new_test);

        new_test.pad_left(10);
        test.push_front(" ".repeat(10).try_into().unwrap());
        assert_eq!(test, new_test);
    }

    #[test]
    fn test_truncate_line() {
        let mut test: Line = Line::from_iter([
            Span::new_colored("test", Color::Blue).unwrap(),
            Span::new_colored("ok", Color::Red).unwrap(),
        ]);
        let mut new_test: Line = test.clone();
        test.truncate_line(10);
        assert_eq!(test, new_test);

        new_test.truncate_line(6);
        assert_eq!(test, new_test);

        new_test.truncate_line(5);
        test.0.pop().unwrap();
        test.push(Span::new_colored("o", Color::Red).unwrap());
        assert_eq!(test, new_test);

        new_test.truncate_line(4);
        test.0.pop().unwrap();
        assert_eq!(test, new_test);

        new_test.truncate_line(0);
        assert_eq!(new_test, Line::default());
    }

    #[test]
    fn test_trim_ends() {
        let line = |spans: &[&str]| -> Line { spans.to_vec().try_into().unwrap() };
        let mut test = line(&["hello", "cat", "world"]);
        test.trim_ends(0, 15);
        assert_eq!(test, line(&["hello", "cat", "world"]));

        test.trim_ends(0, 13);
        assert_eq!(test, line(&["hello", "cat", "world"]));

        let mut test = line(&["hello", "cat", "world"]);
        test.trim_ends(2, 10);
        assert_eq!(test, line(&["llo", "cat", "worl"]));

        let mut test = line(&["hello", "cat", "world"]);
        test.trim_ends(6, 2);
        assert_eq!(test, line(&["at"]));

        let mut test = line(&["hello", "cat", "world"]);
        test.trim_ends(9, 2);
        assert_eq!(test, line(&["or"]));
    }

    #[test]
    fn test_push_collapses() {
        let mut line = Line::default();
        line.push(Span::new_colored("ab", Color::Cyan).unwrap());
        line.push(Span::new_colored("c", Color::Cyan).unwrap());
        line.push(Span::new_colored("d", Color::Red).unwrap());

        let expected = Line::from_iter([
            Span::new_colored("abc", Color::Cyan).unwrap(),
            Span::new_colored("d", Color::Red).unwrap(),
        ]);

        assert_eq!(expected, line);
    }

    #[test]
    fn test_push_front() {
        let mut line = Line::default();
        line.push_front(Span::new_colored("d", Color::Cyan).unwrap());
        line.push_front(Span::new_colored("c", Color::Cyan).unwrap());
        line.push_front(Span::new_colored("ab", Color::Red).unwrap());

        let expected = Line::from_iter([
            Span::new_colored("ab", Color::Red).unwrap(),
            Span::new_colored("cd", Color::Cyan).unwrap(),
        ]);

        assert_eq!(expected, line);
    }

    #[test]
    fn test_fmt_for_test() {
        let line = Line::from_iter([
            Span::new_colored("abra", Color::Blue).unwrap(),
            Span::new_colored("cadabra", Color::Red).unwrap(),
        ]);

        assert_eq!(
            "<span fg=blue>abra</span><span fg=red>cadabra</span>",
            format!("{}", line.fmt_for_test())
        );
    }
    #[test]
    fn test_truncate_line_wide_chars() {
        let mut line = Line::unstyled("\u{6771}\u{6771}\u{6771}").unwrap();
        assert_eq!(line.len(), 6);
        line.truncate_line(2);
        assert_eq!(line.to_unstyled(), "\u{6771}");
        assert_eq!(line.len(), 2);

        // A wide character that straddles the limit is dropped.
        let mut line = Line::unstyled("a\u{6771}").unwrap();
        line.truncate_line(2);
        assert_eq!(line.to_unstyled(), "a");
        assert_eq!(line.len(), 1);

        let mut line = Line::from_iter([
            Span::new_colored("a", Color::Red).unwrap(),
            Span::new_unstyled("\u{6771}\u{6771}").unwrap(),
        ]);
        line.truncate_line(2);
        assert_eq!(line.to_unstyled(), "a");
        assert_eq!(line.len(), 1);
    }

    #[test]
    fn test_truncate_line_zero_width_chars() {
        let mut line = Line::unstyled("a\u{200b}bc").unwrap();
        assert_eq!(line.len(), 3);
        line.truncate_line(2);
        assert_eq!(line.to_unstyled(), "a\u{200b}b");
        assert_eq!(line.len(), 2);
    }

    #[test]
    fn test_to_exact_width_wide_chars() {
        let mut line = Line::unstyled("\u{6771}\u{6771}").unwrap();
        line.to_exact_width(3);
        assert_eq!(line.to_unstyled(), "\u{6771} ");
        assert_eq!(line.len(), 3);
    }
}
