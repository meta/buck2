/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Breakout with multi-hit bricks, powerups, combos, and multi-level progression.
//!
//! Beyond a basic breakout:
//!
//! - **Multi-hit bricks**: bricks have 1–3 health (increasing with level),
//!   with color fading from bright → row color → dark as health decreases.
//!
//! - **4 powerup types** hidden in ~30% of bricks, visually distinguished
//!   by bold double-line borders (`═══`): WidePaddle (stacks up to 3×),
//!   MultiBall (mirrors existing ball), FireBall (pierces without bouncing),
//!   and SplitBall (splits into 3 on next collision).
//!
//! - **Combo scoring**: consecutive brick breaks without a paddle hit multiply
//!   the point value. Combo counter displayed in the status bar.
//!
//! - **Border flash feedback**: breaking a brick flashes the entire border in
//!   the brick's row color for 16 ticks, giving satisfying visual feedback.
//!
//! - **Paddle hit angle control**: ball exit angle depends on where it hits
//!   the paddle (center = straight, edges = angled), giving the player
//!   directional control.
//!
//! - **Fine movement**: Shift+Arrow (or A/D) moves the paddle 1 cell instead
//!   of 4, for precision positioning.
//!
//! - **Respawn delay with flashing lives**: after losing a ball, lives flash
//!   red for 16 ticks before respawning, so the player can prepare.
//!
//! - **Level progression**: clearing all bricks advances to the next level
//!   with more multi-hit bricks. Level 4+ has 3-hit top rows.

use rand::RngExt;
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

const BOARD_WIDTH: usize = 75;
const BOARD_HEIGHT: usize = 40;
const PADDLE_WIDTH: usize = 8;
const BRICK_ROWS: usize = 5;
const BRICK_COLS: usize = 15;
/// Each brick occupies 2 screen rows (top border + bottom border).
const BRICK_HEIGHT: usize = 2;
/// First screen row where bricks appear.
const BRICK_Y_OFFSET: i32 = 2;

fn brick_color(row: usize) -> Color {
    match row {
        0 => Color::Red,
        1 => Color::DarkYellow,
        2 => Color::Yellow,
        3 => Color::Green,
        _ => Color::Cyan,
    }
}

fn brick_color_dark(row: usize) -> Color {
    match row {
        0 => Color::DarkRed,
        1 => Color::DarkYellow,
        2 => Color::DarkYellow,
        3 => Color::DarkGreen,
        _ => Color::DarkCyan,
    }
}

/// Color varies by health: 3 = bright white, 2 = row color, 1 = dark shade.
fn health_color(row: usize, health: u8) -> Color {
    match health {
        3 => Color::White,
        2 => brick_color(row),
        _ => brick_color_dark(row),
    }
}

fn brick_points(row: usize) -> u32 {
    match row {
        0 => 30,
        1 => 25,
        2 => 20,
        3 => 15,
        _ => 10,
    }
}

fn build_bricks(level: u32) -> [[u8; BRICK_COLS]; BRICK_ROWS] {
    let mut bricks = [[1u8; BRICK_COLS]; BRICK_ROWS];
    match level {
        1 => {}
        2 => {
            bricks[0].fill(2);
            bricks[1].fill(2);
        }
        3 => {
            bricks[0].fill(3);
            bricks[1].fill(2);
            bricks[2].fill(2);
        }
        _ => {
            bricks[0].fill(3);
            bricks[1].fill(3);
            bricks[2].fill(2);
            bricks[3].fill(2);
        }
    }
    bricks
}

