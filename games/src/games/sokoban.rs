/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sokoban box-pushing puzzle with built-in A* solver and procedurally
//! generated levels.
//!
//! Beyond a basic sokoban:
//!
//! - **A\* solver**: full push-optimal solver with state normalization
//!   (player position canonicalized to top-left of reachable region),
//!   dead-cell pruning via reverse BFS from targets, and greedy
//!   Manhattan-distance heuristic. Can verify solvability and display
//!   solutions as numbered move sequences (e.g. "1U 2L 3D").
//!
//! - **Procedural level generator** (`sokoban_gen`): generates random room
//!   shapes via 2×2 templates, places boxes on goals, then searches
//!   backward from the solved state using reverse iterative deepening
//!   with a box-lines metric to find the farthest reachable start
//!   position. Based on a simplified version of Taylor & Parberry's
//!   approach: <https://ianparberry.com/pubs/GAMEON-NA_METH_03.pdf>
//!
//! - **600+ pre-generated levels**: organized by box count (2–6 boxes)
//!   in separate level files, all guaranteed solvable by construction.
//!
//! - **Per-level move tracking**: scores record moves per level with
//!   `lower_is_better`, so the high score system tracks best (fewest
//!   moves) solutions.
//!
//! - **Numbered box mode**: toggle to display boxes with indices matching
//!   the solver's notation, useful for following solutions.
//!
//! - **Box-on-target coloring**: boxes turn green when on a target cell,
//!   player turns green when standing on a target.

use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;

use crate::console::Control;

const MAX_WIDTH: usize = 50;
const MAX_HEIGHT: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Empty,
    Wall,
    Target,
}

#[derive(Debug, Clone)]
struct Level {
    width: usize,
    height: usize,
    cells: [[Cell; MAX_WIDTH]; MAX_HEIGHT],
    boxes: Vec<(usize, usize)>,
    player: (usize, usize),
}

#[derive(Debug, Clone)]
pub struct Game {
    levels: Vec<Vec<String>>,
    level: Level,
    level_index: usize,
    moves: u32,
    won: bool,
    numbered: bool,
    completed_levels: Vec<(usize, u32)>,
    pending_scores: Vec<super::Score>,
}

const LEVELS_TEXT: &str = concat!(
    include_str!("sokoban_levels.txt"),
    "\n",
    include_str!("sokoban_levels_2box_large.txt"),
    "\n",
    include_str!("sokoban_levels_3box_large.txt"),
    "\n",
    include_str!("sokoban_levels_4box_large.txt"),
    "\n",
    include_str!("sokoban_levels_5box_large.txt"),
    "\n",
    include_str!("sokoban_levels_6box_large.txt"),
);

pub fn parse_levels_text(text: &str) -> Vec<Vec<String>> {
    let mut levels = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with(';') {
            continue;
        }
        if line.is_empty() {
            if !current.is_empty() {
                levels.push(current);
                current = Vec::new();
            }
        } else {
            current.push(line.to_owned());
        }
    }
    if !current.is_empty() {
        levels.push(current);
    }
    levels
}

fn default_levels() -> Vec<Vec<String>> {
    parse_levels_text(LEVELS_TEXT)
}

// --- A* Solver ---

const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
const DIR_NAMES: [char; 4] = ['U', 'D', 'L', 'R'];

/// Normalized state: player reachable-region id + sorted box positions.
/// We normalize the player position to the top-left-most cell in their
/// reachable region (with boxes as walls) so that states differing only
/// in player position within the same region are treated as identical.
#[derive(Clone, Eq, PartialEq, Hash)]
struct SolverState {
    player: (usize, usize),
    boxes: Vec<(usize, usize)>,
}

struct Solver<'a> {
    cells: &'a [[Cell; MAX_WIDTH]; MAX_HEIGHT],
    width: usize,
    height: usize,
    targets: Vec<(usize, usize)>,
    dead_cells: HashSet<(usize, usize)>,
}

impl<'a> Solver<'a> {
    fn new(level: &'a Level) -> Self {
        let targets: Vec<(usize, usize)> = (0..level.height)
            .flat_map(|y| {
                (0..level.width).filter_map(move |x| {
                    if level.cells[y][x] == Cell::Target {
                        Some((x, y))
                    } else {
                        None
                    }
                })
            })
            .collect();

        let mut solver = Solver {
            cells: &level.cells,
            width: level.width,
            height: level.height,
            targets: targets.clone(),
            dead_cells: HashSet::new(),
        };
        solver.compute_dead_cells();
        solver
    }

