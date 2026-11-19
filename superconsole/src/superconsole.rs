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
use std::env;
use std::io;

use crossterm::QueueableCommand;
use crossterm::cursor;
use crossterm::cursor::MoveToColumn;
use crossterm::cursor::MoveUp;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use crossterm::tty::IsTty;

use crate::Dimensions;
use crate::Direction;
use crate::Lines;
use crate::ansi_support::enable_ansi_support;
use crate::components::Component;
use crate::components::DrawMode;
use crate::content::Line;
use crate::error::OutputError;
use crate::output::BlockingSuperConsoleOutput;
use crate::output::OutputTarget;
use crate::output::SuperConsoleOutput;

const MINIMUM_EMIT: usize = 5;
const MAX_GRAPHEME_BUFFER: usize = 1000000;

/// Handles rendering the console using the user-defined [Component](Component)s and emitted messages.
/// A Canvas area at the bottom of the terminal is re-rendered in place at each tick for the components,
/// while a log area of emitted messages is produced above.
/// Producing output from sources other than SuperConsole will break the TUI.
pub struct SuperConsole {
    /// Number of lines that were used to render the canvas last time.
    canvas_contents: Lines,
    /// Buffer storing the lines (stderr by default) we should emit next time we render.
    to_emit: Lines,
    // Buffer storing auxillary output (stdio by default) that should be emitted next time we render.
    aux_to_emit: Lines,
    /// A default screen size to use if the size cannot be fetched
    /// from the terminal. This generally is only used for testing
    /// situations.
    fallback_size: Option<Dimensions>,
    /// The terminal handle to write a buffer to the screen.
    /// All IO goes through this handle.
    /// It's None only during drop/finalize.
    output: Option<Box<dyn SuperConsoleOutput>>,
}

impl SuperConsole {
    /// Build a new SuperConsole with a root component.
    pub fn new() -> Option<Self> {
        Self::compatible().then(|| {
            Self::new_with_output(
                None,
                Box::new(BlockingSuperConsoleOutput::new(
                    Box::new(io::stderr()),
                    Box::new(io::stdout()),
                )),
            )
        })
    }

    /// Force a new SuperConsole to be built with a root component, regardless of
    /// whether the tty is compatible
    pub fn forced_new(fallback_size: Dimensions) -> Self {
        Self::new_with_output(
            Some(fallback_size),
            Box::new(BlockingSuperConsoleOutput::new(
                Box::new(io::stderr()),
                Box::new(io::stdout()),
            )),
        )
    }

    pub(crate) fn new_with_output(
        fallback_size: Option<Dimensions>,
        output: Box<dyn SuperConsoleOutput>,
    ) -> Self {
        Self {
            canvas_contents: Lines::new(),
            to_emit: Lines::new(),
            fallback_size,
            output: Some(output),
            aux_to_emit: Lines::new(),
        }
    }

    pub(crate) fn output(&self) -> &dyn SuperConsoleOutput {
        &**self
            .output
            .as_ref()
            .expect("output accessed after taken in finalize/drop")
    }

    pub(crate) fn output_mut(&mut self) -> &mut dyn SuperConsoleOutput {
        &mut **self
            .output
            .as_mut()
            .expect("output accessed after taken in finalize/drop")
    }

    pub fn compatible() -> bool {
        // Superconsole only renders on the stderr, so we can display the superconsole
        // even if someone does `command > out.txt`.
        io::stderr().is_tty() && !Self::is_term_dumb() && enable_ansi_support()
    }

    fn is_term_dumb() -> bool {
        // Some terminals (e.g. Emacs' eshell) are very limited in functionality and don't support
        // the control codes required for superconsole to work.
        matches!(env::var("TERM"), Ok(kind) if kind == "dumb")
    }

