/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use futures::pin_mut;
use serde::Deserialize;
use serde::Serialize;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::SuperConsole;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use tokio::select;
use tokio::time;

use crate::console::Control;
use crate::console::Stdin;
use crate::console::control_reader;
use crate::console::emit_unknown_control;

pub mod blocks;
pub mod breakout;
pub mod jump;
pub mod life;
pub mod menu;
pub mod minesweeper;
pub mod snake;
pub mod sokoban;
pub mod r#static;
pub mod twenty48;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    pub category: String,
    pub value: u64,
    pub lower_is_better: bool,
}

pub struct TickResult {
    pub alive: bool,
    pub scores: Vec<Score>,
}

/// Core trait for all games. Implementors provide tick-based updates, input
/// handling, and rendering (via [`Component`]).
pub trait Game: Component<Error = anyhow::Error> + Send {
    /// Advance the game by one tick.
    fn tick(&mut self, tick_count: u32) -> TickResult;
    fn input(&mut self, input: Control) -> Option<Control>;
    /// Report final scores at game over. Default: empty.
    fn scores(&self) -> Vec<Score> {
        Vec::new()
    }
    /// Serialize the game state to a JSON string, if supported.
    fn save_state(&self) -> Option<String> {
        None
    }
    /// Restore game state from a JSON string. Returns true on success.
    fn load_state(&mut self, _json: &str) -> bool {
        false
    }
    /// Message to show in the game over banner. Return `None` to skip the banner.
    fn game_over_message(&self) -> Option<&str> {
        Some("Game Over!")
    }
}

/// Wraps a component to ensure its output is at least `min_height` lines tall,
/// padding with empty lines at the bottom if needed.
pub struct FixedHeight<'a, C: Component + ?Sized> {
    child: &'a C,
    min_height: usize,
}

impl<'a, C: Component + ?Sized> FixedHeight<'a, C> {
    pub fn new(child: &'a C, min_height: usize) -> Self {
        Self { child, min_height }
    }
}

impl<C: Component<Error = anyhow::Error> + ?Sized> Component for FixedHeight<'_, C> {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        let mut output = self.child.draw_unchecked(dimensions, mode)?;
        while output.len() < self.min_height {
            output.0.push(Line::default());
        }
        Ok(output)
    }
}

pub enum ExitReason {
    Finished,
    Escape,
    GameOver(Vec<Score>),
}

/// Splice `overlay` into `line` at display column `col`, preserving content
/// on both sides. Uses `Span::len()` (display width) rather than grapheme
/// count so that wide characters like emoji are handled correctly.
fn splice_into(line: &Line, col: usize, overlay: &Line) -> Line {
    let overlay_width = overlay.len();
    let cut_end = col + overlay_width;

    let mut left = Line::default();
    let mut right = Line::default();
    let mut pos: usize = 0;

    for span in line.clone() {
        let span_width = span.len();
        let span_end = pos + span_width;

        if span_end <= col {
            // Entirely before overlay
            left.push(span);
        } else if pos < col {
            if span_end > cut_end {
                // Straddles BOTH edges — extract the right portion with styling
                left.push(span.clone());
                let skip = cut_end - pos;
                let keep = span_end - cut_end;
                let mut temp = Line::default();
                temp.push(span);
                temp.trim_ends(skip, keep);
                right.extend(temp);
            } else {
                // Straddles only the left edge
                left.push(span);
            }
        } else if pos >= cut_end {
            // Entirely after overlay
            right.push(span);
        } else if span_end > cut_end {
            // Starts within overlay, extends past right edge — extract tail
            let skip = cut_end - pos;
            let keep = span_end - cut_end;
            let mut temp = Line::default();
            temp.push(span);
            temp.trim_ends(skip, keep);
            right.extend(temp);
        }
        // else: entirely within overlay region — correctly replaced by overlay

        pos = span_end;
    }

    // Truncate/pad left to exactly `col` display columns
    left.to_exact_width(col);

    let mut result = left;
    result.extend(overlay.clone());
    result.extend(right);
    result
}

