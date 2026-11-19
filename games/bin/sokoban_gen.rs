/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Sokoban level generator based on a simplified version of
//! Taylor & Parberry's approach: <https://ianparberry.com/pubs/GAMEON-NA_METH_03.pdf>
//!
//! Algorithm:
//! 1. Generate a random room shape using 2x2 templates on a grid.
//! 2. Validate the room (connectivity, no 3-wall dead ends, enough floor).
//! 3. Place N boxes on goal positions (try all combinations, pick the one
//!    whose farthest reachable state is deepest).
//! 4. Find the farthest state from the goal using reverse iterative deepening
//!    with the box-lines metric. This state becomes the puzzle start.
//!
//! The result is guaranteed solvable because the start state was reached by
//! reversing legal moves from the solved goal state.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::Instant;

use clap::Parser;
use rand::Rng;
use rand::RngExt;

// Directions: up, down, left, right
const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

// --- Room Generation ---

/// Generate a random room by placing 2x2 templates on a grid of
/// `tw` x `th` template cells, producing a room of `(tw*2)` x `(th*2)` tiles.
/// Templates are: all-wall, all-floor, and various half-wall patterns.
/// Returns None if the room fails validation.
fn generate_room(tw: usize, th: usize, num_boxes: usize, rng: &mut impl Rng) -> Option<Grid> {
    let w = tw * 2;
    let h = th * 2;
    let mut grid = vec![vec![true; w]; h]; // true = wall

    // For each 2x2 template cell, randomly pick a pattern.
    // Border cells are always walls to ensure enclosure.
    for ty in 0..th {
        for tx in 0..tw {
            let x = tx * 2;
            let y = ty * 2;

            // Border templates are walls
            if tx == 0 || ty == 0 || tx == tw - 1 || ty == th - 1 {
                // All wall
                continue;
            }

            // Random template: bias toward floor to get interesting rooms
            let pattern = rng.random_range(0..10);
            let cells = match pattern {
                0..=1 => [[true, true], [true, true]],     // all wall
                2..=5 => [[false, false], [false, false]], // all floor
                6 => [[false, false], [true, true]],       // top floor
                7 => [[true, true], [false, false]],       // bottom floor
                8 => [[false, true], [false, true]],       // left floor
                _ => [[true, false], [true, false]],       // right floor
            };

            for dy in 0..2 {
                for dx in 0..2 {
                    grid[y + dy][x + dx] = cells[dy][dx];
                }
            }
        }
    }

    let width = w;
    let height = h;

    // Flood fill to find the largest connected floor region
    let mut visited = vec![vec![false; width]; height];
    let mut best_region: Vec<(usize, usize)> = Vec::new();

    for sy in 0..height {
        for sx in 0..width {
            if grid[sy][sx] || visited[sy][sx] {
                continue;
            }
            let mut region = Vec::new();
            let mut queue = VecDeque::new();
            queue.push_back((sx, sy));
            visited[sy][sx] = true;
            while let Some((x, y)) = queue.pop_front() {
                region.push((x, y));
                for &(dx, dy) in &DIRS {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height {
                        let (nx, ny) = (nx as usize, ny as usize);
                        if !grid[ny][nx] && !visited[ny][nx] {
                            visited[ny][nx] = true;
                            queue.push_back((nx, ny));
                        }
                    }
                }
            }
            if region.len() > best_region.len() {
                best_region = region;
            }
        }
    }

    // Fill non-largest-region floor cells with walls
    let region_set: HashSet<(usize, usize)> = best_region.iter().copied().collect();
    for (y, row) in grid.iter_mut().enumerate().take(height) {
        for (x, cell) in row.iter_mut().enumerate().take(width) {
            if !*cell && !region_set.contains(&(x, y)) {
                *cell = true;
            }
        }
    }

    let floor_count = best_region.len();

    // Need enough floor for boxes + player + at least one empty space
    if floor_count < num_boxes + 2 {
        return None;
    }

    // Reject rooms with 3-wall dead ends (floor surrounded on 3 sides by walls)
    for &(x, y) in &best_region {
        let wall_count = DIRS
            .iter()
            .filter(|&&(dx, dy)| {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                nx < 0
                    || ny < 0
                    || nx as usize >= width
                    || ny as usize >= height
                    || grid[ny as usize][nx as usize]
            })
            .count();
        if wall_count >= 3 {
            return None;
        }
    }

    // Reject rooms with any 4x3 or larger open rectangle (too bushy)
    for y in 0..height.saturating_sub(2) {
        for x in 0..width.saturating_sub(3) {
            let all_floor = (0..3).all(|dy| (0..4).all(|dx| !grid[y + dy][x + dx]));
            if all_floor {
                return None;
            }
        }
    }
    for y in 0..height.saturating_sub(3) {
        for x in 0..width.saturating_sub(2) {
            let all_floor = (0..4).all(|dy| (0..3).all(|dx| !grid[y + dy][x + dx]));
            if all_floor {
                return None;
            }
        }
    }

    // Trim the grid to remove outer wall rows/cols that are entirely walls
    let min_x = (0..width)
        .find(|&x| (0..height).any(|y| !grid[y][x]))
        .unwrap_or(0)
        .saturating_sub(1);
    let max_x = (0..width)
        .rev()
        .find(|&x| (0..height).any(|y| !grid[y][x]))
        .unwrap_or(width - 1)
        + 1;
    let min_y = (0..height)
        .find(|&y| (0..width).any(|x| !grid[y][x]))
        .unwrap_or(0)
        .saturating_sub(1);
    let max_y = (0..height)
        .rev()
        .find(|&y| (0..width).any(|x| !grid[y][x]))
        .unwrap_or(height - 1)
        + 1;

    let tw = (max_x - min_x).min(width) + 1;
    let th = (max_y - min_y).min(height) + 1;

    let mut trimmed = vec![vec![true; tw]; th];
    for (y, row) in trimmed.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let sx = min_x + x;
            let sy = min_y + y;
            if sx < width && sy < height {
                *cell = grid[sy][sx];
            }
        }
    }

    let mut floor: Vec<(usize, usize)> = Vec::new();
    for (y, row) in trimmed.iter().enumerate() {
        for (x, &cell) in row.iter().enumerate() {
            if !cell {
                floor.push((x, y));
            }
        }
    }

    if floor.len() < num_boxes + 2 {
        return None;
    }

    Some(Grid {
        cells: trimmed,
        width: tw,
        height: th,
        floor,
    })
}

