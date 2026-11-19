/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;

use crate::Component;
use crate::Dimensions;
use crate::DrawMode;
use crate::Line;
use crate::Lines;
use crate::Span;
use crate::components::Aligned;
use crate::components::alignment::HorizontalAlignmentKind;
use crate::components::alignment::VerticalAlignmentKind;

/// The `Bordered` component can be used to put borders on all sides of the output of its child.
/// This is useful for delimiting the boundaries of a component for reading and aesthetic purposes.
///
/// # About borders
/// Borders may be any `Span`, and can thus be more than a single character long.
///
/// They may consist of any valid sequence of utf-8 characters.
/// However, `superconsole` has a `unicode-segmentation` dependency, which is tied to a specific version of unicode.
/// Differing unicode versions have different `unicode-segmentation` dependencies, so if a newer (or older) version of unicode is used,
/// then some graphemes may cause superconsole to panic.  This is only relevant in the top and bottom borders, which iterate over the graphemes passed.
///
/// Horizontal borders (i.e. top and bottom) are transposed.  For example, if `top = Word::new_unstyled("@@")`,
/// then the resulting output would look something like this:
///
/// ```console
/// @@@@@@@@@@@@@@@@@@@@@
/// @@@@@@@@@@@@@@@@@@@@@
/// // rest of the output
/// ```
#[derive(Debug)]
pub struct Bordered<C: Component> {
    child: Aligned<C>,
    pub border: BorderedSpec,
}

/// The `BorderedSpec` allows the callee to specify the borders (or lack thereof) of each side.
/// The implementation of [`Default`] allows the user to leave some boundaries unspecified.
/// Unspecified boundaries default to:
/// * '|' if `left` or `right`
/// * '-' if `top` or `bottom`
#[derive(Debug)]
pub struct BorderedSpec {
    pub left: Option<Span>,
    pub right: Option<Span>,
    pub top: Option<Span>,
    pub bottom: Option<Span>,
}

impl Default for BorderedSpec {
    fn default() -> Self {
        let vertical = Some(Span::new_unstyled("|").unwrap());
        let horizontal = Some(Span::new_unstyled("-").unwrap());
        Self {
            left: vertical.clone(),
            right: vertical,
            top: horizontal.clone(),
            bottom: horizontal,
        }
    }
}

impl<C: Component> Bordered<C> {
    pub fn new(child: C, border: BorderedSpec) -> Self {
        Self {
            child: Aligned {
                child,
                horizontal: HorizontalAlignmentKind::Left(true),
                vertical: VerticalAlignmentKind::Top,
            },
            border,
        }
    }
}

/// helper method to transpose horizontal padding.
fn construct_vertical_padding(padding: Span, width: usize) -> Vec<Line> {
    padding
        // iterating over the padding here allows us to retain the styling on each duplicate.
        .iter()
        .map(|mut span| {
            // iterator is a single character here, so fill to width.
            // it's possible that a word could be more than a single column, so the number of repetitions must reflect that.
            let grapheme_width = span.len();
            let copies = width.checked_div(grapheme_width).unwrap_or(0);
            span.content = Cow::Owned(span.content.repeat(copies));
            let mut line = Line::from_iter([span]);
            line.pad_right(width - grapheme_width * copies);
            line
        })
        .collect()
}

impl<C: Component> Component for Bordered<C> {
    type Error = C::Error;