#[cfg(test)]
mod tests {
    use superconsole::Span;
    use superconsole::style::Color;

    use super::*;

    const HIDDEN_COLOR: Color = Color::White;

    fn number_color(n: u8) -> Color {
        match n {
            1 => Color::Blue,
            2 => Color::Green,
            3 => Color::Red,
            4 => Color::DarkBlue,
            _ => Color::White,
        }
    }

    /// Build a minesweeper-like board line: ║ + `width` hidden cells + ║
    fn make_board_line(width: usize) -> Line {
        let mut line = Line::default();
        line.push(Span::new_unstyled_lossy("║"));
        for _ in 0..width {
            line.push(Span::new_colored_lossy("███", HIDDEN_COLOR));
        }
        line.push(Span::new_unstyled_lossy("║"));
        line
    }

    /// Build a mixed line matching the user's example:
    /// ║ + 11 hidden + 6 revealed numbers + 13 hidden + ║
    fn make_mixed_line() -> Line {
        let mut line = Line::default();
        line.push(Span::new_unstyled_lossy("║"));
        for _ in 0..11 {
            line.push(Span::new_colored_lossy("███", HIDDEN_COLOR));
        }
        for &n in &[2u8, 1, 2, 2, 1, 1] {
            line.push(Span::new_unstyled_lossy(" "));
            line.push(Span::new_colored_lossy(&n.to_string(), number_color(n)));
            line.push(Span::new_unstyled_lossy(" "));
        }
        for _ in 0..13 {
            line.push(Span::new_colored_lossy("███", HIDDEN_COLOR));
        }
        line.push(Span::new_unstyled_lossy("║"));
        line
    }

    #[test]
    fn test_board_line_width() {
        let line = make_board_line(30);
        // 1 (║) + 30*3 (cells) + 1 (║) = 92
        assert_eq!(line.len(), 92);
    }

    #[test]
    fn test_mixed_line_width() {
        let line = make_mixed_line();
        // 1 + 11*3 + 6*3 + 13*3 + 1 = 1 + 33 + 18 + 39 + 1 = 92
        assert_eq!(line.len(), 92);
    }

    #[test]
    fn test_splice_preserves_width_all_hidden() {
        let line = make_board_line(30);
        let original_width = line.len();

        // Simulate an overlay centered on the line
        let overlay_text = "│  test overlay content here  │";
        let overlay = Line::sanitized(overlay_text);
        let overlay_width = overlay.len();
        let col = (original_width - overlay_width) / 2;

        let result = splice_into(&line, col, &overlay);
        assert_eq!(
            result.len(),
            original_width,
            "splice_into changed width from {} to {} (col={}, overlay_width={})",
            original_width,
            result.len(),
            col,
            overlay_width
        );
    }

    #[test]
    fn test_splice_preserves_width_mixed_line() {
        let line = make_mixed_line();
        let original_width = line.len();

        let overlay_text = "│  test overlay content here  │";
        let overlay = Line::sanitized(overlay_text);
        let overlay_width = overlay.len();
        let col = (original_width - overlay_width) / 2;

        let result = splice_into(&line, col, &overlay);
        assert_eq!(
            result.len(),
            original_width,
            "splice_into changed width from {} to {} (col={}, overlay_width={})",
            original_width,
            result.len(),
            col,
            overlay_width
        );
    }

