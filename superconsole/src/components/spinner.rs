/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A reusable spinner component for showing progress indicators.

use std::convert::Infallible;

use crate::Dimensions;
use crate::Line;
use crate::Lines;
use crate::Span;
use crate::components::Component;
use crate::components::DrawMode;

/// Default braille spinner characters used for animation.
pub const BRAILLE_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// A component that renders a spinner with a message.
///
/// The spinner animates through characters based on the `tick` value,
/// and displays a message `Line` alongside it.
///
/// # Example
/// ```ignore
/// use superconsole::{Line, SuperConsole};
/// use superconsole::components::Spinner;
///
/// let mut console = SuperConsole::new().unwrap();
/// let message = Line::sanitized("Loading...");
/// let spinner = Spinner::new(tick, message);
/// console.render(&spinner)?;
/// ```
pub struct Spinner {
    tick: usize,
    message: Line,
    spinner_chars: &'static [char],
}

impl Spinner {
    /// Create a new spinner with a message using the default braille spinner.
    pub fn new(tick: usize, message: Line) -> Self {
        Self {
            tick,
            message,
            spinner_chars: BRAILLE_SPINNER,
        }
    }

    /// Create a new spinner with custom spinner characters.
    pub fn with_chars(tick: usize, message: Line, spinner_chars: &'static [char]) -> Self {
        Self {
            tick,
            message,
            spinner_chars,
        }
    }

    /// Get the current spinner character based on the tick.
    fn current_char(&self) -> char {
        if self.spinner_chars.is_empty() {
            ' '
        } else {
            self.spinner_chars[self.tick % self.spinner_chars.len()]
        }
    }
}

impl Component for Spinner {
    type Error = Infallible;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> Result<Lines, Infallible> {
        match mode {
            DrawMode::Final => {
                // In final mode, just show the message without spinner
                Ok(Lines(vec![self.message.clone()]))
            }
            DrawMode::Normal => {
                // Prepend spinner character to the message
                let spinner_span = Span::sanitized(format!("{} ", self.current_char()));
                let mut line = Line::from_iter([spinner_span]);
                line.extend(self.message.clone());
                Ok(Lines(vec![line]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_cycles() {
        let message = Line::sanitized("test");
        let spinner = Spinner::new(0, message);
        assert_eq!(spinner.current_char(), '⠋');

        let message = Line::sanitized("test");
        let spinner = Spinner::new(1, message);
        assert_eq!(spinner.current_char(), '⠙');

        // Test wrapping
        let message = Line::sanitized("test");
        let spinner = Spinner::new(10, message);
        assert_eq!(spinner.current_char(), '⠋');
    }

    #[test]
    fn test_spinner_render() {
        let message = Line::sanitized("Loading...");
        let spinner = Spinner::new(0, message);
        let lines = spinner.draw_unchecked(Dimensions::new(80, 24), DrawMode::Normal);
        assert!(lines.is_ok());
        let lines = lines.unwrap();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_spinner_final_mode() {
        let message = Line::sanitized("Done!");
        let spinner = Spinner::new(0, message.clone());
        let lines = spinner.draw_unchecked(Dimensions::new(80, 24), DrawMode::Final);
        assert!(lines.is_ok());
        let lines = lines.unwrap();
        assert_eq!(lines.len(), 1);
        // Final mode shows message without spinner
    }

    #[test]
    fn test_custom_chars() {
        let chars: &'static [char] = &['|', '/', '-', '\\'];
        let message = Line::sanitized("test");
        let spinner = Spinner::with_chars(0, message, chars);
        assert_eq!(spinner.current_char(), '|');

        let message = Line::sanitized("test");
        let spinner = Spinner::with_chars(1, message, chars);
        assert_eq!(spinner.current_char(), '/');
    }
}
