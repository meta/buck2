/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Getting started truly from scratch is excessive for every user of superconsole.
//! A small set of starter components are provided, exposed below.

use std::fmt::Debug;

pub use alignment::Aligned;
pub use blank::Blank;
pub use bordering::Bordered;
pub use bounding::Bounded;
pub use padding::Padded;
pub use spinner::Spinner;
pub use splitting::Split;

use crate::Dimensions;
use crate::Lines;
pub use crate::components::draw_horizontal::DrawHorizontal;
pub use crate::components::draw_vertical::DrawVertical;
pub use crate::components::spinner::BRAILLE_SPINNER;

pub mod alignment;
mod blank;
pub mod bordering;
mod bounding;
mod draw_horizontal;
mod draw_vertical;
pub(crate) mod echo;
pub mod padding;
pub mod spinner;
pub mod splitting;

/// Used to mark whether a draw is final.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum DrawMode {
    Normal,
    Final,
}

/// Components are pluggable drawers that output lines of formatted text.
/// They are composable (eventually) and re-render in place at each render.
pub trait Component {
    type Error;

    /// This method is to be implemented for components to provide the `draw` method.
    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, Self::Error>;

    /// Interprets the current caller state to create its drawing.
    /// Dimensions refers to the maximum (width, height) this component may use.
    /// The mode refers to if this is the final time the component will be drawn.
    /// If a child component is too large to fit in the dimensions, it is truncated.
    fn draw(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, Self::Error> {
        let mut res = self.draw_unchecked(dimensions, mode)?;
        res.shrink_lines_to_dimensions(dimensions);
        Ok(res)
    }
}

impl<E> Component for Box<dyn Component<Error = E>> {
    type Error = E;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, E> {
        (**self).draw_unchecked(dimensions, mode)
    }
}

impl<E> Component for Box<dyn Component<Error = E> + Send> {
    type Error = E;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, E> {
        (**self).draw_unchecked(dimensions, mode)
    }
}

// TODO(nga): this is not really needed.
impl<C: Component> Component for Box<C> {
    type Error = C::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, C::Error> {
        (**self).draw_unchecked(dimensions, mode)
    }
}

impl<E> Component for &dyn Component<Error = E> {
    type Error = E;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, E> {
        (**self).draw_unchecked(dimensions, mode)
    }
}

impl<E> Component for &(dyn Component<Error = E> + Send) {
    type Error = E;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, E> {
        (**self).draw_unchecked(dimensions, mode)
    }
}

impl<C: Component> Component for &C {
    type Error = C::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> Result<Lines, C::Error> {
        (**self).draw_unchecked(dimensions, mode)
    }
}