    #[test]
    fn test_splice_preserves_width_with_paused_message() {
        let line = make_board_line(30);
        let original_width = line.len();

        // Use the actual paused overlay message
        let msg = " Paused \u{2014} press any key to continue ";
        let msg_display_width = Line::sanitized(msg).len();
        let inner_width = msg_display_width + 2;
        let box_width = inner_width + 2;

        // Test all overlay line variants
        let bar = "\u{2500}".repeat(inner_width);
        let texts = [
            format!("\u{256d}{bar}\u{256e}"),
            format!("\u{2502}{}\u{2502}", " ".repeat(inner_width)),
            format!("\u{2502} {msg} \u{2502}"),
            format!("\u{2570}{bar}\u{256f}"),
        ];

        let col = original_width.saturating_sub(box_width) / 2;

        for (i, text) in texts.iter().enumerate() {
            let mut overlay = Line::sanitized(text);
            overlay.to_exact_width(box_width);

            let result = splice_into(&line, col, &overlay);
            assert_eq!(
                result.len(),
                original_width,
                "Overlay line {} changed width from {} to {} (col={}, box_width={})",
                i,
                original_width,
                result.len(),
                col,
                box_width,
            );
        }
    }

    #[test]
    fn test_splice_preserves_width_various_positions() {
        let line = make_board_line(30);
        let original_width = line.len();
        let overlay = Line::sanitized("│ hello │");

        // Test overlay at various positions
        for col in [0, 1, 10, 26, 40, 50, 60] {
            if col + overlay.len() > original_width {
                continue;
            }
            let result = splice_into(&line, col, &overlay);
            assert_eq!(
                result.len(),
                original_width,
                "splice at col={} changed width from {} to {}",
                col,
                original_width,
                result.len()
            );
        }
    }

    #[test]
    fn test_render_overlay_box_preserves_all_line_widths() {
        let mut lines: Vec<Line> = Vec::new();
        // Build 20 board lines — a mix of all-hidden and mixed
        for i in 0..20 {
            if i == 10 {
                lines.push(make_mixed_line());
            } else {
                lines.push(make_board_line(30));
            }
        }
        let mut output = Lines(lines);
        let original_widths: Vec<usize> = output.0.iter().map(|l| l.len()).collect();

        render_overlay_box(&mut output, " Paused \u{2014} press any key to continue ");

        for (i, line) in output.0.iter().enumerate() {
            if i < original_widths.len() {
                assert_eq!(
                    line.len(),
                    original_widths[i],
                    "Line {} width changed from {} to {}",
                    i,
                    original_widths[i],
                    line.len()
                );
            }
        }
    }

    #[test]
    fn test_splice_simple_unstyled() {
        // Simple case: line of 20 dashes, overlay of 6 chars at col 7
        let line: Line = vec!["--------------------".to_owned()].try_into().unwrap();
        assert_eq!(line.len(), 20);

        let overlay = Line::sanitized("[TEST]");
        assert_eq!(overlay.len(), 6);

        let result = splice_into(&line, 7, &overlay);
        assert_eq!(result.len(), 20);
    }

    #[test]
    fn test_splice_overlay_at_edges() {
        let line = make_board_line(10);
        let original_width = line.len(); // 1 + 30 + 1 = 32
        let overlay = Line::sanitized("XX");

        // Overlay at the very start
        let result = splice_into(&line, 0, &overlay);
        assert_eq!(result.len(), original_width);

        // Overlay at the very end
        let result = splice_into(&line, original_width - 2, &overlay);
        assert_eq!(result.len(), original_width);
    }
}

/// Render a centered overlay box on top of game output.
pub fn render_overlay_box(output: &mut Lines, msg: &str) {
    let game_width = output.0.iter().map(|l| l.len()).max().unwrap_or(0);

    // Use actual display width rather than char count so that characters
    // like em-dash (which may occupy a different number of display columns
    // than code points) don't cause width mismatches between overlay lines.
    let msg_display_width = Line::sanitized(msg).len();
    let inner_width = msg_display_width + 2;
    let box_width = inner_width + 2;
    let bar = "\u{2500}".repeat(inner_width);
    let top = format!("\u{256d}{bar}\u{256e}");
    let space_row = format!("\u{2502}{}\u{2502}", " ".repeat(inner_width));
    let mid = format!("\u{2502} {msg} \u{2502}");
    let bot = format!("\u{2570}{bar}\u{256f}");

    let height = output.len();
    let start_row = height * 15 / 100;
    while output.len() < start_row + 5 {
        output.0.push(Line::default());
    }

    let col = game_width.saturating_sub(box_width) / 2;
    let overlay_lines = [&top, &space_row, &mid, &space_row, &bot];
    for (i, text) in overlay_lines.iter().enumerate() {
        let mut overlay = Line::sanitized(text);
        // Normalize all overlay lines to exactly box_width so that
        // splice_into computes the same cut_end for every row.
        overlay.to_exact_width(box_width);
        output.0[start_row + i] = splice_into(&output.0[start_row + i], col, &overlay);
    }
}

