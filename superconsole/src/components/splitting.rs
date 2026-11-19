/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Splitting is one of the most primitive building blocks of a fully featured UI.
//! This module contains components and enums that allow for splits along either dimension.

use std::fmt::Debug;

use crate::Component;
use crate::Dimensions;
use crate::Direction;
use crate::DrawMode;
use crate::Lines;

/// Controls the way the splitter displays its children.
#[derive(Clone, Debug)]
pub enum SplitKind {
    // Each child component takes up area proportional to its size relative to the other sizes.
    Sized(Vec<f64>),
    // Each child component takes up an equal % of the area long the given dimension.
    Equal,
    // Each child may take as much space as it would like along the given dimension.
    // No extra padding is given between children.
    Adaptive,
}

/// Internally, we want to alias away the Equal case
#[derive(Clone, Debug)]
enum InternalSplitKind {
    SizedNormalized(Vec<f64>),
    Adaptive,
}

impl SplitKind {
    fn to_internal_split_kind(&self, children_len: usize) -> InternalSplitKind {
        match self {
            SplitKind::Sized(sizes) => {
                assert_eq!(
                    sizes.len(),
                    children_len,
                    "There must be an equal number of ratios and children."
                );

                let total: f64 = sizes.iter().sum();
                let normalized_sizes = sizes.iter().map(|size| size / total).collect();

                InternalSplitKind::SizedNormalized(normalized_sizes)
            }
            SplitKind::Equal => {
                InternalSplitKind::SizedNormalized(vec![1.0 / children_len as f64; children_len])
            }
            SplitKind::Adaptive => InternalSplitKind::Adaptive,
        }
    }
}

impl InternalSplitKind {
    pub fn draw<'a, C: Component + 'a>(
        &self,
        children: impl IntoIterator<Item = &'a C>,
        direction: Direction,

        dimensions: Dimensions,
        mode: DrawMode,
    ) -> Result<Vec<Lines>, C::Error> {
        match self {
            InternalSplitKind::SizedNormalized(sizes) => {
                let mut outputs = children
                    .into_iter()
                    .zip(sizes.iter())
                    .map(|(child, size)| -> Result<_, C::Error> {
                        // allocate alloted size
                        let child_dimension = dimensions.multiply(*size, direction);
                        let mut output = child.draw(child_dimension, mode)?;

                        // bound non-splitting direction, pad splitting direction
                        match direction {
                            Direction::Horizontal => {
                                output.truncate_lines_bottom(child_dimension.height);
                                output.set_lines_to_exact_width(child_dimension.width);
                            }
                            Direction::Vertical => {
                                output.truncate_lines(child_dimension.width);
                                output.set_lines_to_exact_length(child_dimension.height);
                            }
                        }

                        Ok(output)
                    })
                    .collect::<Result<Vec<Lines>, C::Error>>()?;
                if direction == Direction::Horizontal {
                    // Joining pads every block to the tallest one and justifies each block to its
                    // own widest line, so a block with no lines would take no width at all.
                    let rows = outputs.iter().map(|o| o.len()).max().unwrap_or(0);
                    for (output, size) in outputs.iter_mut().zip(sizes.iter()) {
                        output.set_lines_to_exact_length(rows);
                        output
                            .set_lines_to_exact_width(dimensions.multiply(*size, direction).width);
                    }
                }
                Ok(outputs)
            }
            InternalSplitKind::Adaptive => {
                let mut available = dimensions;
                children
                    .into_iter()
                    .map(|child| {
                        let mut output = child.draw(available, mode)?;
                        output.shrink_lines_to_dimensions(dimensions);

                        // decrease size by however much was just used
                        let used = Dimensions::dimension_from_output_truncated(&output, direction);
                        available = available.saturating_sub(used, direction);

                        Ok(output)
                    })
                    .collect()
            }
        }
    }
}

