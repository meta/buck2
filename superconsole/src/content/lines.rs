/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp;
use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::io::Write;
use std::iter;
use std::mem;

use anstyle_parse::DefaultCharAccumulator;
use anstyle_parse::Params;
use anstyle_parse::Parser;
use anstyle_parse::Perform;
use crossterm::style::Attribute;
use crossterm::style::Attributes;
use crossterm::style::Color;
use itertools::Itertools;

use crate::Dimensions;
use crate::Hyperlink;
use crate::Line;
use crate::Span;
use crate::style::ContentStyle;
use crate::style::StyledContent;

#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct Lines(pub Vec<Line>);

/// State container that's used to parse strings with ANSI color codes in them.
#[derive(Default)]
struct ColoredStringParser {
    foreground_color: Option<Color>,
    background_color: Option<Color>,
    underline_color: Option<Color>,
    attributes: Attributes,
    line_buffer: String,
    hyperlink: Option<Hyperlink>,
    spans: Vec<Span>,
}

impl ColoredStringParser {
    fn push_current(&mut self) {
        let sc = StyledContent::new(
            ContentStyle {
                foreground_color: self.foreground_color,
                background_color: self.background_color,
                underline_color: self.underline_color,
                attributes: self.attributes,
            },
            std::mem::take(&mut self.line_buffer),
        );
        let mut span = Span::new_styled_lossy(sc);
        if self.hyperlink.is_some() {
            span = span.with_hyperlink(self.hyperlink.clone());
        }
        self.spans.push(span);
    }

    /// Given a line w/ some ANSI encoded color characters, turn it into a list of spans.
    fn parse_line(&mut self, parser: &mut Parser, s: &str) -> Line {
        // Because we only stick "printable" characters onto the buffer, and skip any other
        // class of characters that we don't recognize, this should be a
        // roughly sanitized string. We do one more pass when we create the Span, just to
        // make sure, but that is a pretty fast check.
        for byte in s.bytes() {
            parser.advance(self, byte);
        }
        self.push_current();
        Line::from_iter(mem::take(&mut self.spans))
    }

    fn set_intensity(&mut self, attribute: Option<Attribute>) {
        self.push_current();
        self.attributes = attribute.map(Attributes::from).unwrap_or_default();
    }

    fn set_foreground(&mut self, color: Option<Color>) {
        self.push_current();
        self.foreground_color = color;
    }

    fn set_background(&mut self, color: Option<Color>) {
        self.push_current();
        self.background_color = color;
    }

    fn reset(&mut self) {
        self.push_current();
        self.foreground_color = None;
        self.background_color = None;
        self.attributes = Attributes::default();
    }

    fn extended_color(params: &[&[u16]], index: &mut usize) -> Option<Color> {
        let mode = *params.get(*index + 1)?.first()?;
        let arity = match mode {
            5 => 2,
            2 => 4,
            _ => return None,
        };
        // The arguments belong to the colour even when one is missing or out of range; left in
        // place they would be applied as SGR codes of their own.
        let first_arg = *index + 2;
        *index = (*index + arity).min(params.len() - 1);
        let arg = |i: usize| u8::try_from(*params.get(first_arg + i)?.first()?).ok();
        if arity == 2 {
            Some(Color::AnsiValue(arg(0)?))
        } else {
            Some(Color::Rgb {
                r: arg(0)?,
                g: arg(1)?,
                b: arg(2)?,
            })
        }
    }

