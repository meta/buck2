/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Conway's Game of Life with interactive editing and rich visualization.
//!
//! Beyond a basic Life implementation:
//!
//! - **8 color schemes** cycled with `m`: None (green/black), Age (cyan→blue
//!   fade), Density (3×3 blue→green→red), Density2 (5×5 neighborhood),
//!   Velocity (birth=teal, stable=green, death=maroon), Heat (activity
//!   accumulation with decay), Gradient (position-based HSV rainbow), and
//!   Energy (age+density through a heat gradient).
//!
//! - **Per-cell auxiliary state**: age (ticks alive), previous generation
//!   (for velocity detection), and heat (activity accumulation with gradual
//!   decay). These drive the color schemes without affecting the simulation.
//!
//! - **Interactive editing**: movable cursor to toggle cells, pause/step for
//!   debugging patterns, clear board, and re-randomize.
//!
//! - **Toroidal topology**: edges wrap in both dimensions via `rem_euclid`.
//!
//! - **Framebuffer rendering**: uses the shared `FrameBuffer` for per-pixel
//!   RGB color output via half-block characters.
//!
//! - **Sparse save format**: only live cell coordinates are serialized,
//!   keeping save files compact regardless of board size.

use rand::RngExt;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;

use crate::console::Control;
use crate::framebuffer::FrameBuffer;

const WIDTH: usize = 160;
const HEIGHT: usize = 80;

const CURSOR_COLOR: Color = Color::Yellow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorScheme {
    None,
    Age,
    Density,
    Density2,
    Velocity,
    Heat,
    Gradient,
    Energy,
}

impl ColorScheme {
    fn next(self) -> Self {
        match self {
            Self::None => Self::Age,
            Self::Age => Self::Density,
            Self::Density => Self::Density2,
            Self::Density2 => Self::Velocity,
            Self::Velocity => Self::Heat,
            Self::Heat => Self::Gradient,
            Self::Gradient => Self::Energy,
            Self::Energy => Self::None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Age => "Age",
            Self::Density => "Density",
            Self::Density2 => "Density2",
            Self::Velocity => "Velocity",
            Self::Heat => "Heat",
            Self::Gradient => "Gradient",
            Self::Energy => "Energy",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Age => 1,
            Self::Density => 2,
            Self::Density2 => 3,
            Self::Velocity => 4,
            Self::Heat => 5,
            Self::Gradient => 6,
            Self::Energy => 7,
        }
    }

    fn from_index(i: u8) -> Self {
        match i {
            1 => Self::Age,
            2 => Self::Density,
            3 => Self::Density2,
            4 => Self::Velocity,
            5 => Self::Heat,
            6 => Self::Gradient,
            7 => Self::Energy,
            _ => Self::None,
        }
    }
}