/// [`Splits`](SplitKind) along a given [`direction`](crate::Direction) for its child [`components`](Component).
/// Child components are truncated to the bounds passed to them.
pub struct Split<C> {
    children: Vec<C>,
    direction: Direction,
    split: InternalSplitKind,
}

impl<C> Debug for Split<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Split")
            .field("children", &self.children.len())
            .field("direction", &self.direction)
            .field("split", &self.split)
            .finish()
    }
}

impl<C: Component> Split<C> {
    /// Preconditions:
    /// * At least one child.
    /// * If Sized, then ratios must sum to approximately 1.
    /// * If Sized, then there must be as many ratios as components
    pub fn new(children: Vec<C>, direction: Direction, split: SplitKind) -> Self {
        let split = split.to_internal_split_kind(children.len());

        Self {
            children,
            direction,
            split,
        }
    }
}

impl<C: Component> Component for Split<C> {
    type Error = C::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, C::Error> {
        let outputs = self
            .split
            .draw(&self.children, self.direction, dimensions, mode)?;

        Ok(match self.direction {
            Direction::Horizontal => Lines::join_horizontally(outputs),
            Direction::Vertical => outputs.into_iter().flatten().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::iter;

    use derive_more::AsRef;

    use super::Split;
    use super::SplitKind;
    use crate::Component;
    use crate::Dimensions;
    use crate::Direction;
    use crate::DrawMode;
    use crate::Line;
    use crate::Lines;
    use crate::components::Blank;
    use crate::components::echo::Echo;

    type Dyn = Box<dyn Component<Error = Infallible>>;

    fn echo_rows(rows: &[&str]) -> Dyn {
        Box::new(Echo(Lines(
            rows.iter().map(|r| Line::unstyled(r).unwrap()).collect(),
        )))
    }

    fn rows(lines: &Lines) -> Vec<String> {
        lines.iter().map(|l| l.to_unstyled()).collect()
    }

    /// A child that draws nothing still takes up its share of a horizontal split.
    #[test]
    fn test_horizontal_split_with_empty_child() {
        let split = Split::<Dyn>::new(
            vec![Box::new(Blank), echo_rows(&["xx"])],
            Direction::Horizontal,
            SplitKind::Equal,
        );
        let out = split
            .draw(Dimensions::new(10, 1), DrawMode::Normal)
            .unwrap();
        assert_eq!(rows(&out), vec!["     xx   ".to_owned()]);
        assert_eq!(out.max_line_length(), 10);

        let split = Split::<Dyn>::new(
            vec![
                echo_rows(&["aaaaaa"]),
                Box::new(Blank),
                echo_rows(&["cccccc"]),
            ],
            Direction::Horizontal,
            SplitKind::Sized(vec![1.0, 1.0, 1.0]),
        );
        let out = split
            .draw(Dimensions::new(12, 1), DrawMode::Normal)
            .unwrap();
        assert_eq!(rows(&out), vec!["aaaa    cccc".to_owned()]);
    }

    #[derive(AsRef, Debug)]
    #[allow(dead_code)]
    struct Echo1(Lines);
    #[derive(AsRef, Debug)]
    #[allow(dead_code)]
    struct Echo2(Lines);

    #[derive(AsRef, Debug)]
    #[allow(dead_code)]
    struct Echo3(Lines);

    mod horizontal {
        use super::*;
        use crate::Dimensions;
        use crate::components::echo::Echo;
        #[test]
        fn test_adaptive() {
            let kind = SplitKind::Adaptive;
            let dimension = Direction::Horizontal;

            // left rows all the same length
            let mut left_msg = Lines(vec![
                vec!["test"].try_into().unwrap(),
                vec!["ok", "so"].try_into().unwrap(),
                vec!["sync"].try_into().unwrap(),
            ]);
            let right_msg = Lines(vec![
                vec!["xx"].try_into().unwrap(),
                vec!["yyy"].try_into().unwrap(),
                vec!["z"].try_into().unwrap(),
            ]);
            let splitter = Split::new(
                vec![Echo(left_msg.clone()), Echo(right_msg.clone())],
                dimension,
                kind.clone(),
            );
            let horizontal = Lines(vec![
                vec!["test", "xx", " "].try_into().unwrap(),
                vec!["ok", "so", "yyy"].try_into().unwrap(),
                vec!["sync", "z", "  "].try_into().unwrap(),
            ]);

            let output = splitter
                .draw(Dimensions::new(10, 10), DrawMode::Normal)
                .unwrap();
            assert_eq!(output, horizontal);

            // uneven left row
            left_msg.0[0] = vec!["hello world"].try_into().unwrap();
            let horizontal = Lines(vec![
                vec!["hello world", "xx", " "].try_into().unwrap(),
                vec!["ok", "so", &" ".repeat(7), "yyy"].try_into().unwrap(),
                vec!["sync", &" ".repeat(7), "z", "  "].try_into().unwrap(),
            ]);

            let splitter = Split::new(vec![Echo(left_msg), Echo(right_msg)], dimension, kind);

            let output = splitter
                .draw(Dimensions::new(15, 15), DrawMode::Normal)
                .unwrap();
            assert_eq!(output, horizontal);
        }

        #[test]
        fn test_equal() {
            let kind = SplitKind::Equal;
            let dimension = Direction::Horizontal;

            // left rows all the same length
            let mut left_msg = Lines(vec![
                vec!["test"].try_into().unwrap(),
                vec!["ok", "so"].try_into().unwrap(),
                vec!["sync"].try_into().unwrap(),
            ]);
            let right_msg = Lines(vec![
                vec!["xx"].try_into().unwrap(),
                vec!["yyy"].try_into().unwrap(),
                vec!["z"].try_into().unwrap(),
            ]);
            let splitter = Split::new(
                vec![Echo(left_msg.clone()), Echo(right_msg.clone())],
                dimension,
                kind.clone(),
            );
            let horizontal = Lines(vec![
                vec!["test", &" ".repeat(6), "xx", &" ".repeat(8)]
                    .try_into()
                    .unwrap(),
                vec!["ok", "so", &" ".repeat(6), "yyy", &" ".repeat(7)]
                    .try_into()
                    .unwrap(),
                vec!["sync", &" ".repeat(6), "z", &" ".repeat(9)]
                    .try_into()
                    .unwrap(),
            ]);

            let output = splitter
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();
            assert_eq!(output, horizontal);

            // uneven left row
            left_msg.0[0] = vec!["hello worl"].try_into().unwrap();
            let horizontal = Lines(vec![
                vec!["hello worl", "xx", &" ".repeat(8)].try_into().unwrap(),
                vec!["ok", "so", &" ".repeat(6), "yyy", &" ".repeat(7)]
                    .try_into()
                    .unwrap(),
                vec!["sync", &" ".repeat(6), "z", &" ".repeat(9)]
                    .try_into()
                    .unwrap(),
            ]);

            let splitter = Split::new(vec![Echo(left_msg), Echo(right_msg)], dimension, kind);

            let output = splitter
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();
            assert_eq!(output, horizontal);
        }

        #[test]
        fn test_many_sized() {
            let msg1 = Lines(vec![
                vec!["test", "ok"].try_into().unwrap(),
                vec!["also"].try_into().unwrap(),
            ]);
            let msg2 = Lines(vec![vec!["hola"].try_into().unwrap()]);
            let msg3 = Lines(vec![vec!["way way way way too long"].try_into().unwrap()]);
            let splitter = Split::new(
                vec![Echo(msg1), Echo(msg2), Echo(msg3)],
                Direction::Horizontal,
                SplitKind::Sized(vec![0.25, 0.5, 0.25]),
            );

            let output = splitter
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();

            let expected = Lines(vec![
                vec!["test", "o", "hola", &" ".repeat(6), "way w"]
                    .try_into()
                    .unwrap(),
                vec!["also", " ", &" ".repeat(10), &" ".repeat(5)]
                    .try_into()
                    .unwrap(),
            ]);

            assert_eq!(output, expected);
        }
    }

    mod vertical {
        use super::*;
        use crate::Dimensions;
        use crate::components::echo::Echo;

        #[test]
        fn test_equal() {
            let kind = SplitKind::Equal;
            let dimension = Direction::Vertical;

            let top = Lines(vec![
                vec!["Line 1"].try_into().unwrap(),
                vec!["Line 2222"].try_into().unwrap(),
            ]);
            let bottom = Lines(vec![
                vec!["Line 11"].try_into().unwrap(),
                vec!["Line 12"].try_into().unwrap(),
                vec!["Last line just kiddi"].try_into().unwrap(),
            ]);
            let splitter = Split::new(
                vec![Echo(top.clone()), Echo(bottom.clone())],
                dimension,
                kind,
            );

            let mut output = top;
            output.extend(iter::repeat_n(Line::default(), 8));
            output.extend(bottom);
            output.extend(iter::repeat_n(Line::default(), 7));

            let drawn = splitter
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();

            assert_eq!(drawn, output);
        }

        #[test]
        fn test_adaptive() {
            let kind = SplitKind::Adaptive;
            let dimension = Direction::Vertical;

            let top = Lines(vec![
                vec!["Line 1"].try_into().unwrap(),
                vec!["Line 2222"].try_into().unwrap(),
            ]);
            let bottom = Lines(vec![
                vec!["Line 11"].try_into().unwrap(),
                vec!["Line 12"].try_into().unwrap(),
                vec!["Last line just kiddi"].try_into().unwrap(),
            ]);
            let splitter = Split::new(
                vec![Echo(top.clone()), Echo(bottom.clone())],
                dimension,
                kind,
            );

            let mut output = top;
            output.extend(bottom);

            let drawn = splitter
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();
            assert_eq!(drawn, output);
        }

        #[test]
        fn test_many_sized() {
            let msg1 = Lines(vec![
                vec!["line1"].try_into().unwrap(),
                vec!["line2"].try_into().unwrap(),
                vec!["line3"].try_into().unwrap(),
                vec!["line4"].try_into().unwrap(),
                vec!["line5"].try_into().unwrap(),
                vec!["line6"].try_into().unwrap(),
            ]);
            let msg2 = Lines(vec![vec!["line7"].try_into().unwrap()]);
            let msg3 = Lines(vec![vec!["line8"].try_into().unwrap()]);
            let splitter = Split::new(
                vec![Echo(msg1), Echo(msg2), Echo(msg3)],
                Direction::Vertical,
                SplitKind::Sized(vec![0.25, 0.5, 0.25]),
            );

            let output = splitter
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();

            let expected = Lines(vec![
                vec!["line1"].try_into().unwrap(),
                vec!["line2"].try_into().unwrap(),
                vec!["line3"].try_into().unwrap(),
                vec!["line4"].try_into().unwrap(),
                vec!["line5"].try_into().unwrap(),
                vec!["line7"].try_into().unwrap(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
                vec!["line8"].try_into().unwrap(),
                Line::default(),
                Line::default(),
                Line::default(),
                Line::default(),
            ]);

            assert_eq!(output, expected);
        }
    }

    mod panics {
        use super::*;
        use crate::Dimensions;
        use crate::components::Blank;

        #[test]
        fn test_no_children() {
            let lines = Split::<Blank>::new(Vec::new(), Direction::Horizontal, SplitKind::Equal)
                .draw(Dimensions::new(20, 20), DrawMode::Normal)
                .unwrap();
            assert!(lines.is_empty());
        }

        #[test]
        #[should_panic(expected = "There must be an equal number of ratios and children.")]
        fn test_different_ratio_count() {
            Split::new(
                vec![Box::new(Blank), Box::new(Blank), Box::new(Blank)],
                Direction::Vertical,
                SplitKind::Sized(vec![0.4, 0.4]),
            );
        }
    }
}