    /// Render at a given tick.  Draws all components and drains the emitted events buffer.
    /// This will produce any pending emitting events above the Canvas and will re-render the drawing area.
    pub fn render<C: Component + ?Sized>(&mut self, root: &C) -> crate::RenderResult<(), C> {
        // `render_general` refuses to drain more than a single frame, so repeat until done.
        // or until the rendered frame is too large to print anything.
        let mut anything_emitted = true;
        let mut has_rendered = false;
        while !has_rendered
            || (anything_emitted && !(self.to_emit.is_empty() && self.aux_to_emit.is_empty()))
        {
            if !self.output_mut().should_render() {
                break;
            }

            let last_len = self.to_emit.len();
            let last_aux_len = self.aux_to_emit.len();
            self.render_with_mode(root, DrawMode::Normal)?;
            anything_emitted =
                last_len != self.to_emit.len() || last_aux_len != self.aux_to_emit.len();
            has_rendered = true;
        }

        Ok(())
    }

    /// Perform a final render with [`DrawMode::Final`].
    /// Each component will have a chance to finalize themselves before the terminal is disposed of.
    pub fn finalize<C: Component + ?Sized>(self, root: &C) -> crate::RenderResult<(), C> {
        self.finalize_with_mode(root, DrawMode::Final)
    }

    /// Perform a final render, using a specified [`DrawMode`].
    /// Each component will have a chance to finalize themselves before the terminal is disposed of.
    pub fn finalize_with_mode<C: Component + ?Sized>(
        mut self,
        root: &C,
        mode: DrawMode,
    ) -> crate::RenderResult<(), C> {
        self.render_with_mode(root, mode)?;

        // We must not call anything that will access output after this.
        self.output.take().unwrap().finalize().map_err(Into::into)
    }

    /// Convenience method:
    /// - Calls queue_emit to add the lines.
    /// - Next, re-renders the `superconsole`.
    ///
    /// Because this re-renders the console, it requires passed state.
    /// Overuse of this method can cause `superconsole` to use significant CPU.
    pub fn emit_now<C: Component + ?Sized>(
        &mut self,
        lines: Lines,
        root: &C,
    ) -> crate::RenderResult<(), C> {
        self.emit(lines);
        self.render(root)
    }

    /// Queues the passed lines to be drawn on the next render.
    /// The lines *will not* appear until the next render is called.
    pub fn emit(&mut self, lines: Lines) {
        self.to_emit.extend(lines);
    }

    /// Queues the passed lines of auxillary output to be drawn on the next render.
    /// The lines *will not* appear until the next render is called.
    pub fn emit_aux(&mut self, lines: Lines) {
        self.aux_to_emit.extend(lines);
    }

    fn size(&self) -> Result<Dimensions, OutputError> {
        if let Ok(width) = std::env::var("SUPERCONSOLE_TESTING_WIDTH") {
            let width: usize = width.parse().unwrap();
            let height = std::env::var("SUPERCONSOLE_TESTING_HEIGHT")
                .unwrap()
                .parse()
                .unwrap();
            return Ok(Dimensions::new(width, height));
        }
        // We want to get the size, but if that fails or is empty use the fallback_size if available.
        match (self.output().terminal_size(), self.fallback_size) {
            (Ok(size), Some(fallback)) if size.width == 0 || size.height == 0 => Ok(fallback),
            (Ok(size), _) => Ok(size),
            (Err(_), Some(fallback)) => Ok(fallback),
            (Err(e), None) => Err(e),
        }
    }

    /// The first step of drawing.  It moves the buffer up to be overwritten and sets the length to 0.
    /// This is used to clear the scratch area so that any possibly emitted messages can write over it.
    pub(crate) fn clear_canvas_pre(
        writer: &mut Vec<u8>,
        mut height: usize,
    ) -> Result<(), OutputError> {
        while height > 0 {
            // We can only move up at most u16 at a time, so repeat until we move up enough
            let step = height.try_into().unwrap_or(u16::MAX);
            writer.queue(MoveUp(step)).map_err(OutputError::Terminal)?;
            height -= step as usize;
        }
        writer
            .queue(MoveToColumn(0))
            .map_err(OutputError::Terminal)?;
        Ok(())
    }