    fn extended_color_subparams(param: &[u16]) -> Option<Color> {
        match param {
            [_, 5, color] => Some(Color::AnsiValue(u8::try_from(*color).ok()?)),
            [_, 2, r, g, b] | [_, 2, _, r, g, b] => Some(Color::Rgb {
                r: u8::try_from(*r).ok()?,
                g: u8::try_from(*g).ok()?,
                b: u8::try_from(*b).ok()?,
            }),
            _ => None,
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        let params = params.iter().collect::<Vec<_>>();
        let mut index = 0;
        while index < params.len() {
            let param = params[index];
            let action = param.first().copied().unwrap_or_default();
            match action {
                0 => self.reset(),
                1 => self.set_intensity(Some(Attribute::Bold)),
                2 => self.set_intensity(Some(Attribute::Dim)),
                22 => self.set_intensity(None),
                30..=37 => self.set_foreground(Some(Color::AnsiValue((action - 30) as u8))),
                38 => {
                    let color = if param.len() > 1 {
                        Self::extended_color_subparams(param)
                    } else {
                        Self::extended_color(&params, &mut index)
                    };
                    if let Some(color) = color {
                        self.set_foreground(Some(color));
                    }
                }
                39 => self.set_foreground(None),
                40..=47 => self.set_background(Some(Color::AnsiValue((action - 40) as u8))),
                48 => {
                    let color = if param.len() > 1 {
                        Self::extended_color_subparams(param)
                    } else {
                        Self::extended_color(&params, &mut index)
                    };
                    if let Some(color) = color {
                        self.set_background(Some(color));
                    }
                }
                49 => self.set_background(None),
                90..=97 => self.set_foreground(Some(Color::AnsiValue((action - 82) as u8))),
                100..=107 => self.set_background(Some(Color::AnsiValue((action - 92) as u8))),
                _ => {}
            }
            index += 1;
        }
    }
}

impl Perform for ColoredStringParser {
    fn print(&mut self, c: char) {
        self.line_buffer.push(c);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first().copied() != Some(b"8") || params.len() < 3 {
            return;
        }

        self.push_current();
        let uri = params[2..]
            .iter()
            .map(|param| String::from_utf8_lossy(param))
            .join(";");
        self.hyperlink = (!uri.is_empty()).then(|| Hyperlink::new(uri));
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: u8) {
        if !ignore && intermediates.is_empty() && action == b'm' {
            self.apply_sgr(params);
        }
    }
}

/// Set of helper methods for `Vec<Line>`, that manipulate on each line individually.
impl Lines {
    /// Empty lines block.
    pub fn new() -> Lines {
        Lines(Vec::new())
    }

    /// Creates an instance of [`Lines`] with a style applied from a single multiline string.
    pub fn from_multiline_string(multiline_string: &str, style: ContentStyle) -> Lines {
        multiline_string
            .lines()
            .map(|line| {
                let styled = StyledContent::new(style, line.to_owned());
                Line::from_iter([Span::new_styled_lossy(styled)])
            })
            .collect()
    }

    /// Same as `from_multiline_string`, but does not use lossy conversion, it allows all whitespace characters
    /// It's specially designed for emit/emit_aux
    pub fn from_multiline_string_raw(multiline_string: &str, style: ContentStyle) -> Lines {
        multiline_string
            .lines()
            .map(|line| {
                let styled = StyledContent::new(style, line.to_owned());
                Line::from_iter([Span::new_styled_raw(styled)])
            })
            .collect()
    }

