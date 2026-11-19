/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Classic snake game with several quality-of-life improvements:
//!
//! - **Distance-biased apple placement**: apples spawn near the snake's head
//!   using weighted random selection (inverse distance^2.7), with a snake-length
//!   offset so longer snakes get slightly more spread. This keeps early gameplay
//!   engaging and avoids long treks across the board.
//!
//! - **"Last chance" forgiveness**: when the snake would collide, it gets one
//!   extra tick (at slightly faster rate) to change direction, preventing deaths
//!   from input arriving one frame late.
//!
//! - **Input buffering**: direction inputs queue in a VecDeque so rapid
//!   cornering sequences (e.g. up-then-right in quick succession) aren't lost.
//!
//! - **Opposite-direction rejection**: pressing the opposite direction is
//!   silently ignored rather than causing instant self-collision.
//!
//! - **Tail-chase**: moving into the tail cell is legal since it will vacate
//!   before the head arrives.

use std::collections::VecDeque;

use serde::Deserialize;
use serde::Serialize;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::components::Component;
use superconsole::components::DrawMode;

use crate::console::Control;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    fn add_x(mut self, i: i32) -> Self {
        self.x += i;
        self
    }

    fn add_y(mut self, i: i32) -> Self {
        self.y += i;
        self
    }

    fn distance(self, position: Position) -> f64 {
        ((position.x - self.x) as f64).hypot((position.y - self.y) as f64)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn turn(self, turn: Direction) -> Self {
        if self.is_vertical() == turn.is_vertical() {
            self
        } else {
            turn
        }
    }

    pub fn is_vertical(&self) -> bool {
        match self {
            Direction::Down | Direction::Up => true,
            _ => false,
        }
    }

    pub fn apply(&self, pos: Position) -> Position {
        match self {
            Direction::Up => pos.add_y(1),
            Direction::Down => pos.add_y(-1),
            Direction::Left => pos.add_x(-1),
            Direction::Right => pos.add_x(1),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Cell {
    Empty,
    Apple,
    Buck,
}

#[derive(Debug)]
pub struct Game {
    board: [[Cell; 80]; 40],
    score: u64,
    segments: VecDeque<Position>,
    apple_position: Position,
    direction: Direction,
    input_buffer: VecDeque<Direction>,
    last_chance: bool,
    next_update: u32,
}

impl Game {
    pub fn new() -> Self {
        let mut this = Self {
            board: [[Cell::Empty; 80]; 40],
            score: 0,
            segments: VecDeque::new(),
            apple_position: Position { x: 0, y: 0 },
            direction: Direction::Right,
            input_buffer: VecDeque::new(),
            last_chance: false,
            next_update: 10,
        };

        this.set(Position { x: 40, y: 20 }, Cell::Buck);
        this.segments.push_back(Position { x: 40, y: 20 });

        this.place_apple();
        this
    }

    fn set(&mut self, position: Position, cell: Cell) {
        self.board[position.y as usize][position.x as usize] = cell;
    }

    fn head(&self) -> Position {
        *self.segments.back().unwrap()
    }

    fn place_apple(&mut self) {
        let mut sum = 0.0;
        let mut options: Vec<(Position, f64)> = Vec::new();
        for (position, cell) in self.iter_cells() {
            // We're using a pretty large board and so the number of cells at a particular distance is
            // approximately proportional to distance^2, if we just used 1/distance^2 to scale the
            // chance, we'd basically be picking a (uniformly) random distance away for the cell. That's
            // better than a random cell (which has bias for picking things further away), but we'd actually
            // like to pick something close to the cell we're currently on (particularly when the snake is
            // still small) and so we bias it a bit more.
            if let Cell::Empty = cell {
                let distance = self.head().distance(position);
                let cost = (distance + (self.segments.len() as f64) / 10.0).powf(2.7);
                let chance = 1.0 / cost;
                sum += chance;
                options.push((position, sum));
            }
        }
        let roll = rand::random_range(0.0..sum);
        let idx = options
            .binary_search_by(|(_, chance)| chance.partial_cmp(&roll).unwrap())
            .unwrap_or_else(|i| i);
        let (position, _) = options[idx];
        self.apple_position = position;
        self.set(position, Cell::Apple);
    }

    fn iter_cells(&self) -> impl Iterator<Item = (Position, Cell)> + '_ {
        self.board.iter().enumerate().flat_map(|(y, v)| {
            v.iter().enumerate().map(move |(x, cell)| {
                (
                    Position {
                        x: x as i32,
                        y: y as i32,
                    },
                    *cell,
                )
            })
        })
    }

    fn get(&self, position: Position) -> Option<Cell> {
        self.board
            .get(position.y as usize)
            .and_then(|row| row.get(position.x as usize).copied())
    }

    fn is_bad(&self, position: Position) -> bool {
        match self.get(position) {
            Some(Cell::Empty) | Some(Cell::Apple) => false,
            Some(Cell::Buck) if *self.segments.front().unwrap() == position => false,
            _ => true,
        }
    }

    pub fn move_buck(&mut self, direction: Direction) -> bool {
        let next_position = direction.apply(self.head());
        if self.is_bad(next_position) {
            false
        } else {
            if self.apple_position == next_position {
                self.score += 1;
                self.place_apple();
            } else {
                self.pop_segment();
            }
            self.push_segment(next_position);
            true
        }
    }

    fn push_segment(&mut self, position: Position) {
        self.segments.push_back(position);
        self.set(position, Cell::Buck);
    }

    fn pop_segment(&mut self) {
        let position = self.segments.pop_front().unwrap();
        self.set(position, Cell::Empty);
    }
}

#[derive(Serialize, Deserialize)]
struct GameState {
    score: u64,
    segments: VecDeque<Position>,
    apple_position: Position,
    direction: Direction,
    input_buffer: VecDeque<Direction>,
    last_chance: bool,
    next_update: u32,
}

impl super::Game for Game {
    fn tick(&mut self, tick_count: u32) -> super::TickResult {
        if tick_count == self.next_update {
            if let Some(next_direction) = self.input_buffer.pop_front() {
                self.direction = self.direction.turn(next_direction);
            }
            if self.move_buck(self.direction) {
                self.last_chance = false;
                self.next_update += 4;
            } else if !self.last_chance {
                self.last_chance = true;
                self.next_update += 3;
            } else {
                return super::TickResult {
                    alive: false,
                    scores: vec![],
                };
            }
        }
        super::TickResult {
            alive: true,
            scores: vec![super::Score {
                category: "snake".to_owned(),
                value: self.score,
                lower_is_better: false,
            }],
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        vec![super::Score {
            category: "snake".to_owned(),
            value: self.score,
            lower_is_better: false,
        }]
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        let direction = match input {
            Control::Left | Control::Char('a') => Some(Direction::Left),
            Control::Right | Control::Char('d') => Some(Direction::Right),
            Control::Down | Control::Char('s') => Some(Direction::Down),
            Control::Up | Control::Char('w') => Some(Direction::Up),
            other => return Some(other),
        };
        if let Some(d) = direction {
            self.input_buffer.push_back(d);
        }
        None
    }

    fn save_state(&self) -> Option<String> {
        let state = GameState {
            score: self.score,
            segments: self.segments.clone(),
            apple_position: self.apple_position,
            direction: self.direction,
            input_buffer: self.input_buffer.clone(),
            last_chance: self.last_chance,
            next_update: self.next_update,
        };
        serde_json::to_string(&state).ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        if let Ok(state) = serde_json::from_str::<GameState>(json) {
            // Reset the board
            self.board = [[Cell::Empty; 80]; 40];
            self.score = state.score;
            self.segments = state.segments;
            self.apple_position = state.apple_position;
            self.direction = state.direction;
            self.input_buffer = state.input_buffer;
            self.last_chance = state.last_chance;
            self.next_update = state.next_update;
            // Reconstruct board from segments
            for i in 0..self.segments.len() {
                let pos = self.segments[i];
                self.set(pos, Cell::Buck);
            }
            self.set(self.apple_position, Cell::Apple);
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
                let mut lines: Vec<Line> = Vec::with_capacity(self.board.len());
                lines.push(vec![format!("Score: {}", self.score)].try_into().unwrap());
                lines.push(
                    vec!["╔".to_owned(), "═".repeat(160), "╗".to_owned()]
                        .try_into()
                        .unwrap(),
                );

                for row in self.board.iter().rev() {
                    let mut line = " ".repeat(160);
                    for (i, cell) in row.iter().enumerate().rev() {
                        if let Some(replace) = match cell {
                            Cell::Empty => None,
                            Cell::Apple => Some("🍎"),
                            Cell::Buck => Some("🦌"),
                        } {
                            line.replace_range((2 * i)..(2 * i + 2), replace);
                        }
                    }
                    lines.push(
                        vec!["║".to_owned(), line.to_owned(), "║".to_owned()]
                            .try_into()
                            .unwrap(),
                    );
                }
                lines.push(
                    vec!["╚".to_owned(), "═".repeat(160), "╝".to_owned()]
                        .try_into()
                        .unwrap(),
                );
                Lines(lines)
            }
        })
    }
}