    fn is_floor(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height && self.cells[y][x] != Cell::Wall
    }

    /// Compute simple dead cells: floor cells from which a box can never reach
    /// any target. A box can reach a target if there exists a pull-path from
    /// the target to the cell (reverse reachability).
    fn compute_dead_cells(&mut self) {
        // BFS backwards from each target. A box at (x,y) was pushed there
        // from (x-dx,y-dy) with the player at (x-2dx,y-2dy). So in reverse,
        // the box goes back to (x-dx,y-dy) if both (x-dx,y-dy) and
        // (x-2dx,y-2dy) are floor cells.
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();
        for &(tx, ty) in &self.targets {
            if reachable.insert((tx, ty)) {
                queue.push_back((tx, ty));
            }
        }
        while let Some((x, y)) = queue.pop_front() {
            for &(dx, dy) in &DIRS {
                let nx = x as i32 - dx;
                let ny = y as i32 - dy;
                let px = x as i32 - 2 * dx;
                let py = y as i32 - 2 * dy;
                if nx >= 0
                    && ny >= 0
                    && (nx as usize) < self.width
                    && (ny as usize) < self.height
                    && px >= 0
                    && py >= 0
                    && (px as usize) < self.width
                    && (py as usize) < self.height
                    && self.is_floor(nx as usize, ny as usize)
                    && self.is_floor(px as usize, py as usize)
                {
                    if reachable.insert((nx as usize, ny as usize)) {
                        queue.push_back((nx as usize, ny as usize));
                    }
                }
            }
        }

        for y in 0..self.height {
            for x in 0..self.width {
                if self.is_floor(x, y) && !reachable.contains(&(x, y)) {
                    self.dead_cells.insert((x, y));
                }
            }
        }
    }