    /// Takes a multiline string that might contain ANSI color codes, and returns a set of lines
    /// that include spans representing those color codes.
    ///
    /// Note that any other types of control characters are omitted, and certain whitespace
    /// characters are also disallowed / replaced.
    pub fn from_colored_multiline_string(multiline_string: &str) -> Lines {
        let mut parser = Parser::<DefaultCharAccumulator>::new();
        let mut color_parser = ColoredStringParser::default();
        multiline_string
            .lines()
            .map(|s| color_parser.parse_line(&mut parser, s))
            .collect()
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// No lines.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, line: Line) {
        self.0.push(line);
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = Line>) {
        self.0.extend(lines);
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Line> {
        self.0.iter()
    }

    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Line> {
        self.0.iter_mut()
    }

    /// truncates all lines to the same max width.
    pub fn truncate_lines(&mut self, max_width: usize) {
        self.iter_mut()
            .for_each(|line| line.truncate_line(max_width));
    }

    /// Returns the max column width of any line
    pub fn max_line_length(&self) -> usize {
        // for each line in the LHS
        self.iter()
            // add the length of each word in the line
            .map(|line| line.len())
            // pick the maximum line length
            .max()
            // or set it to 0 if there are no lines on the left side
            .unwrap_or_default()
    }

    /// Given a set of lines, inserts the specified amount of padding to the longest line.
    /// However, the entire block of lines will remain a rectangle afterwards.
    /// This means that shorter lines will receive extra paddding such that it has the same length as the longest line
    pub fn pad_lines_right(&mut self, amount: usize) {
        if amount == 0 {
            return;
        }

        let longest_len = self.max_line_length();
        for line in self.iter_mut() {
            let len = line.len();
            line.pad_right(amount + (longest_len - len));
        }
    }

    /// Prepends a fixed `amount` of padding to each row.
    /// Unlike `pad_lines_right`, this method does not add extra padding to shorter lines.
    /// The right end of the padding remains ragged.
    pub fn pad_lines_left(&mut self, amount: usize) {
        if amount == 0 {
            return;
        }

        self.iter_mut().for_each(|line| {
            line.pad_left(amount);
        });
    }

    /// Adds padding to each line dependent on its length so that each line is the same length.
    /// The line is left justified always.  This is because right or center aligning lines inherently justifies them.
    /// Therefore, there is no explicit method for justifying text any other way.
    pub fn justify(&mut self) {
        let longest_len = self.max_line_length();
        for line in self.iter_mut() {
            let len = line.len();
            line.pad_right(longest_len - len);
        }
    }

    /// Given a set of lines, set them all to the exact width
    pub fn set_lines_to_exact_width(&mut self, exact_width: usize) {
        self.iter_mut()
            .for_each(|line| line.to_exact_width(exact_width));
    }

    /// Extends the Lines list by the given length, adding empty lines at the bottom
    pub fn pad_lines_bottom(&mut self, amount: usize) {
        let extender = iter::repeat_n(Line::default(), amount);
        self.extend(extender);
    }

    /// Same functionality as `pad_lines_bottom` but on the top.
    pub fn pad_lines_top(&mut self, amount: usize) {
        let extender = iter::repeat_n(Line::default(), amount);

        self.0.splice(0..0, extender);
    }

    /// Truncates the line list to the given length, removing entries at the end.
    pub fn truncate_lines_bottom(&mut self, desired_length: usize) {
        self.0.truncate(desired_length);
    }

    /// Sets the line list to the given length, padding or truncating from the bottom.
    pub fn set_lines_to_exact_length(&mut self, desired_length: usize) {
        match self.len().cmp(&desired_length) {
            Ordering::Less => {
                self.pad_lines_bottom(desired_length - self.len());
            }
            Ordering::Equal => {}
            Ordering::Greater => {
                self.truncate_lines_bottom(desired_length);
            }
        }
    }

    /// Truncates columns and rows that do not fit within a bounding box
    pub fn shrink_lines_to_dimensions(&mut self, dimensions: Dimensions) {
        self.iter_mut()
            .for_each(|line| line.truncate_line(dimensions.width));
        self.truncate_lines_bottom(dimensions.height);
    }

    /// Like `render`, but with a limit.
    /// If a limit is specified, no more than that amount will be drained.
    /// The limit is on the number of *lines*, **NOT** the number of *bytes*.
    /// Care should be taken with calling a limit of 0 - this will cause no lines to render and the buffer to never be drained.
    ///
    /// Returns the remain limit after rendering.  If the limit is None, means no limit
    pub(crate) fn render_with_limit(
        &mut self,
        writer: &mut Vec<u8>,
        limit: Option<usize>,
    ) -> Option<usize> {
        let output_limit = limit.unwrap_or(self.len());
        let amt = cmp::min(output_limit, self.len());
        for line in self.0.drain(..amt) {
            line.render_with_clear_and_nl(writer);
        }
        if limit.is_some() {
            Some(output_limit - amt)
        } else {
            // if the original limit was None, it means no limit, so just return None meaning no limit
            None
        }
    }

    /// Formats and renders all lines to `buffer`.
    /// Notably, this *queues* the lines for rendering.  You must flush the buffer.
    pub(crate) fn render_from_line(&self, writer: &mut Vec<u8>, start: usize) {
        for line in self.0.iter().skip(start) {
            line.render_with_clear_and_nl(writer);
        }
    }

    /// Render the lines without an escape sequence to clear the line.
    /// It will clear the lines at the end.
    pub(crate) fn render_raw(&mut self, writer: &mut Vec<u8>) {
        for line in self.0.iter() {
            writeln!(writer, "{}", line.render()).unwrap();
        }
        self.0.clear();
    }

    pub(crate) fn lines_equal(&self, other: &Self) -> usize {
        self.0
            .iter()
            .zip(other.0.iter())
            .take_while(|(l1, l2)| l1 == l2)
            .count()
    }

    /// Returns the maximum line width and the number of lines.
    /// This corresponds to how much space a justified version of the output would take.
    pub fn dimensions(&self) -> Dimensions {
        let x = self.max_line_length();
        let y = self.len();

        (x, y).into()
    }

    /// Sets the lines to the exact dimensions specified below, truncating or padding as necessary.
    pub fn set_lines_to_exact_dimensions(&mut self, Dimensions { width, height }: Dimensions) {
        self.set_lines_to_exact_length(height);
        self.set_lines_to_exact_width(width);
    }

    /// Join blocks horizontally, i.e. side by side.
    pub fn join_horizontally(blocks: Vec<Lines>) -> Lines {
        if blocks.is_empty() {
            return Lines::new();
        }

        // unwrap ok because guaranteed > 0 children from constructor
        let longest = blocks.iter().map(|output| output.len()).max().unwrap();

        // pad all other outputs to be the same length.  Then, justify them to be uniform blocks.
        let padded = blocks.into_iter().update(|output| {
            output.set_lines_to_exact_length(longest);
            output.justify();
        });

        // can't do arbitrary zip, so this'll have to do
        padded
            .reduce(|mut all, output| {
                for (all_line, output_line) in all.iter_mut().zip(output) {
                    all_line.extend(output_line);
                }

                all
            })
            // safe to unwrap because at least one child component required
            .unwrap()
    }

    pub fn fmt_for_test(&self) -> impl Display + '_ {
        struct Impl<'a>(&'a Lines);
        impl Display for Impl<'_> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                for line in self.0.iter() {
                    writeln!(f, "{}", line.fmt_for_test())?;
                }
                Ok(())
            }
        }
        Impl(self)
    }
}

