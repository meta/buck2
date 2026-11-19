/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::Component;
use crate::Lines;
use crate::components::Blank;
use crate::components::Dimensions;
use crate::components::DrawMode;

/// The `Padded` [`Component`] wraps its child by padding left, right, above, and below its content.
/// This can be used to shift the content to a different location and ensure that following content comes after a certain distance.
/// It is worth noting that this component will also *truncate* any content that is too long to fit in the given window at draw time.
/// However, components are expected to constrain themselves to the given window, anyway.
///
/// Content is truncated preferentially over padding.
#[derive(Debug)]
pub struct Padded<C: Component> {
    pub child: C,
    pub left: usize,
    pub right: usize,
    pub top: usize,
    pub bottom: usize,
}

impl Default for Padded<Blank> {
    fn default() -> Self {
        Self {
            child: Blank,
            left: 0,
            right: 0,
            top: 0,
            bottom: 0,
        }
    }
}

impl<C: Component> Padded<C> {
    pub fn new(child: C, left: usize, right: usize, top: usize, bottom: usize) -> Self {
        Self {
            child,
            left,
            right,
            top,
            bottom,
        }
    }
}

impl<C: Component> Component for Padded<C> {
    type Error = C::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, C::Error> {
        let inner = Dimensions {
            width: dimensions.width.saturating_sub(self.left + self.right),
            height: dimensions.height.saturating_sub(self.top + self.bottom),
        };
        let mut output = self.child.draw(inner, mode)?;

        // ordering is important:
        // the top and bottom lines need to be padded horizontally too.
        output.pad_lines_top(self.top);
        // cut off enough space at the bottom for the bottom padding
        output.truncate_lines_bottom(dimensions.height.saturating_sub(self.bottom));
        output.pad_lines_bottom(self.bottom);

        output.pad_lines_left(self.left);
        // cut off enough space on the right for the right padding
        output.truncate_lines(dimensions.width.saturating_sub(self.right));
        output.pad_lines_right(self.right);

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use derive_more::AsRef;

    use crate::Component;
    use crate::Dimensions;
    use crate::Direction;
    use crate::DrawMode;
    use crate::Line;
    use crate::Lines;
    use crate::components::Aligned;
    use crate::components::Padded;
    use crate::components::Split;
    use crate::components::alignment::HorizontalAlignmentKind;
    use crate::components::alignment::VerticalAlignmentKind;
    use crate::components::echo::Echo;
    use crate::components::splitting::SplitKind;

    #[derive(Debug, AsRef)]
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

    /// The child is drawn in the window that remains inside the padding, so a child that fills
    /// its window keeps all of its content.
    #[test]
    fn test_child_filling_its_window() {
        // Right-aligned "ab" with two columns of left padding in six columns.
        let padded = Padded::new(
            Aligned::new(
                echo_rows(&["ab"]),
                HorizontalAlignmentKind::Right,
                VerticalAlignmentKind::Top,
            ),
            2,
            0,
            0,
            0,
        );
        let out = padded
            .draw(Dimensions::new(6, 1), DrawMode::Normal)
            .unwrap();
        assert_eq!(rows(&out), vec!["    ab".to_owned()]);

        // Bottom-aligned "ab" with one row of top padding in three rows.
        let padded = Padded::new(
            Aligned::new(
                echo_rows(&["ab"]),
                HorizontalAlignmentKind::Left(false),
                VerticalAlignmentKind::Bottom,
            ),
            0,
            0,
            1,
            0,
        );
        let out = padded
            .draw(Dimensions::new(6, 3), DrawMode::Normal)
            .unwrap();
        let r = rows(&out);
        assert_eq!(r.len(), 3);
        assert!(r[0].trim().is_empty() && r[1].trim().is_empty(), "{r:?}");
        assert_eq!(r[2].trim_end(), "ab");

        // Padding of one around an equal horizontal split whose right pane right-aligns its
        // content, as a two-pane TUI does.
        let left = echo_rows(&["stats", "more", "last"]);
        let right = Aligned::new(
            echo_rows(&["14.1s", "0.2s", "9.9s"]),
            HorizontalAlignmentKind::Right,
            VerticalAlignmentKind::Top,
        );
        let split = Split::<&dyn Component<Error = Infallible>>::new(
            vec![&left, &right],
            Direction::Horizontal,
            SplitKind::Equal,
        );
        let padded = Padded::new(split, 1, 1, 1, 1);
        let out = padded
            .draw(Dimensions::new(20, 5), DrawMode::Normal)
            .unwrap();
        let r = rows(&out);
        assert_eq!(r.len(), 5);
        assert_eq!(r[1], " stats        14.1s ");
    }

    #[test]
    fn test_pad_left() {
        let msg = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);
        let padder = Padded {
            child: Echo(msg),
            left: 5,
            right: 0,
            top: 0,
            bottom: 0,
        };

        let drawing = padder
            .draw(Dimensions::new(20, 20), DrawMode::Normal)
            .unwrap();
        let expected = Lines(vec![
            vec![" ".repeat(5).as_ref(), "hello world"]
                .try_into()
                .unwrap(),
            vec![" ".repeat(5).as_ref(), "ok"].try_into().unwrap(),
            vec![" ".repeat(5)].try_into().unwrap(),
        ]);
        assert_eq!(drawing, expected);
    }