    /// BFS to find the normalized player position (top-left-most reachable cell).
    fn normalize_player(&self, player: (usize, usize), boxes: &[(usize, usize)]) -> (usize, usize) {
        let box_set: HashSet<(usize, usize)> = boxes.iter().copied().collect();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut best = player;
        visited.insert(player);
        queue.push_back(player);
        while let Some((x, y)) = queue.pop_front() {
            if (y, x) < (best.1, best.0) {
                best = (x, y);
            }
            for &(dx, dy) in &DIRS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < self.width && (ny as usize) < self.height {
                    let np = (nx as usize, ny as usize);
                    if self.is_floor(np.0, np.1) && !box_set.contains(&np) && visited.insert(np) {
                        queue.push_back(np);
                    }
                }
            }
        }
        best
    }

    fn make_state(&self, player: (usize, usize), boxes: &[(usize, usize)]) -> SolverState {
        let mut sorted_boxes: Vec<(usize, usize)> = boxes.to_vec();
        sorted_boxes.sort();
        let norm_player = self.normalize_player(player, &sorted_boxes);
        SolverState {
            player: norm_player,
            boxes: sorted_boxes,
        }
    }

    fn is_goal(&self, state: &SolverState) -> bool {
        let target_set: HashSet<(usize, usize)> = self.targets.iter().copied().collect();
        state.boxes.iter().all(|b| target_set.contains(b))
    }

    /// Heuristic: sum of minimum Manhattan distances from each box to nearest
    /// target (greedy assignment).
    fn heuristic(&self, state: &SolverState) -> u32 {
        let mut total = 0u32;
        for &(bx, by) in &state.boxes {
            let min_dist = self
                .targets
                .iter()
                .map(|&(tx, ty)| {
                    (bx as i32 - tx as i32).unsigned_abs() + (by as i32 - ty as i32).unsigned_abs()
                })
                .min()
                .unwrap_or(0);
            total += min_dist;
        }
        total
    }

    /// Check if placing a box here is a simple deadlock.
    fn is_dead(&self, x: usize, y: usize) -> bool {
        self.dead_cells.contains(&(x, y))
    }

    /// Solve using A*. Returns the solution as a list of (box_old_pos, direction_index)
    /// pairs, or None if unsolvable.
    pub fn solve(
        &self,
        player: (usize, usize),
        boxes: &[(usize, usize)],
    ) -> Option<Vec<((usize, usize), usize)>> {
        #[derive(Eq, PartialEq)]
        struct Node {
            f: u32,
            g: u32,
            state: SolverState,
        }

        impl Ord for Node {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                other.f.cmp(&self.f)
            }
        }

        impl PartialOrd for Node {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        let initial = self.make_state(player, boxes);
        let h = self.heuristic(&initial);

        let mut open = BinaryHeap::new();
        // Map from state to (cost, parent_state, direction_index, box_old_position)
        let mut came_from: HashMap<
            SolverState,
            (u32, Option<SolverState>, usize, Option<(usize, usize)>),
        > = HashMap::new();

        came_from.insert(initial.clone(), (0, None, 0, None));
        open.push(Node {
            f: h,
            g: 0,
            state: initial.clone(),
        });

        while let Some(Node { g, state, .. }) = open.pop() {
            if self.is_goal(&state) {
                // Reconstruct path
                let mut path = Vec::new();
                let mut cur = state;
                while let Some((_, Some(prev), dir, Some(box_pos))) = came_from.get(&cur).cloned() {
                    path.push((box_pos, dir));
                    cur = prev;
                }
                path.reverse();
                return Some(path);
            }

            if g > came_from.get(&state).map_or(u32::MAX, |v| v.0) {
                continue;
            }

            let box_set: HashSet<(usize, usize)> = state.boxes.iter().copied().collect();

            let mut reachable = HashSet::new();
            {
                let mut queue = VecDeque::new();
                reachable.insert(state.player);
                queue.push_back(state.player);
                while let Some((x, y)) = queue.pop_front() {
                    for &(dx, dy) in &DIRS {
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx >= 0
                            && ny >= 0
                            && (nx as usize) < self.width
                            && (ny as usize) < self.height
                        {
                            let np = (nx as usize, ny as usize);
                            if self.is_floor(np.0, np.1)
                                && !box_set.contains(&np)
                                && reachable.insert(np)
                            {
                                queue.push_back(np);
                            }
                        }
                    }
                }
            }

            // For each box, check if we can push it in each direction
            for (bi, &(bx, by)) in state.boxes.iter().enumerate() {
                for (di, &(dx, dy)) in DIRS.iter().enumerate() {
                    let px = bx as i32 - dx;
                    let py = by as i32 - dy;
                    let nbx = bx as i32 + dx;
                    let nby = by as i32 + dy;

                    if px < 0
                        || py < 0
                        || nbx < 0
                        || nby < 0
                        || (px as usize) >= self.width
                        || (py as usize) >= self.height
                        || (nbx as usize) >= self.width
                        || (nby as usize) >= self.height
                    {
                        continue;
                    }

                    let player_pos = (px as usize, py as usize);
                    let new_box_pos = (nbx as usize, nby as usize);

                    if !reachable.contains(&player_pos) {
                        continue;
                    }

                    if !self.is_floor(new_box_pos.0, new_box_pos.1)
                        || box_set.contains(&new_box_pos)
                    {
                        continue;
                    }

                    if self.is_dead(new_box_pos.0, new_box_pos.1) {
                        continue;
                    }

                    let mut new_boxes = state.boxes.clone();
                    new_boxes[bi] = new_box_pos;

                    let new_player = (bx, by);
                    let new_state = self.make_state(new_player, &new_boxes);
                    let new_g = g + 1;

                    let existing_g = came_from.get(&new_state).map_or(u32::MAX, |v| v.0);
                    if new_g < existing_g {
                        let h = self.heuristic(&new_state);
                        came_from.insert(
                            new_state.clone(),
                            (new_g, Some(state.clone()), di, Some((bx, by))),
                        );
                        open.push(Node {
                            f: new_g + h,
                            g: new_g,
                            state: new_state,
                        });
                    }
                }
            }
        }

        None
    }
}

/// Format solver output into a string like "1U 2L 3D".
/// Maps box positions back to their original 1-indexed numbers
/// (numbered left-to-right, top-to-bottom from the initial state).
fn format_solution(initial_boxes: &[(usize, usize)], moves: &[((usize, usize), usize)]) -> String {
    // Track current position of each numbered box
    let mut positions: Vec<(usize, usize)> = initial_boxes.to_vec();
    let mut parts = Vec::new();

    for &(old_pos, dir) in moves {
        // Find which numbered box is at old_pos
        let box_num = positions
            .iter()
            .position(|&p| p == old_pos)
            .expect("box not found at position");

        let (dx, dy) = DIRS[dir];
        let new_pos = (
            (old_pos.0 as i32 + dx) as usize,
            (old_pos.1 as i32 + dy) as usize,
        );
        positions[box_num] = new_pos;
        parts.push(format!("{}{}", box_num + 1, DIR_NAMES[dir]));
    }

    parts.join(" ")
}