fn heat_gradient(t: f32) -> (u8, u8, u8) {
    // black -> blue -> cyan -> green -> yellow -> red
    let t = t.clamp(0.0, 1.0);
    let (r, g, b) = if t < 0.2 {
        let s = t / 0.2;
        (0.0, 0.0, s)
    } else if t < 0.4 {
        let s = (t - 0.2) / 0.2;
        (0.0, s, 1.0)
    } else if t < 0.6 {
        let s = (t - 0.4) / 0.2;
        (0.0, 1.0, 1.0 - s)
    } else if t < 0.8 {
        let s = (t - 0.6) / 0.2;
        (s, 1.0, 0.0)
    } else {
        let s = (t - 0.8) / 0.2;
        (1.0, 1.0 - s, 0.0)
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

#[derive(Debug, Clone)]
pub struct Game {
    cells: Vec<Vec<bool>>,
    cursor_x: usize,
    cursor_y: usize,
    paused: bool,
    generation: u64,
    next_update: u32,
    color_scheme: ColorScheme,
    age: Vec<Vec<u16>>,
    prev_cells: Vec<Vec<bool>>,
    heat: Vec<Vec<u16>>,
}

impl Game {
    pub fn new() -> Self {
        let mut cells = vec![vec![false; WIDTH]; HEIGHT];
        let mut rng = rand::rng();
        for row in cells.iter_mut() {
            for cell in row.iter_mut() {
                *cell = rng.random_range(0..5) == 0;
            }
        }
        Self {
            cells,
            cursor_x: WIDTH / 2,
            cursor_y: HEIGHT / 2,
            paused: false,
            generation: 0,
            next_update: 10,
            color_scheme: ColorScheme::None,
            age: vec![vec![0u16; WIDTH]; HEIGHT],
            prev_cells: vec![vec![false; WIDTH]; HEIGHT],
            heat: vec![vec![0u16; WIDTH]; HEIGHT],
        }
    }

    fn reset_aux(&mut self) {
        self.age = vec![vec![0u16; WIDTH]; HEIGHT];
        self.prev_cells = vec![vec![false; WIDTH]; HEIGHT];
        self.heat = vec![vec![0u16; WIDTH]; HEIGHT];
    }

    fn neighbors(&self, x: usize, y: usize) -> u8 {
        let mut count = 0;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = (x as i32 + dx).rem_euclid(WIDTH as i32) as usize;
                let ny = (y as i32 + dy).rem_euclid(HEIGHT as i32) as usize;
                if self.cells[ny][nx] {
                    count += 1;
                }
            }
        }
        count
    }

    fn neighbors_5x5(&self, x: usize, y: usize) -> u8 {
        let mut count = 0u8;
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = (x as i32 + dx).rem_euclid(WIDTH as i32) as usize;
                let ny = (y as i32 + dy).rem_euclid(HEIGHT as i32) as usize;
                if self.cells[ny][nx] {
                    count += 1;
                }
            }
        }
        count
    }

    fn cell_color(&self, x: usize, y: usize) -> Color {
        let alive = self.cells[y][x];
        match self.color_scheme {
            ColorScheme::None => {
                if alive {
                    Color::Green
                } else {
                    Color::Black
                }
            }
            ColorScheme::Age => {
                if !alive {
                    return Color::Black;
                }
                let a = self.age[y][x].min(200) as f32 / 200.0;
                // New = bright cyan, old = dark blue
                let r = 0;
                let g = (255.0 * (1.0 - a)) as u8;
                let b = (100.0 + 155.0 * a) as u8;
                Color::Rgb { r, g, b }
            }
            ColorScheme::Density => {
                if !alive {
                    return Color::Black;
                }
                let n = self.neighbors(x, y);
                let t = n as f32 / 8.0;
                // Sparse=blue, healthy=green, crowded=red
                let (r, g, b) = if t < 0.5 {
                    let s = t / 0.5;
                    (0, (s * 255.0) as u8, (255.0 * (1.0 - s)) as u8)
                } else {
                    let s = (t - 0.5) / 0.5;
                    ((s * 255.0) as u8, (255.0 * (1.0 - s)) as u8, 0)
                };
                Color::Rgb { r, g, b }
            }
            ColorScheme::Density2 => {
                if !alive {
                    return Color::Black;
                }
                let n = self.neighbors_5x5(x, y);
                let t = n as f32 / 24.0;
                let (r, g, b) = if t < 0.5 {
                    let s = t / 0.5;
                    (0, (s * 255.0) as u8, (255.0 * (1.0 - s)) as u8)
                } else {
                    let s = (t - 0.5) / 0.5;
                    ((s * 255.0) as u8, (255.0 * (1.0 - s)) as u8, 0)
                };
                Color::Rgb { r, g, b }
            }
            ColorScheme::Velocity => {
                let was_alive = self.prev_cells[y][x];
                match (was_alive, alive) {
                    (false, true) => {
                        // Just born — soft teal
                        Color::Rgb {
                            r: 0,
                            g: 200,
                            b: 180,
                        }
                    }
                    (true, true) => {
                        // Stable alive — fade from teal to dim green with age
                        let a = self.age[y][x].min(50) as f32 / 50.0;
                        let g = (200.0 - a * 120.0) as u8;
                        let b = (180.0 * (1.0 - a)) as u8;
                        Color::Rgb { r: 0, g, b }
                    }
                    (true, false) => {
                        // Just died — muted maroon
                        Color::Rgb {
                            r: 100,
                            g: 20,
                            b: 30,
                        }
                    }
                    (false, false) => Color::Black,
                }
            }
            ColorScheme::Heat => {
                let h = self.heat[y][x];
                if h == 0 {
                    return Color::Black;
                }
                // black → yellow → orange → red, wider range
                let t = (h as f32 / 400.0).min(1.0);
                let (r, g, b) = if t < 0.4 {
                    let s = t / 0.4;
                    ((s * 255.0) as u8, (s * 180.0) as u8, 0)
                } else if t < 0.7 {
                    let s = (t - 0.4) / 0.3;
                    (255, (180.0 - s * 40.0) as u8, 0)
                } else {
                    let s = (t - 0.7) / 0.3;
                    (255, (140.0 * (1.0 - s)) as u8, 0)
                };
                Color::Rgb { r, g, b }
            }
            ColorScheme::Gradient => {
                if !alive {
                    return Color::Black;
                }
                let hue = ((x as f32 / WIDTH as f32) + (y as f32 / HEIGHT as f32)) * 180.0;
                let (r, g, b) = hsv_to_rgb(hue % 360.0, 1.0, 1.0);
                Color::Rgb { r, g, b }
            }
            ColorScheme::Energy => {
                if !alive {
                    return Color::Black;
                }
                let a = self.age[y][x].min(100) as f32 / 100.0;
                let n = self.neighbors(x, y) as f32 / 8.0;
                let energy = (a + n) / 2.0;
                let (r, g, b) = heat_gradient(energy);
                Color::Rgb { r, g, b }
            }
        }
    }

    fn step(&mut self) {
        let mut next = vec![vec![false; WIDTH]; HEIGHT];
        for (y, row) in next.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                let n = self.neighbors(x, y);
                *cell = if self.cells[y][x] {
                    n == 2 || n == 3
                } else {
                    n == 3
                };
            }
        }

        // Update auxiliary state before swapping cells
        for (y, row) in next.iter().enumerate() {
            for (x, &alive) in row.iter().enumerate() {
                // Age: increment for alive cells, reset for dead
                if alive {
                    self.age[y][x] = self.age[y][x].saturating_add(1);
                } else {
                    self.age[y][x] = 0;
                }

                // Heat: add activity, decay over time
                let changed = self.cells[y][x] != alive;
                if changed {
                    self.heat[y][x] = self.heat[y][x].saturating_add(20).min(500);
                } else {
                    self.heat[y][x] = self.heat[y][x].saturating_sub(2);
                }
            }
        }

        self.prev_cells = std::mem::replace(&mut self.cells, next);
        self.generation += 1;
    }
}