    #[test]
    fn test_pad_right() {
        let msg = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);
        let padder = Padded {
            child: Echo(msg),
            right: 4,
            left: 0,
            top: 0,
            bottom: 0,
        };

        let drawing = padder
            .draw(Dimensions::new(20, 20), DrawMode::Normal)
            .unwrap();
        let expected = Lines(vec![
            vec!["hello world", &" ".repeat(4)].try_into().unwrap(),
            vec!["ok", &" ".repeat(4 + 9)].try_into().unwrap(),
            vec![" ".repeat(4 + 11)].try_into().unwrap(),
        ]);
        assert_eq!(drawing, expected);
    }

    #[test]
    fn test_pad_top() {
        let msg = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);
        let padder = Padded {
            child: Echo(msg),
            top: 5,
            bottom: 0,
            left: 0,
            right: 0,
        };

        let drawing = padder
            .draw(Dimensions::new(15, 15), DrawMode::Normal)
            .unwrap();
        let expected = Lines(vec![
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);

        assert_eq!(drawing, expected);
    }

    #[test]
    fn test_pad_bottom() {
        let msg = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);
        let padder = Padded {
            child: Echo(msg),
            bottom: 5,
            top: 0,
            left: 0,
            right: 0,
        };

        let drawing = padder
            .draw(Dimensions::new(15, 15), DrawMode::Normal)
            .unwrap();
        let expected = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
        ]);

        assert_eq!(drawing, expected);
    }

    #[test]
    fn test_no_pad() {
        let msg = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);
        let padder = Padded {
            child: Echo(msg),
            top: 0,
            bottom: 0,
            left: 0,
            right: 0,
        };

        let drawing = padder
            .draw(Dimensions::new(15, 15), DrawMode::Normal)
            .unwrap();
        let expected = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);

        assert_eq!(drawing, expected);
    }

    #[test]
    fn test_truncated() {
        let msg = Lines(vec![
            vec!["hello world"].try_into().unwrap(),
            vec!["ok"].try_into().unwrap(),
            Line::default(),
        ]);
        let padder = Padded {
            child: Echo(msg),
            left: 5,
            right: 3,
            top: 3,
            bottom: 3,
        };
        let drawing = padder
            .draw(Dimensions::new(10, 8), DrawMode::Normal)
            .unwrap();
        let expected = Lines(vec![
            // 5 rows of padding at the top
            vec![" ".repeat(5), " ".repeat(5)].try_into().unwrap(),
            vec![" ".repeat(5), " ".repeat(5)].try_into().unwrap(),
            vec![" ".repeat(5), " ".repeat(5)].try_into().unwrap(),
            // 2 rows of content, padded on left and right
            vec![" ".repeat(5).as_ref(), "he", &" ".repeat(3)]
                .try_into()
                .unwrap(),
            vec![" ".repeat(5).as_ref(), "ok", &" ".repeat(3)]
                .try_into()
                .unwrap(),
            vec![" ".repeat(5), " ".repeat(5)].try_into().unwrap(),
            vec![" ".repeat(5), " ".repeat(5)].try_into().unwrap(),
            vec![" ".repeat(5), " ".repeat(5)].try_into().unwrap(),
        ]);

        assert_eq!(drawing, expected);
    }
}