/// Assign powerups to ~15% of bricks at level start.
fn build_powerup_bricks() -> [[Option<PowerupKind>; BRICK_COLS]; BRICK_ROWS] {
    let mut rng = rand::rng();
    let mut grid = [[None; BRICK_COLS]; BRICK_ROWS];
    for row in grid.iter_mut() {
        for cell in row.iter_mut() {
            if rng.random_range(0..100u32) < 30 {
                // Weighted: MultiBall 40, WidePaddle 25, FireBall 20, SplitBall 15
                *cell = Some(match rng.random_range(0..100u32) {
                    0..40 => PowerupKind::MultiBall,
                    40..65 => PowerupKind::WidePaddle,
                    65..85 => PowerupKind::FireBall,
                    _ => PowerupKind::SplitBall,
                });
            }
        }
    }
    grid
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ball {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    #[serde(default)]
    split: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum PowerupKind {
    WidePaddle,
    MultiBall,
    FireBall,
    SplitBall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Powerup {
    x: f32,
    y: f32,
    kind: PowerupKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    paddle_x: f32,
    balls: Vec<Ball>,
    bricks: [[u8; BRICK_COLS]; BRICK_ROWS],
    score: u32,
    lives: u32,
    #[serde(default = "default_level")]
    level: u32,
    #[serde(default)]
    combo: u32,
    game_over: bool,
    next_update: u32,
    #[serde(default)]
    powerups: Vec<Powerup>,
    #[serde(default)]
    powerup_bricks: [[Option<PowerupKind>; BRICK_COLS]; BRICK_ROWS],
    #[serde(default)]
    wide_paddle_ticks: u32,
    #[serde(default)]
    wide_paddle_level: u32,
    #[serde(default)]
    fireball_ticks: u32,
    #[serde(default)]
    flash_ticks: u32,
    #[serde(default)]
    flash_color: (u8, u8, u8),
    #[serde(default)]
    respawn_ticks: u32,
}

fn default_level() -> u32 {
    1
}

impl Game {
    pub fn new() -> Self {
        Self {
            paddle_x: (BOARD_WIDTH / 2 - PADDLE_WIDTH / 2) as f32,
            balls: vec![Ball {
                x: BOARD_WIDTH as f32 / 2.0,
                y: BOARD_HEIGHT as f32 - 3.0,
                dx: 1.0,
                dy: -1.0,
                split: false,
            }],
            bricks: build_bricks(1),
            score: 0,
            lives: 3,
            level: 1,
            combo: 0,
            game_over: false,
            next_update: 5,
            powerups: Vec::new(),
            powerup_bricks: build_powerup_bricks(),
            wide_paddle_ticks: 0,
            wide_paddle_level: 0,
            fireball_ticks: 0,
            flash_ticks: 0,
            flash_color: (255, 255, 255),
            respawn_ticks: 0,
        }
    }

    fn brick_width(&self) -> usize {
        BOARD_WIDTH / BRICK_COLS
    }

    fn effective_paddle_width(&self) -> usize {
        // WidePaddle stacks up to 3 times: +4, +3, +2
        let extra = match self.wide_paddle_level {
            0 => 0,
            1 => 4,
            2 => 7,
            _ => 9,
        };
        PADDLE_WIDTH + extra
    }

    fn flash_color_value(&self) -> Color {
        Color::Rgb {
            r: self.flash_color.0,
            g: self.flash_color.1,
            b: self.flash_color.2,
        }
    }

    fn set_flash(&mut self, row: usize) {
        self.flash_ticks = 16;
        let (r, g, b) = match row {
            0 => (255, 80, 80),
            1 => (255, 180, 0),
            2 => (255, 255, 80),
            3 => (80, 255, 80),
            _ => (80, 255, 255),
        };
        self.flash_color = (r, g, b);
    }

    fn update(&mut self) {
        if self.flash_ticks > 0 {
            self.flash_ticks -= 1;
        }
        if self.wide_paddle_ticks > 0 {
            self.wide_paddle_ticks -= 1;
            if self.wide_paddle_ticks == 0 {
                self.wide_paddle_level = 0;
            }
        }
        if self.fireball_ticks > 0 {
            self.fireball_ticks -= 1;
        }

        let paddle_width = self.effective_paddle_width();
        let bw = self.brick_width();

        // Update powerups (fall downward)
        let paddle_top = BOARD_HEIGHT as f32 - 1.0;
        let mut caught_powerups = Vec::new();
        self.powerups.retain_mut(|p| {
            p.y += 0.5;
            if p.y >= paddle_top
                && p.x >= self.paddle_x
                && p.x < self.paddle_x + paddle_width as f32
            {
                caught_powerups.push(p.kind);
                return false;
            }
            p.y < BOARD_HEIGHT as f32
        });

        for kind in caught_powerups {
            match kind {
                PowerupKind::WidePaddle => {
                    self.wide_paddle_level = (self.wide_paddle_level + 1).min(3);
                    self.wide_paddle_ticks = 500;
                }
                PowerupKind::MultiBall => {
                    if let Some(ball) = self.balls.first().cloned() {
                        self.balls.push(Ball {
                            dx: -ball.dx,
                            ..ball
                        });
                    }
                }
                PowerupKind::FireBall => {
                    // 3 full vertical traversals: 3 * BOARD_HEIGHT
                    self.fireball_ticks = (BOARD_HEIGHT as u32) * 3;
                }
                PowerupKind::SplitBall => {
                    for ball in &mut self.balls {
                        ball.split = true;
                    }
                }
            }
        }

        // Update each ball
        let mut new_balls: Vec<Ball> = Vec::new();
        for i in 0..self.balls.len() {
            self.balls[i].x += self.balls[i].dx;
            self.balls[i].y += self.balls[i].dy;

            let mut did_collide = false;

            // Wall bounces
            if self.balls[i].x <= 0.0 || self.balls[i].x >= BOARD_WIDTH as f32 - 1.0 {
                self.balls[i].dx = -self.balls[i].dx;
                self.balls[i].x = self.balls[i].x.clamp(0.0, BOARD_WIDTH as f32 - 1.0);
                did_collide = true;
            }
            if self.balls[i].y <= 0.0 {
                self.balls[i].dy = 1.0;
                self.balls[i].y = 0.0;
                did_collide = true;
            }

            // Paddle bounce — detect when ball crosses into the paddle row.
            let prev_y = self.balls[i].y - self.balls[i].dy;
            if self.balls[i].dy > 0.0
                && prev_y < paddle_top
                && self.balls[i].y >= paddle_top
                && self.balls[i].x >= self.paddle_x
                && self.balls[i].x < self.paddle_x + paddle_width as f32
            {
                self.balls[i].dy = -1.0;
                self.balls[i].y = paddle_top - 0.1;
                let offset = (self.balls[i].x - self.paddle_x) / paddle_width as f32 - 0.5;
                self.balls[i].dx = (offset * 3.0).clamp(-1.5, 1.5);
                self.combo = 0;
                did_collide = true;
            }

            // Brick collision — each brick is BRICK_HEIGHT screen rows tall
            let bx = self.balls[i].x.round() as usize / bw;
            let by_screen = self.balls[i].y.round() as i32 - BRICK_Y_OFFSET;
            let by = by_screen as usize / BRICK_HEIGHT;
            if by_screen >= 0
                && (by_screen as usize) < BRICK_ROWS * BRICK_HEIGHT
                && bx < BRICK_COLS
                && self.bricks[by][bx] > 0
            {
                let fireball = self.fireball_ticks > 0;
                if fireball {
                    self.bricks[by][bx] = 0;
                } else {
                    self.bricks[by][bx] -= 1;
                    self.balls[i].dy = -self.balls[i].dy.signum();
                }
                if self.bricks[by][bx] == 0 {
                    self.combo += 1;
                    self.score += brick_points(by) * self.combo;
                    self.set_flash(by);

                    if let Some(kind) = self.powerup_bricks[by][bx].take() {
                        self.powerups.push(Powerup {
                            x: (bx * bw + bw / 2) as f32,
                            y: (BRICK_Y_OFFSET + by as i32 * BRICK_HEIGHT as i32) as f32,
                            kind,
                        });
                    }
                }
                did_collide = true;
            }

            // SplitBall: on first collision, split into 3
            if did_collide && self.balls[i].split {
                self.balls[i].split = false;
                let b = &self.balls[i];
                new_balls.push(Ball {
                    x: b.x,
                    y: b.y,
                    dx: (b.dx - 0.7).clamp(-1.5, 1.5),
                    dy: b.dy,
                    split: false,
                });
                new_balls.push(Ball {
                    x: b.x,
                    y: b.y,
                    dx: (b.dx + 0.7).clamp(-1.5, 1.5),
                    dy: b.dy,
                    split: false,
                });
            }
        }
        self.balls.append(&mut new_balls);

        // Remove lost balls
        self.balls.retain(|b| b.y < BOARD_HEIGHT as f32);

        // All balls lost -> lose a life, start respawn delay
        if self.balls.is_empty() && self.respawn_ticks == 0 {
            self.lives -= 1;
            if self.lives == 0 {
                self.game_over = true;
            } else {
                self.respawn_ticks = 16;
                self.combo = 0;
            }
        }

        // Respawn countdown
        if self.respawn_ticks > 0 {
            self.respawn_ticks -= 1;
            if self.respawn_ticks == 0 {
                self.balls.push(Ball {
                    x: BOARD_WIDTH as f32 / 2.0,
                    y: BOARD_HEIGHT as f32 - 3.0,
                    dx: 1.0,
                    dy: -1.0,
                    split: false,
                });
            }
        }

        // Check level clear
        if self.bricks.iter().all(|row| row.iter().all(|&b| b == 0)) {
            self.level += 1;
            self.bricks = build_bricks(self.level);
            self.powerup_bricks = build_powerup_bricks();
            self.balls = vec![Ball {
                x: BOARD_WIDTH as f32 / 2.0,
                y: BOARD_HEIGHT as f32 - 3.0,
                dx: 1.0,
                dy: -1.0,
                split: false,
            }];
            self.powerups.clear();
            self.wide_paddle_ticks = 0;
            self.wide_paddle_level = 0;
            self.fireball_ticks = 0;
            self.combo = 0;
        }
    }
}

impl super::Game for Game {
    fn tick(&mut self, tick_count: u32) -> super::TickResult {
        if self.game_over {
            return super::TickResult {
                alive: false,
                scores: vec![],
            };
        }
        if tick_count >= self.next_update {
            self.update();
            let interval = 2;
            self.next_update = tick_count + interval;
        }
        super::TickResult {
            alive: true,
            scores: vec![super::Score {
                category: "breakout".to_owned(),
                value: self.score as u64,
                lower_is_better: false,
            }],
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        vec![super::Score {
            category: "breakout".to_owned(),
            value: self.score as u64,
            lower_is_better: false,
        }]
    }

    fn game_over_message(&self) -> Option<&str> {
        Some("Game Over!")
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        let max_x = (BOARD_WIDTH - self.effective_paddle_width()) as f32;
        match input {
            Control::Left | Control::Char('a') => {
                self.paddle_x = (self.paddle_x - 4.0).max(0.0);
            }
            Control::Right | Control::Char('d') => {
                self.paddle_x = (self.paddle_x + 4.0).min(max_x);
            }
            Control::ShiftLeft | Control::Char('A') => {
                self.paddle_x = (self.paddle_x - 1.0).max(0.0);
            }
            Control::ShiftRight | Control::Char('D') => {
                self.paddle_x = (self.paddle_x + 1.0).min(max_x);
            }
            other => return Some(other),
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

/// Push a border span, colored if flash is active.
fn push_border(line: &mut Line, ch: &str, flash_active: bool, flash_c: Color) {
    if flash_active {
        line.push(Span::new_colored_lossy(ch, flash_c));
    } else {
        line.push(Span::new_unstyled_lossy(ch));
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                let mut lines: Vec<Line> = Vec::new();

                // Status line — flash lives red during respawn
                let respawn_flash =
                    self.respawn_ticks > 0 && ((self.respawn_ticks - 1) / 2).is_multiple_of(2);
                let combo_text = if self.combo > 1 {
                    format!("  Combo: x{}", self.combo)
                } else {
                    String::new()
                };
                {
                    let mut status = Line::default();
                    status.push(Span::new_unstyled_lossy(format!(
                        "Level: {}  Score: {}  ",
                        self.level, self.score
                    )));
                    let lives_text = format!("Lives: {}", self.lives);
                    if respawn_flash {
                        status.push(Span::new_colored_lossy(&lives_text, Color::Red));
                    } else {
                        status.push(Span::new_unstyled_lossy(&lives_text));
                    }
                    if !combo_text.is_empty() {
                        status.push(Span::new_unstyled_lossy(&combo_text));
                    }
                    lines.push(status);
                }

                // Top border — flash active for 4 ticks at a time over 16 ticks
                let flash_active = self.flash_ticks > 0 && ((self.flash_ticks - 1) / 4) % 2 == 1;
                let flash_c = self.flash_color_value();
                {
                    let mut border = Line::default();
                    push_border(&mut border, "╔", flash_active, flash_c);
                    for _ in 0..BOARD_WIDTH {
                        push_border(&mut border, "═", flash_active, flash_c);
                    }
                    push_border(&mut border, "╗", flash_active, flash_c);
                    lines.push(border);
                }

                let bw = self.brick_width();
                let paddle_x = self.paddle_x as usize;
                let paddle_width = self.effective_paddle_width();

                // Collect ball positions for quick lookup
                let ball_positions: Vec<(usize, usize)> = self
                    .balls
                    .iter()
                    .map(|b| (b.x.round() as usize, b.y.round() as usize))
                    .collect();

                for y in 0..BOARD_HEIGHT {
                    let mut line = Line::default();

                    // Left border
                    push_border(&mut line, "║", flash_active, flash_c);

                    let brick_screen_y = y as i32 - BRICK_Y_OFFSET;
                    let is_brick_row = brick_screen_y >= 0
                        && (brick_screen_y as usize) < BRICK_ROWS * BRICK_HEIGHT;

                    // Collect overlays (balls + powerups) for this row, sorted by x
                    struct Overlay {
                        x: usize,
                        ch: &'static str,
                        color: Color,
                    }
                    let mut overlays: Vec<Overlay> = Vec::new();

                    for &(bx, by) in &ball_positions {
                        if by == y && bx < BOARD_WIDTH {
                            let ball_color = if self.fireball_ticks > 0 {
                                Color::Red
                            } else {
                                Color::White
                            };
                            overlays.push(Overlay {
                                x: bx,
                                ch: "●",
                                color: ball_color,
                            });
                        }
                    }

                    for p in &self.powerups {
                        let px = p.x.round() as usize;
                        let py = p.y.round() as usize;
                        if py == y && px < BOARD_WIDTH {
                            let (ch, color) = match p.kind {
                                PowerupKind::WidePaddle => ("W", Color::Green),
                                PowerupKind::MultiBall => ("M", Color::Cyan),
                                PowerupKind::FireBall => ("F", Color::Red),
                                PowerupKind::SplitBall => ("S", Color::Magenta),
                            };
                            overlays.push(Overlay { x: px, ch, color });
                        }
                    }

                    overlays.sort_by_key(|o| o.x);
                    let mut overlay_idx = 0;

                    // Helper: fill from `col` to `target`, rendering any overlays
                    // that fall in the gap. Returns the new col.
                    let mut col = 0usize;

                    let fill_gap = |line: &mut Line,
                                    col: &mut usize,
                                    target: usize,
                                    overlays: &[Overlay],
                                    overlay_idx: &mut usize| {
                        while *overlay_idx < overlays.len() && overlays[*overlay_idx].x < target {
                            let o = &overlays[*overlay_idx];
                            if o.x >= *col {
                                if o.x > *col {
                                    line.push(Span::padding(o.x - *col));
                                }
                                line.push(Span::new_colored_lossy(o.ch, o.color));
                                *col = o.x + 1;
                            }
                            *overlay_idx += 1;
                        }
                        if *col < target {
                            line.push(Span::padding(target - *col));
                            *col = target;
                        }
                    };

                    if is_brick_row {
                        let br = brick_screen_y as usize / BRICK_HEIGHT;
                        let is_top = (brick_screen_y as usize).is_multiple_of(BRICK_HEIGHT);
                        let (left_corner, right_corner) = if is_top {
                            ("╭", "╮")
                        } else {
                            ("╰", "╯")
                        };
                        for bx in 0..BRICK_COLS {
                            let start = bx * bw;
                            let health = self.bricks[br][bx];
                            if health > 0 {
                                fill_gap(&mut line, &mut col, start, &overlays, &mut overlay_idx);
                                let color = health_color(br, health);
                                if self.powerup_bricks[br][bx].is_some() {
                                    // Powerup bricks: bold + double-line fill
                                    line.push(Span::new_styled_lossy_str(
                                        left_corner.with(color).bold(),
                                    ));
                                    line.push(Span::new_styled_lossy_str("═══".with(color).bold()));
                                    line.push(Span::new_styled_lossy_str(
                                        right_corner.with(color).bold(),
                                    ));
                                } else {
                                    line.push(Span::new_colored_lossy(left_corner, color));
                                    line.push(Span::new_colored_lossy("───", color));
                                    line.push(Span::new_colored_lossy(right_corner, color));
                                }
                                col = start + bw;
                            }
                        }
                    }

                    // Draw paddle
                    if y == BOARD_HEIGHT - 1 {
                        fill_gap(&mut line, &mut col, paddle_x, &overlays, &mut overlay_idx);
                        let paddle_end = (paddle_x + paddle_width).min(BOARD_WIDTH);
                        let paddle_color = if flash_active { flash_c } else { Color::White };
                        for _ in col..paddle_end {
                            line.push(Span::new_colored_lossy("▀", paddle_color));
                        }
                        col = paddle_end;
                    }

                    // Fill remaining width, rendering any trailing overlays
                    fill_gap(
                        &mut line,
                        &mut col,
                        BOARD_WIDTH,
                        &overlays,
                        &mut overlay_idx,
                    );

                    // Right border
                    push_border(&mut line, "║", flash_active, flash_c);

                    lines.push(line);
                }

                // Bottom border
                {
                    let mut border = Line::default();
                    push_border(&mut border, "╚", flash_active, flash_c);
                    for _ in 0..BOARD_WIDTH {
                        push_border(&mut border, "═", flash_active, flash_c);
                    }
                    push_border(&mut border, "╝", flash_active, flash_c);
                    lines.push(border);
                }

                Lines(lines)
            }
        })
    }
}