impl super::Game for Game {
    fn tick(&mut self, tick_count: u32) -> super::TickResult {
        if !self.paused && tick_count >= self.next_update {
            self.step();
            self.next_update = tick_count + 4;
        }
        super::TickResult {
            alive: true,
            scores: vec![],
        }
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        match input {
            // FrameBuffer renders row 0 at the bottom (y increases upward), so
            // moving the cursor up on screen means increasing cursor_y.
            Control::Up | Control::Char('w') => {
                self.cursor_y = (self.cursor_y + 1) % HEIGHT;
            }
            Control::Down | Control::Char('s') => {
                self.cursor_y = (self.cursor_y + HEIGHT - 1) % HEIGHT;
            }
            Control::Left | Control::Char('a') => {
                self.cursor_x = (self.cursor_x + WIDTH - 1) % WIDTH;
            }
            Control::Right | Control::Char('d') => {
                self.cursor_x = (self.cursor_x + 1) % WIDTH;
            }
            Control::Char(' ') => {
                self.cells[self.cursor_y][self.cursor_x] =
                    !self.cells[self.cursor_y][self.cursor_x];
            }
            Control::Char('p') => {
                self.paused = !self.paused;
            }
            Control::Char('c') => {
                self.cells = vec![vec![false; WIDTH]; HEIGHT];
                self.generation = 0;
                self.reset_aux();
            }
            Control::Char('r') => {
                let mut rng = rand::rng();
                for row in self.cells.iter_mut() {
                    for cell in row.iter_mut() {
                        *cell = rng.random_range(0..5) == 0;
                    }
                }
                self.generation = 0;
                self.reset_aux();
            }
            Control::Char('n') => {
                if self.paused {
                    self.step();
                }
            }
            Control::Char('m') => {
                self.color_scheme = self.color_scheme.next();
            }
            other => return Some(other),
        }
        None
    }

    fn save_state(&self) -> Option<String> {
        let live: Vec<(usize, usize)> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).filter_map(move |x| self.cells[y][x].then_some((x, y))))
            .collect();
        serde_json::to_string(&(
            &live,
            self.cursor_x,
            self.cursor_y,
            self.paused,
            self.generation,
            self.next_update,
            self.color_scheme.index(),
        ))
        .ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        type State = (Vec<(usize, usize)>, usize, usize, bool, u64, u32, u8);
        let Ok((live, cx, cy, p, generation, next, scheme)) = serde_json::from_str::<State>(json)
        else {
            return false;
        };
        self.cells = vec![vec![false; WIDTH]; HEIGHT];
        for (x, y) in live {
            if y < HEIGHT && x < WIDTH {
                self.cells[y][x] = true;
            }
        }
        self.cursor_x = cx;
        self.cursor_y = cy;
        self.paused = p;
        self.generation = generation;
        self.next_update = next;
        self.color_scheme = ColorScheme::from_index(scheme);
        self.reset_aux();
        true
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                let mut fb = FrameBuffer::new();
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        let is_cursor = x == self.cursor_x && y == self.cursor_y;
                        let color = if is_cursor {
                            CURSOR_COLOR
                        } else {
                            self.cell_color(x, y)
                        };
                        fb.display[y][x] = color;
                    }
                }

                let mut output = fb.draw_unchecked(dimensions, mode)?;

                // Prepend status line
                let status = if self.paused { "PAUSED" } else { "RUNNING" };
                let population: usize = self
                    .cells
                    .iter()
                    .flat_map(|r| r.iter())
                    .filter(|c| **c)
                    .count();
                let status_line: Line = vec![format!(
                    "Gen: {}  Pop: {}  Color: {}  [{}]",
                    self.generation,
                    population,
                    self.color_scheme.name(),
                    status
                )]
                .try_into()
                .unwrap();
                output.0.insert(0, status_line);

                // Append help line
                output.0.push(
                    vec![
                        "[space] toggle  [p] pause  [n] step  [r] random  [c] clear  [m] color"
                            .to_owned(),
                    ]
                    .try_into()
                    .unwrap(),
                );
                output
            }
        })
    }
}