    /// The last step of drawing. Ensures there is nothing else below on the console.
    /// Important in case the new canvas was smaller than the last.
    pub(crate) fn clear_canvas_post(writer: &mut Vec<u8>) -> Result<(), OutputError> {
        writer
            .queue(Clear(ClearType::FromCursorDown))
            .map_err(OutputError::Terminal)?;
        Ok(())
    }

    /// Clears the canvas portion of the superconsole.
    pub fn clear(&mut self) -> Result<(), OutputError> {
        let mut buffer = Vec::new();
        Self::clear_canvas_pre(&mut buffer, self.canvas_contents.len())?;
        self.canvas_contents = Lines::new();
        Self::clear_canvas_post(&mut buffer)?;
        self.output_mut().output(buffer).map_err(Into::into)
    }

    /// Helper method to share render + finalize behavior by specifying mode.
    fn render_with_mode<C: Component + ?Sized>(
        &mut self,
        root: &C,
        mode: DrawMode,
    ) -> crate::RenderResult<(), C> {
        // TODO(cjhopman): We may need to try to keep each write call to be under the pipe buffer
        // size so it can be completed in a single syscall otherwise we might see a partially
        // rendered frame.

        // We remove the last line as we always have a blank final line in our output.
        let size = self.size()?.saturating_sub(1, Direction::Vertical);

        self.render_general(root, mode, size)?;

        Ok(())
    }

    /// Helper method that makes rendering highly configurable.
    fn render_general<C: Component + ?Sized>(
        &mut self,
        root: &C,
        mode: DrawMode,
        size: Dimensions,
    ) -> crate::RenderResult<(), C> {
        /// Heuristic to determine if a buffer is too large to buffer.
        /// Can be tuned, but is currently set to 1000000 graphemes.
        fn is_big(buf0: &Lines, buf1: &Lines) -> bool {
            let len0: usize = buf0.iter().map(Line::len).sum();
            let len1: usize = buf1.iter().map(Line::len).sum();
            (len0 + len1) > MAX_GRAPHEME_BUFFER
        }
        // Pre-draw the frame *and then* start rendering emitted messages.
        let mut canvas = root.draw(size, mode).map_err(crate::Error::Draw)?;
        // We don't trust the child to not truncate the result.
        canvas.shrink_lines_to_dimensions(size);

        // The closure to compute the limit to render the emit lines above the canvas.
        // Render at most a single frame if this not the last render.
        // Does not buffer if there is a ridiculous amount of data.
        let compute_limit = |buf0: &Lines, buf1: &Lines| {
            match mode {
                DrawMode::Normal if !is_big(buf0, buf1) => {
                    let limit = size.height.saturating_sub(canvas.len());
                    // arbitrary value picked so we don't starve `emit` on small terminal sizes.
                    Some(cmp::max(limit, MINIMUM_EMIT))
                }
                _ => None,
            }
        };

        // Render at most a single frame if this not the last render.
        // Does not buffer if there is a ridiculous amount of data.
        let mut limit = compute_limit(&self.to_emit, &self.aux_to_emit);

        // How much of the canvas hasn't changed, so I can avoid overwriting
        // and thus avoid flickering things like URL's in VS Code terminal.
        let reuse_prefix = if self.to_emit.is_empty() && self.aux_to_emit.is_empty() {
            self.canvas_contents.lines_equal(&canvas)
        } else {
            0
        };

        let mut buffer = Vec::new();
        buffer.queue(cursor::Hide).map_err(OutputError::Terminal)?;

        Self::clear_canvas_pre(&mut buffer, self.canvas_contents.len() - reuse_prefix)?;

        if !self.aux_to_emit.is_empty() {
            if self.output().aux_stream_is_tty() {
                // If we have aux_to_emit and the aux stream is tty, we need to output the main output (stderr by default) first
                // and flushed, so that we can make sure the all output order is correct.
                self.output_mut().output(buffer)?;

                let mut aux_buffer = Vec::new();
                limit = self.aux_to_emit.render_with_limit(&mut aux_buffer, limit);
                self.output_mut().output_to(aux_buffer, OutputTarget::Aux)?;

                // Since output is moved at `self.output.output(buffer)`, we need to new a new buffer
                buffer = Vec::new();
            } else {
                // If the aux stream is not tty, we don't need to render the line, we just output to the auxillary output
                let mut output_buffer = Vec::new();
                self.aux_to_emit.render_raw(&mut output_buffer);
                self.output_mut()
                    .output_to(output_buffer, OutputTarget::Aux)?;

                // Since we clear the aux_to_emit, we need to recompute the `limit`
                limit = compute_limit(&self.to_emit, &self.aux_to_emit);
            }
        }

        self.to_emit.render_with_limit(&mut buffer, limit);

        canvas.render_from_line(&mut buffer, reuse_prefix);

        buffer.queue(cursor::Show).map_err(OutputError::Terminal)?;
        Self::clear_canvas_post(&mut buffer)?;
        self.canvas_contents = canvas;

        self.output_mut().output(buffer)?;

        Ok(())
    }
}

