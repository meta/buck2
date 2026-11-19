/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! 2048 tile-sliding puzzle with polished TUI rendering.
//!
//! Beyond a basic 2048:
//!
//! - **Direction-agnostic slide**: a single `slide(dx, dy)` function handles
//!   all 4 directions via a `cell_pos` mapping that translates (line_index,
//!   position) to board (x, y). No duplicated merge logic.
//!
//! - **Box-drawn tiles**: each tile renders as a 3-line box with rounded
//!   corners (`╭──╮ │val│ ╰──╯`), built span-by-span for correct width.
//!
//! - **Per-value color coding**: distinct terminal colors for each tile value
//!   from 2 through 2048+, making the board scannable at a glance.
//!
//! - **New-tile highlighting**: the most recently spawned tile renders bold
//!   so the player can immediately spot where it appeared.
//!
//! - **90/10 spawn distribution**: 90% chance of 2, 10% chance of 4,
//!   matching the original 2048 game's probabilities.

use serde::Deserialize;
use serde::Serialize;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;
use superconsole::style::Stylize;

use crate::console::Control;

const SIZE: usize = 4;
const TILE_INNER: usize = 6;
const TILE_WIDTH: usize = TILE_INNER + 2;
const PAD: usize = 1;
const CELL_WIDTH: usize = PAD + TILE_WIDTH + PAD;

fn tile_color(val: u32) -> Color {
    match val {
        2 => Color::White,
        4 => Color::Cyan,
        8 => Color::Green,
        16 => Color::Yellow,
        32 => Color::Magenta,
        64 => Color::Red,
        128 => Color::Blue,
        256 => Color::DarkCyan,
        512 => Color::DarkGreen,
        1024 => Color::DarkYellow,
        2048 => Color::DarkRed,
        _ => Color::Grey,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    board: [[u32; SIZE]; SIZE],
    score: u32,
    game_over: bool,
    #[serde(default)]
    last_spawned: Option<(usize, usize)>,
}

impl Game {
    pub fn new() -> Self {
        let mut game = Self {
            board: [[0; SIZE]; SIZE],
            score: 0,
            game_over: false,
            last_spawned: None,
        };
        game.spawn_tile();
        game.spawn_tile();
        game
    }

    pub fn render_test() -> Self {
        let values = [
            2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 2,
        ];
        let mut board = [[0u32; SIZE]; SIZE];
        for (i, &v) in values.iter().enumerate() {
            board[i / SIZE][i % SIZE] = v;
        }
        Self {
            board,
            score: 0,
            game_over: false,
            last_spawned: Some((SIZE - 1, SIZE - 1)),
        }
    }

    fn spawn_tile(&mut self) {
        let mut empty = Vec::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                if self.board[y][x] == 0 {
                    empty.push((x, y));
                }
            }
        }
        if let Some(&(x, y)) = empty.get(rand::random_range(0..empty.len())) {
            self.board[y][x] = if rand::random_range(0..10) < 9 { 2 } else { 4 };
            self.last_spawned = Some((x, y));
        }
    }

    /// Map (line_index, position_along_line) to board (x, y) for a given
    /// slide direction. position 0 is the destination edge (where tiles
    /// slide toward).
    fn cell_pos(line: usize, pos: usize, dx: i32, dy: i32) -> (usize, usize) {
        match (dx, dy) {
            (-1, 0) => (pos, line),
            (1, 0) => (SIZE - 1 - pos, line),
            (0, -1) => (line, pos),
            (0, 1) => (line, SIZE - 1 - pos),
            _ => unreachable!(),
        }
    }

    fn slide(&mut self, dx: i32, dy: i32) -> bool {
        let mut moved = false;

        for i in 0..SIZE {
            // Extract non-zero values along the slide direction.
            let mut vals: Vec<u32> = Vec::new();
            for j in 0..SIZE {
                let (x, y) = Self::cell_pos(i, j, dx, dy);
                if self.board[y][x] != 0 {
                    vals.push(self.board[y][x]);
                }
            }

            // Merge adjacent equal pairs (first match wins, no double-merge).
            let mut merged: Vec<u32> = Vec::new();
            let mut k = 0;
            while k < vals.len() {
                if k + 1 < vals.len() && vals[k] == vals[k + 1] {
                    let v = vals[k] * 2;
                    merged.push(v);
                    self.score += v;
                    k += 2;
                } else {
                    merged.push(vals[k]);
                    k += 1;
                }
            }

            // Write back, padding with zeros.
            for j in 0..SIZE {
                let (x, y) = Self::cell_pos(i, j, dx, dy);
                let new_val = merged.get(j).copied().unwrap_or(0);
                if self.board[y][x] != new_val {
                    moved = true;
                }
                self.board[y][x] = new_val;
            }
        }

        moved
    }

    fn has_moves(&self) -> bool {
        for y in 0..SIZE {
            for x in 0..SIZE {
                if self.board[y][x] == 0 {
                    return true;
                }
                if x + 1 < SIZE && self.board[y][x] == self.board[y][x + 1] {
                    return true;
                }
                if y + 1 < SIZE && self.board[y][x] == self.board[y + 1][x] {
                    return true;
                }
            }
        }
        false
    }

    fn grid_line(left: &str, mid: &str, right: &str) -> Line {
        let bar = "═".repeat(CELL_WIDTH);
        let mut s = left.to_owned();
        for i in 0..SIZE {
            s += &bar;
            s += if i < SIZE - 1 { mid } else { right };
        }
        vec![s].try_into().unwrap()
    }

    fn render_tile_row(&self, y: usize) -> [Line; 3] {
        let tile_top_str = format!("╭{}╮", "─".repeat(TILE_INNER));
        let tile_bot_str = format!("╰{}╯", "─".repeat(TILE_INNER));
        let empty = " ".repeat(CELL_WIDTH);
        let pad = " ".repeat(PAD);

        let mut top = Line::default();
        let mut mid = Line::default();
        let mut bot = Line::default();

        top.push(Span::new_unstyled_lossy("║"));
        mid.push(Span::new_unstyled_lossy("║"));
        bot.push(Span::new_unstyled_lossy("║"));

        for x in 0..SIZE {
            let val = self.board[y][x];
            if val == 0 {
                top.push(Span::new_unstyled_lossy(&empty));
                mid.push(Span::new_unstyled_lossy(&empty));
                bot.push(Span::new_unstyled_lossy(&empty));
            } else {
                let is_new = self.last_spawned == Some((x, y));
                let color = tile_color(val);
                top.push(Span::new_unstyled_lossy(&pad));
                mid.push(Span::new_unstyled_lossy(&pad));
                bot.push(Span::new_unstyled_lossy(&pad));
                if is_new {
                    top.push(Span::new_styled_lossy(
                        tile_top_str.clone().with(color).bold(),
                    ));
                    let content = format!("│{:^width$}│", val, width = TILE_INNER);
                    mid.push(Span::new_styled_lossy(content.with(color).bold()));
                    bot.push(Span::new_styled_lossy(
                        tile_bot_str.clone().with(color).bold(),
                    ));
                } else {
                    top.push(Span::new_colored_lossy(&tile_top_str, color));
                    let content = format!("│{:^width$}│", val, width = TILE_INNER);
                    mid.push(Span::new_colored_lossy(&content, color));
                    bot.push(Span::new_colored_lossy(&tile_bot_str, color));
                }
                top.push(Span::new_unstyled_lossy(&pad));
                mid.push(Span::new_unstyled_lossy(&pad));
                bot.push(Span::new_unstyled_lossy(&pad));
            }

            top.push(Span::new_unstyled_lossy("║"));
            mid.push(Span::new_unstyled_lossy("║"));
            bot.push(Span::new_unstyled_lossy("║"));
        }

        [top, mid, bot]
    }
}

