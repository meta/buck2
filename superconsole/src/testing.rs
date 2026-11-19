/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Testing utilities for Superconsole.
use std::any::Any;

use crate::Dimensions;
use crate::error::OutputError;
use crate::output::OutputTarget;
use crate::output::SuperConsoleOutput;
use crate::superconsole::SuperConsole;

/// An output for testing that doesn't do real I/O.
pub struct TestOutput {
    /// Callers can modify this to indicate whether the output is blocked.
    pub should_render: bool,
    /// The size to report the terminal
    pub terminal_size: Dimensions,
    /// The frames that were written to this output.
    pub frames: Vec<Vec<u8>>,
    /// Whether auxiliary output is compatible with tty
    aux_stream_is_tty: bool,
}

impl TestOutput {
    fn aux_prefix() -> &'static str {
        "AUX PREFIX: "
    }

    pub fn aux_output_with_prefix(content: &str) -> String {
        format!("{}{}", Self::aux_prefix(), content)
    }
}

impl SuperConsoleOutput for TestOutput {
    fn should_render(&mut self) -> bool {
        self.should_render
    }

    fn output(&mut self, buffer: Vec<u8>) -> Result<(), OutputError> {
        self.frames.push(buffer);
        Ok(())
    }

    fn output_to(&mut self, buffer: Vec<u8>, target: OutputTarget) -> Result<(), OutputError> {
        match target {
            OutputTarget::Main => self.output(buffer),
            OutputTarget::Aux => {
                let output = Self::aux_prefix()
                    .as_bytes()
                    .iter()
                    .copied()
                    .chain(buffer.into_iter())
                    .collect::<Vec<u8>>();
                self.frames.push(output);
                Ok(())
            }
        }
    }

    fn aux_stream_is_tty(&self) -> bool {
        self.aux_stream_is_tty
    }

    fn terminal_size(&self) -> Result<Dimensions, OutputError> {
        Ok(self.terminal_size)
    }

    fn finalize(self: Box<Self>) -> Result<(), OutputError> {
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub trait SuperConsoleTestingExt {
    fn test_output(&self) -> &TestOutput;
    fn test_output_mut(&mut self) -> &mut TestOutput;
}

impl SuperConsoleTestingExt for SuperConsole {
    fn test_output(&self) -> &TestOutput {
        self.output()
            .as_any()
            .downcast_ref()
            .expect("Downcast failed")
    }

    fn test_output_mut(&mut self) -> &mut TestOutput {
        self.output_mut()
            .as_any_mut()
            .downcast_mut()
            .expect("Downcast failed")
    }
}

pub fn test_console() -> SuperConsole {
    test_console_inner(true)
}

pub fn test_console_aux_incompatible() -> SuperConsole {
    test_console_inner(false)
}

fn test_console_inner(aux_stream_is_tty: bool) -> SuperConsole {
    let size = Dimensions {
        width: 80,
        height: 80,
    };
    SuperConsole::new_with_output(
        Some(size),
        Box::new(TestOutput {
            should_render: true,
            terminal_size: size,
            frames: Vec::new(),
            aux_stream_is_tty,
        }),
    )
}

pub fn frame_contains(frame: &[u8], needle: impl AsRef<[u8]>) -> bool {
    let needle = needle.as_ref();
    for w in frame.windows(needle.len()) {
        if w == needle {
            return true;
        }
    }
    false
}

#[track_caller]
pub fn assert_frame_contains(frame: &[u8], needle: impl AsRef<[u8]>) {
    if !frame_contains(frame, needle.as_ref()) {
        panic!(
            "Expected frame to contain `{}`, but was:\n{}",
            String::from_utf8_lossy(needle.as_ref()),
            String::from_utf8_lossy(frame)
        );
    }
}