impl Drop for SuperConsole {
    fn drop(&mut self) {
        if let Some(v) = &mut self.output {
            let mut buffer = Vec::new();
            drop(buffer.queue(cursor::Show));
            drop(v.output(buffer));
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context as _;
    use derive_more::AsRef;

    use super::*;
    use crate::components::echo::Echo;
    use crate::testing::SuperConsoleTestingExt;
    use crate::testing::TestOutput;
    use crate::testing::frame_contains;
    use crate::testing::test_console;
    use crate::testing::test_console_aux_incompatible;

    #[derive(AsRef, Debug)]
    #[allow(dead_code)]
    struct Msg(Lines);

    #[test]
    fn test_small_buffer() -> anyhow::Result<()> {
        let mut console = test_console();
        let msg_count = MINIMUM_EMIT + 5;
        console.emit(Lines(vec![vec!["line 1"].try_into()?; msg_count]));
        let msg = Lines(vec![vec!["line"].try_into()?; msg_count]);
        let root = Echo(msg);

        // even though the canvas is larger than the tty
        console.render_general(&root, DrawMode::Normal, Dimensions::new(100, 2))?;

        // we should still drain a minimum of 5 messages.
        assert_eq!(console.to_emit.len(), msg_count - MINIMUM_EMIT);

        Ok(())
    }

    #[test]
    fn test_huge_buffer() -> anyhow::Result<()> {
        let mut console = test_console();
        console.emit(Lines(vec![
            vec!["line 1"].try_into()?;
            MAX_GRAPHEME_BUFFER * 2
        ]));
        let root = Echo(Lines(vec![vec!["line"].try_into()?]));

        // Even though we have more messages than fit on the screen in the `to_emit` buffer
        console.render_general(&root, DrawMode::Normal, Dimensions::new(100, 20))?;

        // We have so many that we should just drain them all.
        assert!(console.to_emit.is_empty());

        Ok(())
    }

    /// Check that no frames are produced when should_render returns false.
    #[test]
    fn test_block_render() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        console.render(&root)?;
        assert_eq!(console.test_output().frames.len(), 1);

        console.test_output_mut().should_render = false;
        console.render(&root)?;
        assert_eq!(console.test_output().frames.len(), 1);

        console.emit(Lines(vec![vec!["line 1"].try_into()?]));
        console.render(&root)?;
        assert_eq!(console.test_output().frames.len(), 1);

        Ok(())
    }

    /// Check that lines are deferred when should_render returns false, and emitted once the output
    /// is unblocked.
    #[test]
    fn test_block_lines() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        console.test_output_mut().should_render = false;
        console.emit(Lines(vec![vec!["line 1"].try_into()?]));
        console.render(&root)?;
        assert_eq!(console.test_output().frames.len(), 0);

        console.test_output_mut().should_render = true;
        console.emit(Lines(vec![vec!["line 2"].try_into()?]));
        console.render(&root)?;

        let frame = console
            .test_output_mut()
            .frames
            .pop()
            .context("No frame was emitted")?;

        assert!(frame_contains(&frame, "state"));
        assert!(frame_contains(&frame, "line 1"));
        assert!(frame_contains(&frame, "line 2"));

        Ok(())
    }