/// Wraps a game to render it with a centered "Paused" overlay box.
struct PausedOverlay<'a> {
    game: &'a dyn Game,
    min_height: Option<usize>,
}

impl Component for PausedOverlay<'_> {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        let mut output = self.game.draw_unchecked(dimensions, mode)?;
        if let Some(h) = self.min_height {
            while output.len() < h {
                output.0.push(Line::default());
            }
        }
        if !matches!(mode, DrawMode::Final) {
            render_overlay_box(&mut output, " Paused \u{2014} press any key to continue ");
        }
        Ok(output)
    }
}

/// Wraps a game to render it with a centered "Game Over" overlay box.
struct GameOverOverlay<'a> {
    game: &'a dyn Game,
    min_height: Option<usize>,
    message: String,
}

impl Component for GameOverOverlay<'_> {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        let mut output = self.game.draw_unchecked(dimensions, mode)?;
        if let Some(h) = self.min_height {
            while output.len() < h {
                output.0.push(Line::default());
            }
        }
        if !matches!(mode, DrawMode::Final) {
            let msg = format!(" {} \u{2014} press Esc to exit ", self.message);
            render_overlay_box(&mut output, &msg);
        }
        Ok(output)
    }
}

#[derive(Serialize, Deserialize)]
struct SaveEnvelope {
    tick_count: u32,
    state: String,
}

#[derive(Debug)]
pub struct NewRecord {
    pub category: String,
    pub value: u64,
    pub previous: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HighScoreEntry {
    pub value: u64,
    pub lower_is_better: bool,
}

/// Persistent per-game high score tracker, serialized as JSON.
#[derive(Default, Serialize, Deserialize)]
pub struct HighScores {
    scores: HashMap<String, HashMap<String, HighScoreEntry>>,
}

impl HighScores {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if let Some(parent) = path.parent() {
                drop(std::fs::create_dir_all(parent));
            }
            if let Ok(mut f) = std::fs::File::create(path) {
                drop(f.write_all(json.as_bytes()));
            }
        }
    }

    pub fn record(&mut self, game_name: &str, scores: &[Score]) -> Vec<NewRecord> {
        let mut records = Vec::new();
        let game_scores = self.scores.entry(game_name.to_owned()).or_default();
        for score in scores {
            let is_better = match game_scores.get(&score.category) {
                None => true,
                Some(existing) => {
                    if score.lower_is_better {
                        score.value < existing.value
                    } else {
                        score.value > existing.value
                    }
                }
            };
            if is_better {
                let previous = game_scores.get(&score.category).map(|e| e.value);
                game_scores.insert(
                    score.category.clone(),
                    HighScoreEntry {
                        value: score.value,
                        lower_is_better: score.lower_is_better,
                    },
                );
                records.push(NewRecord {
                    category: score.category.clone(),
                    value: score.value,
                    previous,
                });
            }
        }
        records
    }

    pub fn get(&self, game_name: &str) -> Option<&HashMap<String, HighScoreEntry>> {
        self.scores.get(game_name)
    }
}

fn save_state_to_file(game: &dyn Game, tick_count: u32, path: &Path) {
    if let Some(state) = game.save_state() {
        let envelope = SaveEnvelope { tick_count, state };
        if let Ok(json) = serde_json::to_string(&envelope) {
            if let Some(parent) = path.parent() {
                drop(std::fs::create_dir_all(parent));
            }
            if let Ok(mut f) = std::fs::File::create(path) {
                drop(f.write_all(json.as_bytes()));
            }
        }
    }
}