/// Solve a level from the default levels and return the solution string.
pub fn solve_level(level_index: usize) -> Option<String> {
    let levels = default_levels();
    solve_from_levels(&levels, level_index)
}

/// Solve a level from a parsed levels list.
pub fn solve_from_levels(levels: &[Vec<String>], level_index: usize) -> Option<String> {
    if level_index >= levels.len() {
        return None;
    }
    let level = Level::parse(&levels[level_index]);
    let solver = Solver::new(&level);
    solver
        .solve(level.player, &level.boxes)
        .map(|moves| format_solution(&level.boxes, &moves))
}

/// Solve a level from raw strings.
pub fn solve_level_str(strings: &[&str]) -> Option<String> {
    let level = Level::parse(strings);
    let solver = Solver::new(&level);
    solver
        .solve(level.player, &level.boxes)
        .map(|moves| format_solution(&level.boxes, &moves))
}

impl Level {
    fn parse<S: AsRef<str>>(strings: &[S]) -> Self {
        let mut cells = [[Cell::Empty; MAX_WIDTH]; MAX_HEIGHT];
        let mut boxes = Vec::new();
        let mut player = (0, 0);
        let height = strings.len();
        let width = strings.iter().map(|s| s.as_ref().len()).max().unwrap_or(0);

        for (y, row) in strings.iter().enumerate() {
            for (x, ch) in row.as_ref().chars().enumerate() {
                match ch {
                    '#' => cells[y][x] = Cell::Wall,
                    '.' => cells[y][x] = Cell::Target,
                    '$' => boxes.push((x, y)),
                    '@' => player = (x, y),
                    '+' => {
                        cells[y][x] = Cell::Target;
                        player = (x, y);
                    }
                    '*' => {
                        cells[y][x] = Cell::Target;
                        boxes.push((x, y));
                    }
                    _ => {}
                }
            }
        }

        Level {
            width,
            height,
            cells,
            boxes,
            player,
        }
    }
}

impl Game {
    pub fn new() -> Self {
        let levels = default_levels();
        let level = Level::parse(&levels[0]);
        Self {
            levels,
            level,
            level_index: 0,
            moves: 0,
            won: false,
            numbered: false,
            completed_levels: Vec::new(),
            pending_scores: Vec::new(),
        }
    }

    pub fn new_at_level(level_index: usize) -> Self {
        let levels = default_levels();
        let index = level_index.min(levels.len().saturating_sub(1));
        let level = Level::parse(&levels[index]);
        Self {
            levels,
            level,
            level_index: index,
            moves: 0,
            won: false,
            numbered: false,
            completed_levels: Vec::new(),
            pending_scores: Vec::new(),
        }
    }

    pub fn new_from_levels(levels: Vec<Vec<String>>, level_index: usize) -> Self {
        let index = level_index.min(levels.len().saturating_sub(1));
        let level = Level::parse(&levels[index]);
        Self {
            levels,
            level,
            level_index: index,
            moves: 0,
            won: false,
            numbered: false,
            completed_levels: Vec::new(),
            pending_scores: Vec::new(),
        }
    }

    pub fn set_numbered(&mut self, numbered: bool) {
        self.numbered = numbered;
    }

    fn num_levels(&self) -> usize {
        self.levels.len()
    }

    fn reload_level(&mut self) {
        self.level = Level::parse(&self.levels[self.level_index]);
    }

    fn try_move(&mut self, dx: i32, dy: i32) {
        let (px, py) = self.level.player;
        let nx = (px as i32 + dx) as usize;
        let ny = (py as i32 + dy) as usize;

        if self.level.cells[ny][nx] == Cell::Wall {
            return;
        }

        if let Some(box_idx) = self.level.boxes.iter().position(|&b| b == (nx, ny)) {
            let bx = (nx as i32 + dx) as usize;
            let by = (ny as i32 + dy) as usize;
            if self.level.cells[by][bx] == Cell::Wall {
                return;
            }
            if self.level.boxes.contains(&(bx, by)) {
                return;
            }
            self.level.boxes[box_idx] = (bx, by);
        }

        self.level.player = (nx, ny);
        self.moves += 1;

        self.check_win();
    }