    /// Check that render_with_mode does not respect should_render.
    #[test]
    fn test_block_finalize() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        console.test_output_mut().should_render = false;
        console.emit(Lines(vec![vec!["line 1"].try_into()?]));
        console.emit(Lines(vec![vec!["line 2"].try_into()?]));
        console.render_with_mode(&root, DrawMode::Final)?;

        let frame = console
            .test_output_mut()
            .frames
            .pop()
            .context("No frame was emitted")?;

        assert!(frame_contains(&frame, "state"));
        assert!(frame_contains(&frame, "line 1"));
        assert!(frame_contains(&frame, "line 2"));

        Ok(())
    }

    #[test]
    fn test_reuse_buffer() -> anyhow::Result<()> {
        let mut console = test_console();

        console.render(&Echo(Lines(vec![
            vec!["http://example.com/ link"].try_into()?,
            vec!["number 1, special 1"].try_into()?,
        ])))?;
        console.render(&Echo(Lines(vec![
            vec!["http://example.com/ link"].try_into()?,
            vec!["number 2, special 2"].try_into()?,
        ])))?;
        console.emit(Lines(vec![vec!["special 3"].try_into()?]));
        console.render(&Echo(Lines(vec![
            vec!["http://example.com/ link"].try_into()?,
            vec!["number 3"].try_into()?,
        ])))?;
        console.render(&Echo(Lines(vec![
            vec!["http://example.com/ link"].try_into()?,
            vec!["special 4"].try_into()?,
            vec!["number 4"].try_into()?,
        ])))?;

        let frames = &console.test_output().frames;
        assert_eq!(frames.len(), 4);
        // We expect the URL to be omitted on some frames, because it didn't change.
        let expect_url = [0, 2];
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame_contains(frame, "http://example.com/"),
                expect_url.contains(&i),
            );
            assert!(frame_contains(frame, format!("number {}", i + 1)));
            assert!(frame_contains(frame, format!("special {}", i + 1)));
        }
        Ok(())
    }

    #[test]
    fn test_emit_aux() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        // Emit auxiliary output
        console.emit_aux(Lines(vec![vec!["aux line 1"].try_into()?]));
        console.render(&root)?;

        // Since we emit aux, we don't output the whole frame once in TestConsole
        let frame: Vec<u8> = console
            .test_output_mut()
            .frames
            .iter()
            .flatten()
            .copied()
            .collect();

        assert!(frame_contains(&frame, "state"));
        assert!(frame_contains(
            &frame,
            TestOutput::aux_output_with_prefix("aux line 1"),
        ));

        Ok(())
    }

    #[test]
    fn test_emit_aux_multiple_lines() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        // Emit multiple auxiliary output lines
        console.emit_aux(Lines(vec![
            vec!["aux line 1"].try_into()?,
            vec!["aux line 2"].try_into()?,
            vec!["aux line 3"].try_into()?,
        ]));
        console.render(&root)?;

        // Since we emit aux, we don't output the whole frame once in TestConsole
        let frame: Vec<u8> = console
            .test_output_mut()
            .frames
            .iter()
            .flatten()
            .copied()
            .collect();

        println!("frame: {:?}", String::from_utf8_lossy(&frame));

        assert!(frame_contains(&frame, "state"));
        assert!(frame_contains(
            &frame,
            TestOutput::aux_output_with_prefix("aux line 1")
        ));
        // Only the first line of a render of aux output is prefixed
        assert!(frame_contains(&frame, "aux line 2"));
        assert!(frame_contains(&frame, "aux line 3"));

        Ok(())
    }

    #[test]
    fn test_block_aux_lines() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        // Block rendering and emit auxiliary output
        console.test_output_mut().should_render = false;
        console.emit_aux(Lines(vec![vec!["aux line 1"].try_into()?]));
        console.render(&root)?;
        assert_eq!(console.test_output().frames.len(), 0);

        // Unblock rendering and check if auxiliary output is rendered
        console.test_output_mut().should_render = true;
        console.emit_aux(Lines(vec![vec!["aux line 2"].try_into()?]));
        console.render(&root)?;

        // Since we emit aux, we don't output the whole frame once in TestConsole
        let frame: Vec<u8> = console
            .test_output_mut()
            .frames
            .iter()
            .flatten()
            .copied()
            .collect();

        assert!(frame_contains(&frame, "state"));
        assert!(frame_contains(
            &frame,
            TestOutput::aux_output_with_prefix("aux line 1")
        ));
        assert!(frame_contains(&frame, "aux line 2"));

        Ok(())
    }

    #[test]
    fn test_emit_and_emit_aux() -> anyhow::Result<()> {
        let mut console = test_console();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));

        // Emit both regular and auxiliary output
        console.emit(Lines(vec![vec!["regular line 1"].try_into()?]));
        console.emit(Lines(vec![vec!["regular line 2"].try_into()?]));
        console.emit_aux(Lines(vec![vec!["aux line 1"].try_into()?]));
        console.emit_aux(Lines(vec![vec!["aux line 2"].try_into()?]));
        console.render(&root)?;

        // Since we emit aux, we don't output the whole frame once in TestConsole
        let frame: Vec<u8> = console
            .test_output_mut()
            .frames
            .iter()
            .flatten()
            .copied()
            .collect();

        assert!(frame_contains(&frame, "state"));
        assert!(frame_contains(&frame, "regular line 1"));
        assert!(frame_contains(&frame, "regular line 2"));
        assert!(frame_contains(
            &frame,
            TestOutput::aux_output_with_prefix("aux line 1")
        ));
        assert!(frame_contains(&frame, "aux line 2"));

        Ok(())
    }

    #[test]
    fn test_emit_aux_with_aux_incompatible_output() -> anyhow::Result<()> {
        let mut console = test_console_aux_incompatible();

        let root = Echo(Lines(vec![vec!["state"].try_into()?]));
        console.emit_aux(Lines(vec![vec!["aux line 1"].try_into()?]));
        console.emit_aux(Lines(vec![vec!["aux line 2"].try_into()?]));
        console.render(&root)?;

        // Since we emit aux, we don't output the whole frame once in TestConsole
        let frame: Vec<u8> = console
            .test_output_mut()
            .frames
            .iter()
            .flatten()
            .copied()
            .collect();
        println!("frame: {:?}", String::from_utf8_lossy(&frame));
        assert!(frame_contains(&frame, "state"));
        // Since aux output is incompatible with tty, we don't output the any escape sequence to clear the line.
        assert!(frame_contains(&frame, "aux line 1\naux line 2"));

        Ok(())
    }

    /// `render` keeps drawing frames until the emitted lines are drained.
    #[test]
    fn test_render_drains_emitted_lines() -> anyhow::Result<()> {
        let mut console = test_console();
        console.emit(Lines(vec![vec!["log line"].try_into()?; 200]));
        let root = Echo(Lines(vec![vec!["status"].try_into()?]));
        console.render(&root)?;
        assert!(console.test_output().frames.len() > 1);
        assert!(console.to_emit.is_empty());
        Ok(())
    }
}