impl super::Game for Game {
    fn tick(&mut self, _tick_count: u32) -> super::TickResult {
        super::TickResult {
            alive: !self.game_over,
            scores: vec![super::Score {
                category: "2048".to_owned(),
                value: self.score as u64,
                lower_is_better: false,
            }],
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        vec![super::Score {
            category: "2048".to_owned(),
            value: self.score as u64,
            lower_is_better: false,
        }]
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        let moved = match input {
            Control::Up | Control::Char('w') => self.slide(0, -1),
            Control::Down | Control::Char('s') => self.slide(0, 1),
            Control::Left | Control::Char('a') => self.slide(-1, 0),
            Control::Right | Control::Char('d') => self.slide(1, 0),
            other => return Some(other),
        };
        if moved {
            self.spawn_tile();
            if !self.has_moves() {
                self.game_over = true;
            }
        }
        None
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        if let Ok(state) = serde_json::from_str::<Game>(json) {
            *self = state;
            true
        } else {
            false
        }
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                let mut lines: Vec<Line> = Vec::new();
                lines.push(vec![format!("Score: {}", self.score)].try_into().unwrap());
                lines.push(Self::grid_line("╔", "╦", "╗"));

                for y in 0..SIZE {
                    let [top, mid, bot] = self.render_tile_row(y);
                    lines.push(top);
                    lines.push(mid);
                    lines.push(bot);
                    if y < SIZE - 1 {
                        lines.push(Self::grid_line("╠", "╬", "╣"));
                    }
                }

                lines.push(Self::grid_line("╚", "╩", "╝"));

                Lines(lines)
            }
        })
    }
}