    fn check_win(&mut self) {
        let all_on_target = self
            .level
            .boxes
            .iter()
            .all(|&(x, y)| self.level.cells[y][x] == Cell::Target);
        if all_on_target {
            self.completed_levels.push((self.level_index, self.moves));
            self.pending_scores.push(super::Score {
                category: format!("Level {}", self.level_index + 1),
                value: self.moves as u64,
                lower_is_better: true,
            });
            self.level_index += 1;
            if self.level_index < self.num_levels() {
                self.reload_level();
                self.moves = 0;
            } else {
                self.won = true;
            }
        }
    }
}

impl super::Game for Game {
    fn tick(&mut self, _tick_count: u32) -> super::TickResult {
        super::TickResult {
            alive: !self.won,
            scores: std::mem::take(&mut self.pending_scores),
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        self.completed_levels
            .iter()
            .map(|(level, moves)| super::Score {
                category: format!("Level {}", level + 1),
                value: *moves as u64,
                lower_is_better: true,
            })
            .collect()
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        match input {
            Control::Up | Control::Char('w') => self.try_move(0, -1),
            Control::Down | Control::Char('s') => self.try_move(0, 1),
            Control::Left | Control::Char('a') => self.try_move(-1, 0),
            Control::Right | Control::Char('d') => self.try_move(1, 0),
            Control::Char('r') => {
                self.reload_level();
                self.moves = 0;
            }
            other => return Some(other),
        }
        None
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&(
            self.level_index,
            self.moves,
            &self.level.boxes,
            self.level.player,
        ))
        .ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        type State = (usize, u32, Vec<(usize, usize)>, (usize, usize));
        if let Ok((level_index, moves, boxes, player)) = serde_json::from_str::<State>(json) {
            if level_index < self.num_levels() {
                self.level_index = level_index;
                self.reload_level();
                self.moves = moves;
                self.level.boxes = boxes;
                self.level.player = player;
                self.won = false;
                true
            } else {
                false
            }
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
                lines.push(
                    vec![format!(
                        "Level: {}  Moves: {}",
                        self.level_index + 1,
                        self.moves
                    )]
                    .try_into()
                    .unwrap(),
                );

                for y in 0..self.level.height {
                    let mut top = Line::default();
                    let mut bot = Line::default();
                    for x in 0..self.level.width {
                        let is_player = self.level.player == (x, y);
                        let box_idx = self.level.boxes.iter().position(|&b| b == (x, y));
                        let cell = self.level.cells[y][x];

                        if is_player {
                            let color = if cell == Cell::Target {
                                Color::Green
                            } else {
                                Color::Cyan
                            };
                            top.push(Span::new_colored_lossy("▗▄▖", color));
                            bot.push(Span::new_colored_lossy("▝▀▘", color));
                        } else if let Some(bi) = box_idx {
                            let color = if cell == Cell::Target {
                                Color::Green
                            } else {
                                Color::Yellow
                            };
                            if self.numbered {
                                let n = format!("{}", bi + 1);
                                let label = format!("{0}{0}{0}", n);
                                top.push(Span::new_colored_lossy(&label, color));
                                bot.push(Span::new_colored_lossy(&label, color));
                            } else {
                                top.push(Span::new_colored_lossy("╭─╮", color));
                                bot.push(Span::new_colored_lossy("╰─╯", color));
                            }
                        } else {
                            match cell {
                                Cell::Wall => {
                                    top.push(Span::new_colored_lossy("███", Color::Grey));
                                    bot.push(Span::new_colored_lossy("███", Color::Grey));
                                }
                                Cell::Target => {
                                    top.push(Span::new_colored_lossy("░░░", Color::Grey));
                                    bot.push(Span::new_colored_lossy("░░░", Color::Grey));
                                }
                                Cell::Empty => {
                                    top.push(Span::new_unstyled_lossy("   "));
                                    bot.push(Span::new_unstyled_lossy("   "));
                                }
                            }
                        }
                    }
                    lines.push(top);
                    lines.push(bot);
                }

                if self.won {
                    lines.push(vec!["All levels complete!".to_owned()].try_into().unwrap());
                }
                lines.push(
                    vec!["[arrows] move  [r] restart level".to_owned()]
                        .try_into()
                        .unwrap(),
                );
                Lines(lines)
            }
        })
    }
}