struct Grid {
    cells: Vec<Vec<bool>>, // true = wall
    width: usize,
    height: usize,
    floor: Vec<(usize, usize)>,
}

impl Grid {
    fn is_wall(&self, x: usize, y: usize) -> bool {
        x >= self.width || y >= self.height || self.cells[y][x]
    }

    fn to_string(
        &self,
        goals: &[(usize, usize)],
        boxes: &[(usize, usize)],
        player: (usize, usize),
    ) -> String {
        let goal_set: HashSet<_> = goals.iter().copied().collect();
        let box_set: HashSet<_> = boxes.iter().copied().collect();
        let mut lines = Vec::new();
        for y in 0..self.height {
            let mut line = String::new();
            for x in 0..self.width {
                let is_goal = goal_set.contains(&(x, y));
                let is_box = box_set.contains(&(x, y));
                let is_player = (x, y) == player;
                let ch = if self.cells[y][x] {
                    '#'
                } else if is_player && is_goal {
                    '+'
                } else if is_player {
                    '@'
                } else if is_box && is_goal {
                    '*'
                } else if is_box {
                    '$'
                } else if is_goal {
                    '.'
                } else {
                    ' '
                };
                line.push(ch);
            }
            lines.push(line.trim_end().to_owned());
        }
        // Remove trailing empty lines
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

// --- State representation for reverse search ---

/// State: sorted box positions + player region id (normalized to top-left-most
/// reachable floor cell).
#[derive(Clone, Eq, PartialEq, Hash)]
struct State {
    boxes: Vec<(usize, usize)>,
    player_region: (usize, usize),
}

fn normalize_player(
    grid: &Grid,
    player: (usize, usize),
    boxes: &HashSet<(usize, usize)>,
) -> (usize, usize) {
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
            if nx >= 0 && ny >= 0 {
                let np = (nx as usize, ny as usize);
                if !grid.is_wall(np.0, np.1) && !boxes.contains(&np) && visited.insert(np) {
                    queue.push_back(np);
                }
            }
        }
    }
    best
}

fn make_state(grid: &Grid, boxes: &[(usize, usize)], player: (usize, usize)) -> State {
    let mut sorted = boxes.to_vec();
    sorted.sort();
    let box_set: HashSet<_> = sorted.iter().copied().collect();
    let pr = normalize_player(grid, player, &box_set);
    State {
        boxes: sorted,
        player_region: pr,
    }
}

