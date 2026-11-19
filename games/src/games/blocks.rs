/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Tetromino color-matching game.
//!
//! Pieces fall onto a board and lock in place. Runs of 4+ same-colored
//! cells in a row or column are cleared, and unsupported blocks above
//! fall as groups.
//!
//! Notable implementation details:
//!
//! - **Color-matching, not line-clearing**: each tetromino cell is randomly
//!   colored from a small palette. Matches are 4+ adjacent same-color cells
//!   in a row/column, scored as (n − 3)² to reward longer runs.
//!
//! - **Gravity for floating groups**: after matches clear, a disjoint-set
//!   (union-find) algorithm identifies connected components. Groups not
//!   connected to the floor fall as rigid units, then re-check for chain
//!   matches.
//!
//! - **Seven-bag randomizer**: pieces are drawn from shuffled bags of all 7
//!   tetromino types, guaranteeing even distribution.
//!
//! - **Lock delay with reset**: when a piece lands, the player gets ~0.5s
//!   to rotate/move before it locks. Moving resets the timer.
//!
//! - **Match animation**: cleared cells flash random colors for 30 ticks
//!   before being removed.

use std::collections::HashSet;
use std::fmt::Debug;

use rand::prelude::IndexedRandom;
use rand::rng;
use rand::seq::SliceRandom;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;
use superconsole::style::Stylize;

use crate::console::Control;

// Currently gravity is in # frames for a single cell fall. this doesn't support falling multiple
// positions in one frame like some games might.
type Gravity = u32;

const HEIGHT: usize = 24; // +4 for hidden area
const WIDTH: usize = 20;

const CLEAR_MATCHES_TICKS: u32 = 30;
const FLOATER_GRAVITY: Gravity = 5;

const MATCH_LENGTH: usize = 4;

struct Board<T> {
    board: [[T; WIDTH]; HEIGHT],
}

impl<T: Debug> std::fmt::Debug for Board<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut board_display = Board::from_fn(|_, _| String::new());
        let mut max_len = 0;
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                board_display.set(x, y, format!("{:?}", self.board[y][x]));
                max_len = std::cmp::max(max_len, board_display.get(x, y).len());
            }
        }
        let mut board_dbg = "\n".to_owned();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                board_dbg += &format!(" {:width$} ", board_display.get(x, y), width = max_len);
            }
            board_dbg += "\n";
        }

        f.write_str(&board_dbg)
    }
}

impl<T: Clone> Board<T> {
    fn new() -> Self
    where
        T: Default + Copy,
    {
        Self::of(T::default())
    }

    fn of(v: T) -> Self
    where
        T: Copy,
    {
        Self {
            board: [[v; WIDTH]; HEIGHT],
        }
    }

    fn from_fn<F: FnMut(usize, usize) -> T>(mut cb: F) -> Self {
        Self {
            board: std::array::from_fn(move |y| std::array::from_fn(|x| cb(x, y))),
        }
    }

    fn get<N: TryInto<usize>>(&self, x: N, y: N) -> T {
        self.try_get(x, y).unwrap().clone()
    }

    fn try_get<N: TryInto<usize>>(&self, x: N, y: N) -> Option<T> {
        match (x.try_into(), y.try_into()) {
            (Ok(x), Ok(y)) => Some(self.board[y][x].clone()),
            _ => None,
        }
    }

    fn set<D: Debug, N: TryInto<usize, Error = D>>(&mut self, x: N, y: N, v: T) {
        self.board[y.try_into().unwrap()][x.try_into().unwrap()] = v;
    }
}

enum RotateDirection {
    Clockwise,
    CounterClockwise,
}

#[derive(Debug, Clone, Copy)]
enum PieceRotation {
    Zero,
    Ninety,
    OneEighty,
    TwoSeventy,
}
impl PieceRotation {
    fn rotate(self, dir: RotateDirection) -> Self {
        match dir {
            RotateDirection::CounterClockwise => self
                .rotate(RotateDirection::Clockwise)
                .rotate(RotateDirection::Clockwise)
                .rotate(RotateDirection::Clockwise),
            _ => match self {
                PieceRotation::Zero => PieceRotation::Ninety,
                PieceRotation::Ninety => PieceRotation::OneEighty,
                PieceRotation::OneEighty => PieceRotation::TwoSeventy,
                PieceRotation::TwoSeventy => PieceRotation::Zero,
            },
        }
    }
}