    fn draw_unchecked(
        &self,

        Dimensions { width, height }: Dimensions,
        mode: DrawMode,
    ) -> Result<Lines, C::Error> {
        // Reserve enough draw space for the walls.
        let opt_len = |opt_word: &Option<Span>| match opt_word {
            Some(word) => word.len(),
            None => 0,
        };
        // A top or bottom border takes one row per grapheme, whatever its width.
        let opt_rows = |opt_word: &Option<Span>| match opt_word {
            Some(word) => word.iter().count(),
            None => 0,
        };
        let new_dims = Dimensions {
            width: width.saturating_sub(opt_len(&self.border.left) + opt_len(&self.border.right)),
            height: height
                .saturating_sub(opt_rows(&self.border.top) + opt_rows(&self.border.bottom)),
        };

        // The [`Aligned`] box ensures that the child is justified and bounded.
        let mut output = self.child.draw(new_dims, mode)?;

        for line in output.iter_mut() {
            if let Some(left) = &self.border.left {
                line.push_front(left.clone());
            }
            if let Some(right) = &self.border.right {
                line.push(right.clone());
            }
        }
        if let Some(top) = &self.border.top {
            let lines = construct_vertical_padding(top.clone(), output.max_line_length());
            output.0.splice(0..0, lines);
        }
        if let Some(bottom) = &self.border.bottom {
            let lines = construct_vertical_padding(bottom.clone(), output.max_line_length());
            output.extend(lines);
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use derive_more::AsRef;

    use super::*;
    use crate::components::echo::Echo;

    #[derive(AsRef, Debug)]
    #[allow(dead_code)]
    struct Msg(Lines);

    fn rows(lines: &Lines) -> Vec<String> {
        lines.iter().map(|l| l.to_unstyled()).collect()
    }

    fn echo_rows(rows: &[&str]) -> Echo {
        Echo(Lines(
            rows.iter().map(|r| Line::unstyled(r).unwrap()).collect(),
        ))
    }

    fn top_only(top: &str) -> BorderedSpec {
        BorderedSpec {
            left: None,
            right: None,
            top: Some(Span::new_unstyled(top).unwrap()),
            bottom: None,
        }
    }

    /// A border grapheme of width zero is repeated zero times instead of dividing by zero.
    #[test]
    fn test_zero_width_border() {
        let span = Span::new_unstyled("\u{200b}").unwrap();
        assert_eq!(span.len(), 0);
        let bordered = Bordered::new(echo_rows(&["abc"]), top_only("\u{200b}"));
        let out = bordered
            .draw(Dimensions::new(6, 4), DrawMode::Normal)
            .unwrap();
        let r = rows(&out);
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[1].trim_end(), "abc");
    }

    /// A border grapheme wider than one column reserves one row per grapheme and is padded to
    /// the full width when the width is not a multiple of its own.
    #[test]
    fn test_wide_border_grapheme() {
        let bordered = Bordered::new(echo_rows(&["abc", "def", "ghi"]), top_only("\u{1f9b6}"));
        let out = bordered
            .draw(Dimensions::new(7, 4), DrawMode::Normal)
            .unwrap();
        assert_eq!(
            rows(&out),
            vec![
                "\u{1f9b6} ".to_owned(),
                "abc".to_owned(),
                "def".to_owned(),
                "ghi".to_owned()
            ]
        );

        let bordered = Bordered::new(echo_rows(&["abcdefg"]), top_only("\u{1f9b6}"));
        let out = bordered
            .draw(Dimensions::new(7, 4), DrawMode::Normal)
            .unwrap();
        let widths: Vec<usize> = out.iter().map(|l| l.len()).collect();
        assert_eq!(widths, vec![7, 7]);
    }

    #[test]
    fn test_basic() -> anyhow::Result<()> {
        let msg = Lines(vec![
            vec!["Test"].try_into()?,              // 4 chars
            vec!["Longer"].try_into()?,            // 6 chars
            vec!["Even Longer", "ok"].try_into()?, // 13 chars
            Line::default(),
        ]);

        let component = Bordered::new(Echo(msg), BorderedSpec::default());

        let output = component.draw(Dimensions::new(14, 5), DrawMode::Normal)?;

        // A single character on the right side of the message gets truncated to make way for side padding
        let expected = Lines(vec![
            vec!["-".repeat(14)].try_into()?,
            vec!["|", "Test", &" ".repeat(12 - 4), "|"].try_into()?,
            vec!["|", "Longer", &" ".repeat(12 - 6), "|"].try_into()?,
            vec!["|", "Even Longer", "o", "|"].try_into()?,
            vec!["-".repeat(14)].try_into()?,
        ]);

        assert_eq!(output, expected);

        Ok(())
    }

    #[test]
    fn test_complex() -> anyhow::Result<()> {
        let msg = Lines(vec![
            vec!["Test"].try_into()?,              // 4 chars
            vec!["Longer"].try_into()?,            // 6 chars
            vec!["Even Longer", "ok"].try_into()?, // 13 chars
            Line::default(),
        ]);

        let component = Bordered::new(
            Echo(msg),
            BorderedSpec {
                top: Some("@@@".try_into()?),
                left: None,
                bottom: Some("@".try_into()?),
                ..Default::default()
            },
        );

        let output = component.draw(Dimensions::new(13, 7), DrawMode::Normal)?;

        // A single character on the right side of the message gets truncated to make way for side padding
        let expected = Lines(vec![
            vec!["@".repeat(13)].try_into()?,
            vec!["@".repeat(13)].try_into()?,
            vec!["@".repeat(13)].try_into()?,
            vec!["Test", &" ".repeat(12 - 4), "|"].try_into()?,
            vec!["Longer", &" ".repeat(12 - 6), "|"].try_into()?,
            vec!["Even Longer", "o", "|"].try_into()?,
            vec!["@".repeat(13)].try_into()?,
        ]);

        assert_eq!(output, expected);

        Ok(())
    }

    #[test]
    fn test_multi_width_unicode() -> anyhow::Result<()> {
        let multi_width = "🦶";

        let msg = Lines(vec![vec!["Tested"].try_into()?]);

        let component = Bordered::new(
            Echo(msg),
            BorderedSpec {
                top: Some(multi_width.try_into()?),
                left: None,
                right: None,
                bottom: None,
            },
        );

        let output = component.draw(Dimensions::new(13, 7), DrawMode::Normal)?;
        let expected = Lines(vec![vec!["🦶🦶🦶"].try_into()?, vec!["Tested"].try_into()?]);

        assert_eq!(output, expected);
        Ok(())
    }
}