/// Expand a set of states by one "box-line" step in reverse.
/// A box-line in reverse: pull a box (move it toward the player) along a line
/// until it can't continue, counting the whole line as one step.
///
/// In reverse, a "pull" means: player is adjacent to box, player moves away
/// pulling the box with them. The box follows the player one step at a time.
/// A box-line = pulling the same box in the same direction any number of times.
fn expand_box_lines(grid: &Grid, states: &HashSet<State>) -> HashSet<State> {
    let mut new_states = HashSet::new();

    for state in states {
        let box_set: HashSet<_> = state.boxes.iter().copied().collect();

        // Find all cells the player can reach
        let reachable = {
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            visited.insert(state.player_region);
            queue.push_back(state.player_region);
            while let Some((x, y)) = queue.pop_front() {
                for &(dx, dy) in &DIRS {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 {
                        let np = (nx as usize, ny as usize);
                        if !grid.is_wall(np.0, np.1) && !box_set.contains(&np) && visited.insert(np)
                        {
                            queue.push_back(np);
                        }
                    }
                }
            }
            visited
        };

        // For each box, try pulling it in each direction (reverse of push).
        // A pull: player is at (bx+dx, by+dy), pulls box from (bx, by) to
        // (bx+dx, by+dy), player moves to (bx+2dx, by+2dy).
        // In a box-line, we keep pulling the same box in the same direction.
        for (bi, &(bx, by)) in state.boxes.iter().enumerate() {
            for &(dx, dy) in &DIRS {
                // Player must be adjacent to box on the pull side
                let px = bx as i32 + dx;
                let py = by as i32 + dy;
                if px < 0 || py < 0 {
                    continue;
                }
                let player_pos = (px as usize, py as usize);
                if grid.is_wall(player_pos.0, player_pos.1) || box_set.contains(&player_pos) {
                    continue;
                }

                // Player must be able to reach the pull position
                if !reachable.contains(&player_pos) {
                    continue;
                }

                // Pull the box along this direction as far as possible
                let mut cur_box = (bx, by);
                let mut cur_player = player_pos;

                loop {
                    // New box position = old player position
                    let new_box = cur_player;
                    // New player position = one step further in pull direction
                    let new_px = cur_player.0 as i32 + dx;
                    let new_py = cur_player.1 as i32 + dy;

                    if new_px < 0 || new_py < 0 {
                        break;
                    }
                    let new_player = (new_px as usize, new_py as usize);

                    // Check new player position is free
                    if grid.is_wall(new_player.0, new_player.1) {
                        break;
                    }

                    // Check new player position doesn't have a box
                    // (the box at cur_box is being moved, so exclude it)
                    if box_set.contains(&new_player) && new_player != cur_box {
                        break;
                    }

                    // Check new box position doesn't collide with another box
                    if box_set.contains(&new_box) && new_box != cur_box {
                        break;
                    }

                    cur_box = new_box;
                    cur_player = new_player;

                    // Create new state after this pull step
                    let mut new_boxes = state.boxes.clone();
                    new_boxes[bi] = cur_box;
                    let ns = make_state(grid, &new_boxes, cur_player);
                    new_states.insert(ns);
                }
            }
        }
    }

    new_states
}

/// Find the farthest states from the goal using BFS in reverse
/// with the box-lines metric.
///
/// Returns (depth, set of farthest states).
fn find_farthest(
    grid: &Grid,
    goal_boxes: &[(usize, usize)],
    time_limit: std::time::Duration,
) -> Option<(u32, Vec<State>)> {
    let start_time = Instant::now();

    // MakeStartSet: place player in each reachable region of the goal state
    let box_set: HashSet<_> = goal_boxes.iter().copied().collect();
    let mut start_states = HashSet::new();
    let mut visited_cells = HashSet::new();

    for &(fx, fy) in &grid.floor {
        if box_set.contains(&(fx, fy)) || visited_cells.contains(&(fx, fy)) {
            continue;
        }

        // BFS to find connected region
        let mut region = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back((fx, fy));
        visited_cells.insert((fx, fy));
        while let Some((x, y)) = queue.pop_front() {
            region.push((x, y));
            for &(dx, dy) in &DIRS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 {
                    let np = (nx as usize, ny as usize);
                    if !grid.is_wall(np.0, np.1)
                        && !box_set.contains(&np)
                        && visited_cells.insert(np)
                    {
                        queue.push_back(np);
                    }
                }
            }
        }

        if !region.is_empty() {
            let best = region.iter().copied().min_by_key(|&(x, y)| (y, x)).unwrap();
            let s = make_state(grid, goal_boxes, best);
            start_states.insert(s);
        }
    }

    if start_states.is_empty() {
        return None;
    }

    // BFS: expand level by level using box-lines metric
    let mut visited: HashSet<State> = start_states.clone();
    let mut current_frontier: HashSet<State> = start_states;
    let mut best_depth = 0u32;
    let mut best_frontier = current_frontier.iter().cloned().collect::<Vec<_>>();

    let mut depth = 0u32;
    loop {
        if start_time.elapsed() > time_limit {
            break;
        }

        let expanded = expand_box_lines(grid, &current_frontier);
        let new_states: HashSet<State> = expanded
            .into_iter()
            .filter(|s| visited.insert(s.clone()))
            .collect();

        if new_states.is_empty() {
            break;
        }

        depth += 1;
        best_depth = depth;
        best_frontier = new_states.iter().cloned().collect();
        current_frontier = new_states;
    }

    if best_depth == 0 {
        return None;
    }

    Some((best_depth, best_frontier))
}