#[derive(Debug)]
enum Piece {
    I,
    O,
    S,
    Z,
    L,
    J,
    T,
}

impl Piece {
    #[rustfmt::skip]
    fn grid(&self, rot: PieceRotation) -> &[[u8; 4]; 4] {
        match (self, rot) {
            (Piece::I, PieceRotation::Zero) => &[
                [0, 1, 0, 0],
                [0, 2, 0, 0],
                [0, 3, 0, 0],
                [0, 4, 0, 0],
            ],
            (Piece::I, PieceRotation::Ninety) => &[
                [0, 0, 0, 0],
                [4, 3, 2, 1],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::I, PieceRotation::OneEighty) => &[
                [0, 4, 0, 0],
                [0, 3, 0, 0],
                [0, 2, 0, 0],
                [0, 1, 0, 0],
            ],
            (Piece::I, PieceRotation::TwoSeventy) => &[
                [0, 0, 0, 0],
                [1, 2, 3, 4],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::O, PieceRotation::Zero) => &[
                [1, 2, 0, 0],
                [4, 3, 0, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::O, PieceRotation::Ninety) => &[
                [4, 1, 0, 0],
                [3, 2, 0, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::O, PieceRotation::OneEighty) => &[
                [3, 4, 0, 0],
                [2, 1, 0, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::O, PieceRotation::TwoSeventy) => &[
                [2, 3, 0, 0],
                [1, 4, 0, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::S, PieceRotation::Zero) => &[
                [0, 0, 0, 0],
                [0, 3, 4, 0],
                [1, 2, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::S, PieceRotation::Ninety) => &[
                [1, 0, 0, 0],
                [2, 3, 0, 0],
                [0, 4, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::S, PieceRotation::OneEighty) => &[
                [0, 2, 1, 0],
                [4, 3, 0, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::S, PieceRotation::TwoSeventy) => &[
                [0, 4, 0, 0],
                [0, 3, 2, 0],
                [0, 0, 1, 0],
                [0, 0, 0, 0],
            ],
            (Piece::Z, PieceRotation::Zero) => &[
                [0, 0, 0, 0],
                [1, 2, 0, 0],
                [0, 3, 4, 0],
                [0, 0, 0, 0],
            ],
            (Piece::Z, PieceRotation::Ninety) => &[
                [0, 1, 0, 0],
                [3, 2, 0, 0],
                [4, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::Z, PieceRotation::OneEighty) => &[
                [4, 3, 0, 0],
                [0, 2, 1, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::Z, PieceRotation::TwoSeventy) => &[
                [0, 0, 4, 0],
                [0, 2, 3, 0],
                [0, 1, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::L, PieceRotation::Zero) => &[
                [0, 1, 0, 0],
                [0, 2, 0, 0],
                [0, 3, 4, 0],
                [0, 0, 0, 0],
            ],
            (Piece::L, PieceRotation::Ninety) => &[
                [0, 0, 0, 0],
                [3, 2, 1, 0],
                [4, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::L, PieceRotation::OneEighty) => &[
                [4, 3, 0, 0],
                [0, 2, 0, 0],
                [0, 1, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::L, PieceRotation::TwoSeventy) => &[
                [0, 0, 4, 0],
                [1, 2, 3, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::J, PieceRotation::Zero) => &[
                [0, 1, 0, 0],
                [0, 2, 0, 0],
                [4, 3, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::J, PieceRotation::Ninety) => &[
                [4, 0, 0, 0],
                [3, 2, 1, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::J, PieceRotation::OneEighty) => &[
                [0, 3, 4, 0],
                [0, 2, 0, 0],
                [0, 1, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::J, PieceRotation::TwoSeventy) => &[
                [0, 0, 0, 0],
                [1, 2, 3, 0],
                [0, 0, 4, 0],
                [0, 0, 0, 0],
            ],
            (Piece::T, PieceRotation::Zero) => &[
                [0, 4, 0, 0],
                [1, 2, 3, 0],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::T, PieceRotation::Ninety) => &[
                [0, 1, 0, 0],
                [0, 2, 4, 0],
                [0, 3, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::T, PieceRotation::OneEighty) => &[
                [0, 0, 0, 0],
                [3, 2, 1, 0],
                [0, 4, 0, 0],
                [0, 0, 0, 0],
            ],
            (Piece::T, PieceRotation::TwoSeventy) => &[
                [0, 3, 0, 0],
                [4, 2, 0, 0],
                [0, 1, 0, 0],
                [0, 0, 0, 0],
            ],
        }
    }

    fn at(&self, rotation: PieceRotation, at: Position) -> u8 {
        match (at.x, at.y) {
            (x, _) if x < 0 || x > 3 => 0,
            (_, y) if y < 0 || y > 3 => 0,
            (x, y) => self.grid(rotation)[y as usize][x as usize],
        }
    }

    fn iter(&self, rotation: PieceRotation) -> impl Iterator<Item = (Position, u8)> {
        let grid = self.grid(rotation);
        let mut positions = [(Position::ORIGIN, 0); 4];
        let mut i = 0;
        for (y, row) in grid.iter().enumerate() {
            for (x, &cell) in row.iter().enumerate() {
                if cell != 0 {
                    positions[i] = (Position::new(x, y), cell);
                    i += 1;
                }
            }
        }
        assert!(i == 4);
        positions.into_iter()
    }
}

#[derive(Debug)]
struct ColoredPiece {
    kind: Piece,
    colors: [Color; 4],
}

impl ColoredPiece {
    fn colorize(kind: Piece, colors: &[Color]) -> Self {
        Self {
            kind,
            colors: (0..4)
                .map(|_| *colors.choose(&mut rng()).unwrap())
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
        }
    }
}

#[derive(Debug)]
struct SevenBag {
    items: Vec<Piece>,
}

impl SevenBag {
    fn new() -> Self {
        let mut items = vec![
            Piece::I,
            Piece::O,
            Piece::S,
            Piece::Z,
            Piece::L,
            Piece::J,
            Piece::T,
        ];
        items.shuffle(&mut rng());
        Self { items }
    }

    fn take(&mut self) -> Option<Piece> {
        self.items.pop()
    }
}

#[derive(Debug)]
struct PieceGenerator {
    colors: Vec<Color>,
    next: ColoredPiece,
    bag: SevenBag,
}

impl PieceGenerator {
    fn new() -> Self {
        let mut bag = SevenBag::new();
        let colors = vec![Color::Blue, Color::Red, Color::Green];
        let next = bag.take().unwrap();
        Self {
            bag,
            next: ColoredPiece::colorize(next, &colors),
            colors,
        }
    }

    fn take(&mut self) -> ColoredPiece {
        let v = if let Some(v) = self.bag.take() {
            v
        } else {
            self.bag = SevenBag::new();
            self.bag.take().unwrap()
        };
        let mut v = ColoredPiece::colorize(v, &self.colors);
        std::mem::swap(&mut v, &mut self.next);
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Position {
    x: i32,
    y: i32,
}

impl Position {
    const ORIGIN: Position = Position { x: 0, y: 0 };

    fn translate(mut self, x: i32, y: i32) -> Self {
        self.x += x;
        self.y += y;
        self
    }

    fn add(self, position: Position) -> Self {
        self.translate(position.x, position.y)
    }

    fn sub(self, position: Position) -> Self {
        self.translate(-position.x, -position.y)
    }

    fn new<E: Debug, T: TryInto<i32, Error = E>>(x: T, y: T) -> Self {
        Self {
            x: x.try_into().unwrap(),
            y: y.try_into().unwrap(),
        }
    }
}

#[derive(Debug)]
struct FallingPiece {
    piece: ColoredPiece,
    rotation: PieceRotation,
    location: Position,
}

impl FallingPiece {
    fn iter(&self) -> impl Iterator<Item = (Position, Color)> + '_ {
        self.piece.kind.iter(self.rotation).map(|(pos, color_idx)| {
            (
                pos.add(self.location),
                self.piece.colors[(color_idx - 1) as usize],
            )
        })
    }

    fn color_at(&self, x: usize, y: usize) -> Option<Color> {
        match self
            .piece
            .kind
            .at(self.rotation, Position::new(x, y).sub(self.location))
        {
            0 => None,
            v => Some(self.piece.colors[(v - 1) as usize]),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FloaterState {
    Empty,
    Locked,
    FloatGroup(usize),
}

#[derive(Debug)]
enum PieceState {
    Spawned,
    Falling {
        next_move_tick: u32,
    },
    Locking {
        lock_tick: u32,
    },
    ClearingMatches {
        start_tick: u32,
        matches: HashSet<Position>,
    },
    Floaters {
        next_move_tick: u32,
        state: Box<Board<FloaterState>>,
    },
}

#[derive(Debug)]
pub struct Game {
    stack: Board<Option<Color>>,
    pieces: PieceGenerator,
    current: FallingPiece,
    state: PieceState,
    gravity: Gravity,
    tick_count: u32,
    score: u64,
}

impl Game {
    fn create_falling_piece(piece_gen: &mut PieceGenerator) -> FallingPiece {
        FallingPiece {
            piece: piece_gen.take(),
            rotation: PieceRotation::Zero,
            location: Position {
                x: (WIDTH as i32) / 2,
                y: 0,
            },
        }
    }

    pub fn new() -> Self {
        let stack = Board::new();
        let mut pieces = PieceGenerator::new();
        let current = Self::create_falling_piece(&mut pieces);
        Self {
            stack,
            pieces,
            current,
            state: PieceState::Spawned,
            gravity: 10,
            tick_count: 0,
            score: 0,
        }
    }

    fn draw_width(&self) -> usize {
        WIDTH
    }

    fn draw_height(&self) -> usize {
        HEIGHT - 4
    }

    pub fn tick(&mut self, tick_count: u32) -> bool {
        self.tick_count = tick_count;
        match &self.state {
            PieceState::Spawned => {
                self.state = PieceState::Falling {
                    next_move_tick: tick_count + self.gravity,
                }
            }
            PieceState::Falling { next_move_tick } if *next_move_tick <= tick_count => {
                if self.down_one() {
                    self.state = PieceState::Falling {
                        next_move_tick: tick_count + self.gravity,
                    };
                } else {
                    assert!(matches!(self.state, PieceState::Locking { .. }));
                }
            }
            PieceState::Locking { lock_tick, .. } if *lock_tick <= tick_count => {
                // the piece may have been moved to a non-bottom location/rotation, so try to drop a cell and if that succeeds we go back to falling.
                if self.down_one() {
                    self.state = PieceState::Falling {
                        next_move_tick: tick_count + self.gravity,
                    };
                } else {
                    // lock the piece, switch to scoring state
                    let hidden_rows = HEIGHT - self.draw_height();
                    for (pos, color) in self.current.iter() {
                        let (x, y) = (pos.x as usize, pos.y as usize);
                        if y < hidden_rows || self.stack.get(x, y).is_some() {
                            return false;
                        }
                        self.stack.set(x, y, Some(color));
                    }
                    self.check_for_matches()
                }
            }
            PieceState::ClearingMatches {
                start_tick,
                matches,
            } => {
                if tick_count > *start_tick + CLEAR_MATCHES_TICKS {
                    for pos in matches {
                        self.stack.set(pos.x, pos.y, None);
                    }
                    self.check_for_floaters()
                }
            }
            PieceState::Floaters {
                next_move_tick,
                state,
            } => {
                if tick_count < *next_move_tick {
                    return true;
                }

                let mut floaters_to_lock = HashSet::new();
                let mut new_state = Board::of(FloaterState::Empty);

                // Floaters all drop one cell, update stack and construct new floater state. Check if any floaters have hit bottom.
                for y in (0..HEIGHT).rev() {
                    // floaters can't be on the bottom row
                    for x in 0..WIDTH {
                        match state.get(x, y) {
                            FloaterState::Empty => {
                                assert!(self.stack.get(x, y).is_none());
                            }
                            FloaterState::Locked => {
                                assert!(self.stack.get(x, y).is_some());
                                new_state.set(x, y, FloaterState::Locked)
                            }
                            FloaterState::FloatGroup(n) => {
                                assert!(self.stack.get(x, y).is_some());
                                assert!(
                                    self.stack.get(x, y + 1).is_none(),
                                    "bad floater at {},{}: {:?} {:?}",
                                    x,
                                    y,
                                    state,
                                    self.stack
                                );
                                self.stack.set(x, y + 1, self.stack.get(x, y));
                                self.stack.set(x, y, None);
                                new_state.set(x, y + 1, state.get(x, y));

                                if y + 2 == HEIGHT
                                    || matches!(new_state.get(x, y + 2), FloaterState::Locked)
                                {
                                    floaters_to_lock.insert(n);
                                }
                            }
                        }
                    }
                }

                // lock any floaters that have hit bottom
                let mut remaining_floaters;
                if floaters_to_lock.is_empty() {
                    remaining_floaters = true
                } else {
                    remaining_floaters = false;
                    for x in 0..WIDTH {
                        for y in 0..HEIGHT {
                            if let FloaterState::FloatGroup(n) = new_state.get(x, y) {
                                if floaters_to_lock.contains(&n) {
                                    new_state.set(x, y, FloaterState::Locked);
                                } else {
                                    remaining_floaters = true;
                                }
                            }
                        }
                    }
                }

                if remaining_floaters {
                    self.state = PieceState::Floaters {
                        next_move_tick: tick_count + FLOATER_GRAVITY,
                        state: Box::new(new_state),
                    }
                } else {
                    self.check_for_matches()
                }
            }
            _ => {
                // nothing to do
            }
        }
        true
    }

    fn check_for_matches(&mut self) {
        let mut matches = HashSet::new();

        for y in 0..HEIGHT {
            let mut left = 0;

            while left < WIDTH {
                match self.stack.get(left, y) {
                    None => {
                        left += 1;
                    }
                    Some(left_color) => {
                        let mut right = left + 1;
                        while right < WIDTH && self.stack.get(right, y) == Some(left_color) {
                            right += 1;
                        }
                        let run_len = right - left;
                        if run_len >= MATCH_LENGTH {
                            // Score: (n - 3)^2, so 4=1, 5=4, 6=9, ...
                            let bonus = (run_len - 3) as u64;
                            self.score += bonus * bonus;
                            for x in left..right {
                                matches.insert(Position::new(x, y));
                            }
                        }
                        left = right;
                    }
                }
            }
        }

        for x in 0..WIDTH {
            let mut top = 0;

            while top < HEIGHT {
                match self.stack.get(x, top) {
                    None => {
                        top += 1;
                    }
                    Some(top_color) => {
                        let mut bot = top + 1;
                        while bot < HEIGHT && self.stack.get(x, bot) == Some(top_color) {
                            bot += 1;
                        }
                        let run_len = bot - top;
                        if run_len >= MATCH_LENGTH {
                            let bonus = (run_len - 3) as u64;
                            self.score += bonus * bonus;
                            for y in top..bot {
                                matches.insert(Position::new(x, y));
                            }
                        }
                        top = bot;
                    }
                }
            }
        }

        if matches.is_empty() {
            self.spawn_piece()
        } else {
            self.state = PieceState::ClearingMatches {
                start_tick: self.tick_count,
                matches,
            }
        }
    }

    fn check_for_floaters(&mut self) {
        struct DJSet {
            groups: Vec<usize>,
        }
        impl DJSet {
            fn union(&mut self, mut left: usize, mut right: usize) {
                left = self.get(left);
                right = self.get(right);
                self.groups[left] = right;
            }

            fn get(&mut self, id: usize) -> usize {
                if self.groups[id] == id {
                    id
                } else {
                    let res = self.get(self.groups[id]);
                    self.groups[id] = res;
                    res
                }
            }

            fn new_set(&mut self) -> usize {
                let res = self.groups.len();
                self.groups.push(res);
                res
            }
        }
        let mut groups = DJSet { groups: vec![0] };
        let mut cells = Board::of(None::<usize>);

        fn maybe_union(
            groups: &mut DJSet,
            cells: &Board<Option<usize>>,
            existing: Position,
            candidate: Position,
        ) {
            if candidate.y >= 0 && candidate.y < (HEIGHT as i32) {
                if candidate.x >= 0 && candidate.x < (WIDTH as i32) {
                    if let Some(other) = cells.get(candidate.x, candidate.y) {
                        let curr = cells.get(existing.x, existing.y).unwrap();
                        groups.union(curr, other);
                    }
                }
            }
        }

        for x in 0..WIDTH {
            for y in (0..HEIGHT).rev() {
                if self.stack.get(x, y).is_some() {
                    if y == HEIGHT - 1 {
                        cells.set(x, y, Some(0));
                    } else {
                        cells.set(x, y, Some(groups.new_set()));
                    }
                    let pos = Position::new(x, y);
                    maybe_union(&mut groups, &cells, pos, pos.translate(-1, 0));
                    maybe_union(&mut groups, &cells, pos, pos.translate(0, 1));
                }
            }
        }
        let mut state = Board::of(FloaterState::Empty);
        let mut found_floaters = false;
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                state.set(
                    x,
                    y,
                    match cells.get(x, y) {
                        None => FloaterState::Empty,
                        Some(n) => {
                            let group = groups.get(n);
                            // we used 0 for blocks on the bottom row, via union it could now have a new identifier
                            if group == groups.get(0) {
                                FloaterState::Locked
                            } else {
                                found_floaters = true;
                                FloaterState::FloatGroup(group)
                            }
                        }
                    },
                );
            }
        }

        if found_floaters {
            self.state = PieceState::Floaters {
                next_move_tick: self.tick_count + FLOATER_GRAVITY,
                state: Box::new(state),
            }
        } else {
            self.spawn_piece()
        }
    }

    fn spawn_piece(&mut self) {
        self.current = Self::create_falling_piece(&mut self.pieces);
        self.state = PieceState::Falling {
            next_move_tick: self.tick_count + self.gravity,
        }
    }

    fn start_locking(&mut self) {
        // get 0.5 second to maybe rotate/move before locked
        self.state = PieceState::Locking {
            lock_tick: self.tick_count + 15,
        }
    }

    fn maybe_reset_locking(&mut self) {
        if let PieceState::Locking { .. } = self.state {
            self.start_locking();
        }
    }

    fn test_candidate(&self, position: Position, rotation: PieceRotation) -> bool {
        for (mut pos, _) in self.current.piece.kind.iter(rotation) {
            pos = pos.add(position);
            match (pos.x, pos.y) {
                (x, _) if x < 0 || x >= (WIDTH as i32) => {
                    return false;
                }
                (_, y) if y < 0 || y >= (HEIGHT as i32) => {
                    return false;
                }
                (x, y) if self.stack.get(x, y).is_some() => {
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    fn try_move(&mut self, position: Position, rotation: PieceRotation) -> bool {
        if self.test_candidate(position, rotation) {
            self.current.location = position;
            self.current.rotation = rotation;
            self.maybe_reset_locking();
            true
        } else {
            false
        }
    }

    fn down_one(&mut self) -> bool {
        if self.try_move(self.current.location.translate(0, 1), self.current.rotation) {
            true
        } else {
            match self.state {
                PieceState::Locking { .. } => {
                    self.state = PieceState::Locking {
                        lock_tick: self.tick_count,
                    }
                }
                PieceState::Falling { .. } => self.start_locking(),
                _ => {}
            }
            false
        }
    }

    pub fn input(&mut self, input: Control) -> Option<Control> {
        // input is only accepted while a piece is falling or locking
        match self.state {
            PieceState::Falling { .. } | PieceState::Locking { .. } => {}
            _ => {
                return None;
            }
        }

        match input {
            Control::Up | Control::Char('w') => {
                self.try_move(
                    self.current.location,
                    self.current.rotation.rotate(RotateDirection::Clockwise),
                );
            }
            Control::Left | Control::Char('a') => {
                self.try_move(
                    self.current.location.translate(-1, 0),
                    self.current.rotation,
                );
            }
            Control::Down | Control::Char('s') => {
                self.down_one();
            }
            Control::Right | Control::Char('d') => {
                self.try_move(self.current.location.translate(1, 0), self.current.rotation);
            }
            Control::Char('c') => {
                self.try_move(
                    self.current.location,
                    self.current
                        .rotation
                        .rotate(RotateDirection::CounterClockwise),
                );
            }
            Control::Char(' ') => {
                // If falling, this will drop as far as it can and then start locking
                while self.down_one() {}
            }
            other => return Some(other),
        }

        None
    }

    fn color_at(&self, x: usize, y: usize) -> Option<Color> {
        if let Some(color) = self.stack.get(x, y) {
            return Some(color);
        }
        match &self.state {
            PieceState::Falling { .. } | PieceState::Locking { .. } => self.current.color_at(x, y),
            _ => None,
        }
    }
}

impl super::Game for Game {
    fn tick(&mut self, tick_count: u32) -> super::TickResult {
        super::TickResult {
            alive: self.tick(tick_count),
            scores: vec![super::Score {
                category: "blocks".to_owned(),
                value: self.score,
                lower_is_better: false,
            }],
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        vec![super::Score {
            category: "blocks".to_owned(),
            value: self.score,
            lower_is_better: false,
        }]
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        self.input(input)
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&self.stack.board).ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        if let Ok(board) = serde_json::from_str::<[[Option<Color>; WIDTH]; HEIGHT]>(json) {
            self.stack.board = board;
            self.pieces = PieceGenerator::new();
            self.current = Self::create_falling_piece(&mut self.pieces);
            self.state = PieceState::Spawned;
            true
        } else {
            false
        }
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        const T: &str = "▎▔▔▕";
        const B: &str = "▎▁▁▕";
        const E: &str = "    ";
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                let mut lines: Vec<Line> = Vec::with_capacity(self.draw_height() * 2 + 3);
                lines.push(vec![format!("Score: {}", self.score)].try_into().unwrap());
                let inner_width = self.draw_width() * 4;
                lines.push(
                    vec!["╔".to_owned(), "═".repeat(inner_width), "╗".to_owned()]
                        .try_into()
                        .unwrap(),
                );

                for y in (HEIGHT - self.draw_height())..HEIGHT {
                    let mut top_line = Line::default();
                    let mut bot_line = Line::default();

                    top_line.push(Span::new_unstyled_lossy("║"));
                    bot_line.push(Span::new_unstyled_lossy("║"));

                    for x in 0..self.draw_width() {
                        match &self.color_at(x, y) {
                            None => {
                                top_line.push(Span::new_unstyled_lossy(E));
                                bot_line.push(Span::new_unstyled_lossy(E));
                            }
                            Some(v) => {
                                let mut t = T.on(*v);
                                let mut b = B.on(*v);
                                if let PieceState::ClearingMatches {
                                    start_tick: _,
                                    matches,
                                } = &self.state
                                {
                                    if matches.contains(&Position::new(x, y)) {
                                        let color = [
                                            Color::Red,
                                            Color::Green,
                                            Color::Blue,
                                            Color::Yellow,
                                            Color::Cyan,
                                            Color::Black,
                                            Color::White,
                                        ]
                                        .choose(&mut rng())
                                        .unwrap();
                                        t = t.with(*color);
                                        b = b.with(*color);
                                    }
                                }

                                top_line.push(Span::new_styled_lossy_str(t));
                                bot_line.push(Span::new_styled_lossy_str(b));
                            }
                        }
                    }
                    top_line.push(Span::new_unstyled_lossy("║"));
                    bot_line.push(Span::new_unstyled_lossy("║"));

                    lines.push(top_line);
                    lines.push(bot_line);
                }
                lines.push(
                    vec!["╚".to_owned(), "═".repeat(inner_width), "╝".to_owned()]
                        .try_into()
                        .unwrap(),
                );
                Lines(lines)
            }
        })
    }
}