impl FromIterator<Line> for Lines {
    fn from_iter<I: IntoIterator<Item = Line>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl IntoIterator for Lines {
    type Item = Line;
    type IntoIter = <Vec<Line> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_lines() -> anyhow::Result<()> {
        let mut test: Lines = Lines(vec![
            vec!["test", "line"].try_into()?,
            vec!["another one"].try_into()?,
        ]);

        let mut new_test = test.clone();
        new_test.truncate_lines(5);
        test.0[0] = vec!["test", "l"].try_into()?;
        test.0[1] = vec!["anoth"].try_into()?;
        assert_eq!(test, new_test);

        let mut empty = Lines::new();
        empty.truncate_lines(5);
        assert_eq!(empty, Lines::new());

        Ok(())
    }

    #[test]
    fn test_max_line_length() -> anyhow::Result<()> {
        let test = Lines(vec![
            Line::default(),
            vec!["test", "line"].try_into()?,
            vec!["another one"].try_into()?,
        ]);

        assert_eq!(test.max_line_length(), 11);
        assert_eq!(Lines::new().max_line_length(), 0);

        Ok(())
    }

    #[test]
    fn test_pad_lines_right() -> anyhow::Result<()> {
        let mut test = Lines(vec![
            vec!["test", "line"].try_into()?, // 8 chars
            vec!["another one"].try_into()?,  // 11 chars
            Line::default(),                  // 0 chars
        ]);
        let result = Lines(vec![
            vec!["test", "line", &" ".repeat(11 + 3)].try_into()?,
            vec!["another one", &" ".repeat(11)].try_into()?,
            vec![" ".repeat(11 + 11)].try_into()?,
        ]);
        test.pad_lines_right(11);
        assert_eq!(test, result);

        Ok(())
    }

    #[test]
    fn test_pad_lines_left() -> anyhow::Result<()> {
        let mut test = Lines(vec![
            vec!["test", "line"].try_into()?, // 8 chars
            vec!["another one"].try_into()?,  // 11 chars
            Line::default(),                  // 0 chars
        ]);
        let result = Lines(vec![
            vec![" ".repeat(11).as_ref(), "test", "line"].try_into()?,
            vec![" ".repeat(11).as_ref(), "another one"].try_into()?,
            vec![" ".repeat(11)].try_into()?,
        ]);
        test.pad_lines_left(11);
        assert_eq!(test, result);

        Ok(())
    }

    #[test]
    fn test_pad_lines_bottom() -> anyhow::Result<()> {
        let mut test = Lines(vec![vec!["test"].try_into()?, vec!["another"].try_into()?]);
        test.pad_lines_bottom(3);
        let result = Lines(vec![
            vec!["test"].try_into()?,
            vec!["another"].try_into()?,
            Line::default(),
            Line::default(),
            Line::default(),
        ]);

        assert_eq!(test, result);

        Ok(())
    }

    #[test]
    fn test_pad_lines_top() -> anyhow::Result<()> {
        let mut test = Lines(vec![vec!["test"].try_into()?, vec!["another"].try_into()?]);
        test.pad_lines_top(3);
        let result = Lines(vec![
            Line::default(),
            Line::default(),
            Line::default(),
            vec!["test"].try_into()?,
            vec!["another"].try_into()?,
        ]);

        assert_eq!(test, result);

        Ok(())
    }

    #[test]
    fn test_truncate_lines_bottom() {
        let mut test = Lines(vec![
            vec!["test"].try_into().unwrap(),
            vec!["another"].try_into().unwrap(),
            vec!["one more"].try_into().unwrap(),
        ]);
        test.truncate_lines_bottom(1);
        let output = Lines(vec![vec!["test"].try_into().unwrap()]);
        assert_eq!(test, output);
    }

    #[test]
    fn test_justify() {
        let mut test = Lines(vec![
            vec!["test"].try_into().unwrap(),
            Line::default(),
            vec!["ok"].try_into().unwrap(),
        ]);

        test.justify();
        let expected = Lines(vec![
            vec!["test"].try_into().unwrap(),
            vec![" ".repeat(4)].try_into().unwrap(),
            vec!["ok", "  "].try_into().unwrap(),
        ]);

        assert_eq!(test, expected);
    }

    #[test]
    fn test_from_multiline_string() {
        let content = "foo bar\n\nbaz\nsome other line";
        let style = ContentStyle {
            foreground_color: Some(Color::Red),
            background_color: None,
            underline_color: None,
            attributes: Default::default(),
        };
        let test = Lines::from_multiline_string(content, style);
        let expected = Lines(vec![
            Line::from_iter([Span::new_styled_lossy(StyledContent::new(
                style,
                "foo bar".to_owned(),
            ))]),
            Line::from_iter([Span::new_styled_lossy(StyledContent::new(
                style,
                "".to_owned(),
            ))]),
            Line::from_iter([Span::new_styled_lossy(StyledContent::new(
                style,
                "baz".to_owned(),
            ))]),
            Line::from_iter([Span::new_styled_lossy(StyledContent::new(
                style,
                "some other line".to_owned(),
            ))]),
        ]);

        assert_eq!(test, expected);
    }

    #[test]
    fn test_colored_from_multiline_string() {
        // Lots of little things we check in here, including that we persist state
        // across lines, that we only change the state that is requested to be changed,
        // that we control fg/bg properly, that we parse different ansi formats, etc.
        let test_string = format!(
            "This is a string
That has both {blue}8 bit blue {blue2}(in both formats)
in it,{reset} as well as {high_blue}256 color blue,
{reset}and {rgb_blue}RGB blue as well.{reset} It resets to the
console default at the end.
It can do {rgb_blue}{bg_blue}background colors, {blue}foreground colors,
{bold}colored, and {reset}normal {bold}bold,{remove_bold} and it
strips out {bs}invalid control sequences",
            blue = "\x1b[34m",
            blue2 = "\x1b[38;5;4m",
            high_blue = "\x1b[38;5;20m",
            rgb_blue = "\x1b[38;2;0;0;238m",
            bg_blue = "\x1b[44m",
            reset = "\x1b[0m",
            bold = "\x1b[1m",
            remove_bold = "\x1b[22m",
            bs = "\x1b[D"
        );

        let default = ContentStyle::default();
        // Both 34m and 38;5;4m parse to the same thing, so no need for blue2
        let blue = ContentStyle {
            foreground_color: Some(Color::AnsiValue(4)),
            ..Default::default()
        };
        let high_blue = ContentStyle {
            foreground_color: Some(Color::AnsiValue(20)),
            ..Default::default()
        };
        let rgb_blue = ContentStyle {
            foreground_color: Some(Color::from((0, 0, 238))),
            ..Default::default()
        };
        let bg_blue = ContentStyle {
            background_color: Some(Color::AnsiValue(4)),
            ..Default::default()
        };
        let rgb_blue_and_bg_blue = ContentStyle {
            foreground_color: rgb_blue.foreground_color,
            background_color: bg_blue.background_color,
            ..Default::default()
        };
        let blue_and_bg_blue = ContentStyle {
            foreground_color: blue.foreground_color,
            background_color: bg_blue.background_color,
            ..Default::default()
        };
        let blue_and_bg_blue_bold = ContentStyle {
            foreground_color: blue.foreground_color,
            background_color: bg_blue.background_color,
            underline_color: None,
            attributes: Attributes::from(Attribute::Bold),
        };
        let bold = ContentStyle {
            attributes: Attributes::from(Attribute::Bold),
            ..Default::default()
        };

        let expected = vec![
            vec![StyledContent::new(default, "This is a string")],
            vec![
                StyledContent::new(default, "That has both "),
                StyledContent::new(blue, "8 bit blue "),
                StyledContent::new(blue, "(in both formats)"),
            ],
            vec![
                StyledContent::new(blue, "in it,"),
                StyledContent::new(default, " as well as "),
                StyledContent::new(high_blue, "256 color blue,"),
            ],
            vec![
                StyledContent::new(default, "and "),
                StyledContent::new(rgb_blue, "RGB blue as well."),
                StyledContent::new(default, " It resets to the"),
            ],
            vec![StyledContent::new(default, "console default at the end.")],
            vec![
                StyledContent::new(default, "It can do "),
                StyledContent::new(rgb_blue_and_bg_blue, "background colors, "),
                StyledContent::new(blue_and_bg_blue, "foreground colors,"),
            ],
            vec![
                StyledContent::new(blue_and_bg_blue_bold, "colored, and "),
                StyledContent::new(default, "normal "),
                StyledContent::new(bold, "bold,"),
                StyledContent::new(default, " and it"),
            ],
            vec![StyledContent::new(
                default,
                "strips out invalid control sequences",
            )],
        ];
        let expected: Lines = expected
            .into_iter()
            .map(|spans| {
                Line::from_iter(spans.iter().map(|sc| {
                    Span::new_styled_lossy(StyledContent::new(
                        *sc.style(),
                        (*sc.content()).to_owned(),
                    ))
                }))
            })
            .collect();

        let lines = Lines::from_colored_multiline_string(&test_string);

        assert_eq!(expected, lines);
    }

    #[test]
    fn test_hyperlink() {
        let input = format!(
            "This is a {link}https://example.com{endlink}hyper{link}{endlink}{link}https://example.com{endlink}link{link}{endlink}.",
            link = "\x1b]8;;",
            endlink = "\x1b\\"
        );
        let lines = Lines::from_colored_multiline_string(&input);
        let expected: Lines = vec![Line::from_iter([
            Span::new_unstyled("This is a ").unwrap(),
            Span::new_unstyled("hyperlink")
                .unwrap()
                .with_hyperlink(Some(Hyperlink::new("https://example.com"))),
            Span::new_unstyled(".").unwrap(),
        ])]
        .into_iter()
        .collect();
        assert_eq!(expected, lines);
    }

    #[test]
    fn test_bell_terminated_hyperlink_with_semicolon() {
        let input = "\x1b]8;;https://example.com/a;b\x07link\x1b]8;;\x07";
        let lines = Lines::from_colored_multiline_string(input);
        let expected: Lines = vec![Line::from_iter([Span::new_unstyled("link")
            .unwrap()
            .with_hyperlink(Some(Hyperlink::new("https://example.com/a;b")))])]
        .into_iter()
        .collect();
        assert_eq!(expected, lines);
    }

    #[test]
    fn test_bright_and_colon_delimited_colors() {
        let input = "\x1b[91mbright\x1b[39mdefault\x1b[48:2::1:2:3mbg\x1b[49mplain";
        let lines = Lines::from_colored_multiline_string(input);
        let expected = Line::from_iter([
            Span::new_styled_lossy(StyledContent::new(
                ContentStyle {
                    foreground_color: Some(Color::AnsiValue(9)),
                    ..Default::default()
                },
                "bright".to_owned(),
            )),
            Span::new_unstyled("default").unwrap(),
            Span::new_styled_lossy(StyledContent::new(
                ContentStyle {
                    background_color: Some(Color::Rgb { r: 1, g: 2, b: 3 }),
                    ..Default::default()
                },
                "bg".to_owned(),
            )),
            Span::new_unstyled("plain").unwrap(),
        ]);
        assert_eq!(Lines(vec![expected]), lines);
    }

    #[test]
    fn test_combined_sgr_parameters_and_unicode() {
        let input = "\x1b[1;34;43m\u{1f9b6} styled\x1b[22;39;49m plain";
        let lines = Lines::from_colored_multiline_string(input);
        let expected = Line::from_iter([
            Span::new_styled_lossy(StyledContent::new(
                ContentStyle {
                    foreground_color: Some(Color::AnsiValue(4)),
                    background_color: Some(Color::AnsiValue(3)),
                    underline_color: None,
                    attributes: Attributes::from(Attribute::Bold),
                },
                "\u{1f9b6} styled".to_owned(),
            )),
            Span::new_unstyled(" plain").unwrap(),
        ]);
        assert_eq!(Lines(vec![expected]), lines);
    }

    #[test]
    fn test_fmt_for_test() {
        let lines = Lines::from_iter([
            Line::unstyled("orange").unwrap(),
            Line::from_iter([Span::new_colored("pineapple", Color::Yellow).unwrap()]),
        ]);
        assert_eq!(
            "orange\n<span fg=yellow>pineapple</span>\n",
            format!("{}", lines.fmt_for_test())
        );
    }

    /// The arguments of a malformed extended colour are consumed with it, so the text stays plain.
    #[test]
    fn test_malformed_extended_colour_args_are_ignored() {
        let lines = Lines::from_colored_multiline_string("\x1b[38;2;1;2mx");
        assert_eq!(lines.0[0].fmt_for_test().to_string(), "x");
        let lines = Lines::from_colored_multiline_string("\x1b[38;2;300;31;1mx");
        assert_eq!(lines.0[0].fmt_for_test().to_string(), "x");
        let lines = Lines::from_colored_multiline_string("\x1b[38;5;1;1mx");
        assert_eq!(
            lines.0[0].fmt_for_test().to_string(),
            "<span fg=ansi(1) bold>x</span>"
        );
    }
}