/// Generate a sokoban level.
fn generate_level(
    template_w: usize,
    template_h: usize,
    num_boxes: usize,
    time_limit_secs: u64,
) -> Option<String> {
    let mut rng = rand::rng();
    let time_limit = std::time::Duration::from_secs(time_limit_secs);
    let start = Instant::now();

    let mut best_level: Option<String> = None;
    let mut best_depth = 0u32;
    let mut attempts = 0u32;

    while start.elapsed() < time_limit {
        attempts += 1;

        // Phase 1: Generate room
        let grid = match generate_room(template_w, template_h, num_boxes, &mut rng) {
            Some(g) => g,
            None => continue,
        };

        // Phase 2: Try goal placements
        // For small numbers of boxes, try random combinations of goal positions
        let remaining_time = time_limit.saturating_sub(start.elapsed());
        let per_room_limit =
            remaining_time.min(std::time::Duration::from_secs((time_limit_secs / 5).max(2)));
        let room_start = Instant::now();

        // Generate random goal placements and evaluate each
        let mut goal_attempts = 0;
        while room_start.elapsed() < per_room_limit && start.elapsed() < time_limit {
            goal_attempts += 1;
            if goal_attempts > 500 {
                break;
            }

            // Pick random floor positions for goals
            let mut goals: Vec<(usize, usize)> = Vec::new();
            let mut used = HashSet::new();
            let mut valid = true;
            for _ in 0..num_boxes {
                let mut found = false;
                for _ in 0..100 {
                    let idx = rng.random_range(0..grid.floor.len());
                    let pos = grid.floor[idx];
                    if !used.contains(&pos) {
                        used.insert(pos);
                        goals.push(pos);
                        found = true;
                        break;
                    }
                }
                if !found {
                    valid = false;
                    break;
                }
            }
            if !valid {
                continue;
            }

            // Phase 3: Find farthest state
            let search_limit = per_room_limit
                .saturating_sub(room_start.elapsed())
                .min(std::time::Duration::from_secs(2));
            if let Some((depth, states)) = find_farthest(&grid, &goals, search_limit) {
                if depth > best_depth && !states.is_empty() {
                    // Pick a random farthest state
                    let idx = rng.random_range(0..states.len());
                    let state = &states[idx];
                    best_depth = depth;
                    best_level = Some(grid.to_string(&goals, &state.boxes, state.player_region));
                }
            }
        }
    }

    eprintln!(
        "Attempted {} rooms, best depth: {} box-lines",
        attempts, best_depth
    );
    best_level
}

#[derive(Parser)]
struct Args {
    /// Template grid width (room width = 2 * this)
    #[clap(long, default_value = "5")]
    width: usize,

    /// Template grid height (room height = 2 * this)
    #[clap(long, default_value = "5")]
    height: usize,

    /// Number of boxes
    #[clap(long, default_value = "3")]
    boxes: usize,

    /// Time limit in seconds
    #[clap(long, default_value = "30")]
    time_limit: u64,
}

fn main() {
    let args = Args::parse();

    eprintln!(
        "Generating {}-box sokoban level ({}x{} template, {}x{} room)...",
        args.boxes,
        args.width,
        args.height,
        args.width * 2,
        args.height * 2,
    );
    eprintln!("Time limit: {}s", args.time_limit);

    match generate_level(args.width, args.height, args.boxes, args.time_limit) {
        Some(level) => {
            println!("{}", level);
        }
        None => {
            eprintln!("Failed to generate a level within the time limit.");
            std::process::exit(1);
        }
    }
}