/// Main game loop: ticks the game at 30 fps, dispatches input, handles
/// pause/resume via Escape, and renders overlays for pause and game-over states.
pub async fn run_game(
    renderer: &mut SuperConsole,
    stdin: &mut Stdin,
    game: &mut dyn Game,
    min_height: Option<usize>,
    save_state_path: Option<&Path>,
    load_state_path: Option<&Path>,
    mut on_scores: impl FnMut(&[Score]),
) -> anyhow::Result<ExitReason> {
    let delay = Duration::from_millis(1000 / 30);
    let mut interval = time::interval(delay);

    let console_reader = control_reader(stdin);
    pin_mut!(console_reader);

    let mut tick_count = 0u32;

    // Try to load saved state
    let mut resumed = false;
    if let Some(path) = load_state_path {
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(envelope) = serde_json::from_str::<SaveEnvelope>(&data) {
                if game.load_state(&envelope.state) {
                    tick_count = envelope.tick_count;
                    resumed = true;
                }
            }
        }
    }

    // If we resumed from a save, render one frame paused and wait for a keypress.
    if resumed {
        // Run one tick so the game renders its current state.
        tick_count += 1;
        game.tick(tick_count);
        let paused = PausedOverlay { game, min_height };
        renderer.render(&paused).unwrap();

        // Wait for any keypress (or ctrl-c / EOF).
        select! {
            c = console_reader.next() => {
                match c {
                    None => { return Ok(ExitReason::Finished); }
                    Some(c) => {
                        let c = c?;
                        if matches!(c, Control::Escape) {
                            if let Some(path) = save_state_path {
                                save_state_to_file(game, tick_count, path);
                            }
                            return Ok(ExitReason::Escape);
                        }
                        // Any other key unpauses.
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                if let Some(path) = save_state_path {
                    save_state_to_file(game, tick_count, path);
                }
                return Ok(ExitReason::Finished);
            }
        }
        // Reset the interval so the first tick doesn't fire immediately.
        interval.reset();
    }

    loop {
        select! {
            _ = interval.tick() => {
                tick_count += 1;
                let tick_result = game.tick(tick_count);
                if !tick_result.scores.is_empty() {
                    on_scores(&tick_result.scores);
                }
                if !tick_result.alive {
                    // Game over: delete save file
                    if let Some(path) = save_state_path {
                        drop(std::fs::remove_file(path));
                    }
                    // Show game over overlay and wait for Esc
                    if let Some(msg) = game.game_over_message() {
                        let overlay = GameOverOverlay {
                            game: &*game,
                            min_height,
                            message: msg.to_owned(),
                        };
                        renderer.render(&overlay).unwrap();
                        loop {
                            select! {
                                c = console_reader.next() => {
                                    match c {
                                        None => break,
                                        Some(Ok(Control::Escape)) => break,
                                        _ => {}
                                    }
                                }
                                _ = tokio::signal::ctrl_c() => break,
                            }
                        }
                    }
                    return Ok(ExitReason::GameOver(game.scores()));
                }
                match min_height {
                    Some(h) => renderer.render(&FixedHeight::new(game, h)).unwrap(),
                    None => renderer.render(game).unwrap(),
                }
            }
            c = console_reader.next() => {
                match c {
                    None => { return Ok(ExitReason::Finished); }
                    Some(c) => {
                        let c = c?;
                        if matches!(c, Control::Escape) {
                            if let Some(path) = save_state_path {
                                save_state_to_file(game, tick_count, path);
                            }
                            return Ok(ExitReason::Escape);
                        }
                        if let Some(unhandled) = game.input(c) {
                            emit_unknown_control(renderer, unhandled)?;
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                if let Some(path) = save_state_path {
                    save_state_to_file(game, tick_count, path);
                }
                return Ok(ExitReason::Finished);
            }
        }
    }
}
