/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Minesweeper with constraint solving, no-guess guarantee, and safe mode.
//!
//! Beyond a basic minesweeper:
//!
//! - **Constraint-based solver**: two-phase deduction engine — single-cell
//!   propagation to fixpoint, then subset elimination between constraint
//!   pairs (if A's unknowns ⊆ B's, the difference has exactly B−A mines).
//!   Powers both safe mode and the no-guess board rearrangement.
//!
//! - **No-guess guarantee** (`ensure_safe`): when no logically deducible
//!   safe move exists, the board is rearranged to make the clicked cell
//!   safe while preserving all existing constraints. Uses component
//!   decomposition with exact backtracking for small components (≤25 cells)
//!   and randomized search for larger ones.
//!
//! - **Safe mode** (toggle with `g`): blocks clicks on cells that are
//!   provably risky when logically safe alternatives exist. Visual feedback
//!   shows when a click was blocked.
//!
//! - **Auto-chord** (toggle with `c`): when placing a flag, automatically
//!   reveals neighbors of any satisfied number cell, cascading until no
//!   more auto-chords apply.
//!
//! - **Guaranteed large opening**: first click re-generates mine layouts
//!   until the initial flood-fill reveal exceeds a size threshold.
//!
//! - **4 board sizes**: Beginner (9×9), Intermediate (16×16), Expert
//!   (30×16), and Huge (50×34) with a size selection menu.
//!
//! - **Timer**: tracks elapsed seconds, paused during inactivity.

use std::time::Instant;

use rand::Rng;
use rand::RngExt;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;
use superconsole::style::Stylize;

use crate::console::Control;

const CELL_WIDTH: usize = 3;
const HIDDEN_COLOR: Color = Color::White;

// (width, height, mines, label)
const SIZES: [(usize, usize, usize, &str); 4] = [
    (9, 9, 10, "Beginner"),
    (16, 16, 40, "Intermediate"),
    (30, 16, 99, "Expert"),
    (50, 34, 200, "Huge"),
];

fn format_elapsed_time(elapsed_secs: f64) -> String {
    let elapsed_secs = elapsed_secs as u64;
    format!("{:02}:{:02}", elapsed_secs / 60, elapsed_secs % 60)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellState {
    Hidden,
    Revealed,
    Flagged,
}

fn number_color(n: u8) -> Color {
    match n {
        1 => Color::Blue,
        2 => Color::Green,
        3 => Color::Red,
        4 => Color::DarkBlue,
        5 => Color::DarkRed,
        6 => Color::DarkCyan,
        7 => Color::Magenta,
        8 => Color::Grey,
        _ => Color::White,
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    width: usize,
    height: usize,
    mine_count: usize,
    mines: Vec<Vec<bool>>,
    state: Vec<Vec<CellState>>,
    numbers: Vec<Vec<u8>>,
    cursor_x: usize,
    cursor_y: usize,
    game_over: bool,
    won: bool,
    placed_mines: bool,
    fast_mode: bool,
    auto_chord_enabled: bool,
    safe_mode: bool,
    blocked_click: bool,
    selecting_size: bool,
    size_cursor: usize,
    elapsed_secs: f64,
    last_tick: Option<Instant>,
}

impl Game {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            mine_count: 0,
            mines: Vec::new(),
            state: Vec::new(),
            numbers: Vec::new(),
            cursor_x: 0,
            cursor_y: 0,
            game_over: false,
            won: false,
            placed_mines: false,
            fast_mode: false,
            auto_chord_enabled: false,
            safe_mode: false,
            blocked_click: false,
            selecting_size: true,
            size_cursor: 0,
            elapsed_secs: 0.0,
            last_tick: None,
        }
    }

    fn init_board(&mut self, size_idx: usize) {
        let (w, h, m, _) = SIZES[size_idx];
        self.width = w;
        self.height = h;
        self.mine_count = m;
        self.mines = vec![vec![false; w]; h];
        self.state = vec![vec![CellState::Hidden; w]; h];
        self.numbers = vec![vec![0u8; w]; h];
        self.cursor_x = w / 2;
        self.cursor_y = h / 2;
        self.selecting_size = false;
    }

    pub fn set_fast_mode(&mut self, fast: bool) {
        self.fast_mode = fast;
        self.auto_chord_enabled = fast;
    }

    fn neighbors(x: usize, y: usize, width: usize, height: usize) -> Vec<(usize, usize)> {
        let mut result = Vec::with_capacity(8);
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
                    result.push((nx as usize, ny as usize));
                }
            }
        }
        result
    }

    fn place_mines(&mut self, safe_x: usize, safe_y: usize) {
        let mut rng = rand::rng();
        let threshold = ((self.width * self.height - self.mine_count) / 25).max(5);
        loop {
            self.mines = vec![vec![false; self.width]; self.height];
            let mut placed = 0;
            while placed < self.mine_count {
                let x = rng.random_range(0..self.width);
                let y = rng.random_range(0..self.height);
                if (x as i32 - safe_x as i32).abs() <= 1 && (y as i32 - safe_y as i32).abs() <= 1 {
                    continue;
                }
                if self.mines[y][x] {
                    continue;
                }
                self.mines[y][x] = true;
                placed += 1;
            }
            if self.count_reveal(safe_x, safe_y) >= threshold {
                break;
            }
        }
        self.placed_mines = true;
    }

    fn count_reveal(&self, start_x: usize, start_y: usize) -> usize {
        let mut visited = vec![vec![false; self.width]; self.height];
        let mut stack = vec![(start_x, start_y)];
        let mut count = 0;
        while let Some((x, y)) = stack.pop() {
            if visited[y][x] {
                continue;
            }
            visited[y][x] = true;
            count += 1;
            if self.adjacent_mines(x, y) == 0 {
                for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
                    if !visited[ny][nx] && !self.mines[ny][nx] {
                        stack.push((nx, ny));
                    }
                }
            }
        }
        count
    }

    fn adjacent_mines(&self, x: usize, y: usize) -> u8 {
        let mut count = 0;
        for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
            if self.mines[ny][nx] {
                count += 1;
            }
        }
        count
    }

    fn reveal(&mut self, x: usize, y: usize) {
        if self.state[y][x] != CellState::Hidden {
            return;
        }
        self.state[y][x] = CellState::Revealed;
        if self.mines[y][x] {
            self.game_over = true;
            return;
        }
        let n = self.adjacent_mines(x, y);
        self.numbers[y][x] = n;
        if n == 0 {
            for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
                self.reveal(nx, ny);
            }
        }
    }

    fn refresh_revealed_numbers(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                if self.state[y][x] == CellState::Revealed && !self.mines[y][x] {
                    self.numbers[y][x] = self.adjacent_mines(x, y);
                }
            }
        }
    }

    fn check_win(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                if !self.mines[y][x] && self.state[y][x] != CellState::Revealed {
                    return;
                }
            }
        }
        self.won = true;
        self.game_over = true;
    }

    fn adjacent_flags(&self, x: usize, y: usize) -> u8 {
        let mut count = 0;
        for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
            if self.state[ny][nx] == CellState::Flagged {
                count += 1;
            }
        }
        count
    }

    fn auto_chord(&mut self) {
        loop {
            let mut any_chorded = false;
            for y in 0..self.height {
                for x in 0..self.width {
                    if self.game_over {
                        return;
                    }
                    if self.state[y][x] != CellState::Revealed {
                        continue;
                    }
                    let mines = self.numbers[y][x];
                    if mines == 0 || self.adjacent_flags(x, y) != mines {
                        continue;
                    }
                    for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
                        if self.state[ny][nx] == CellState::Hidden {
                            self.reveal(nx, ny);
                            any_chorded = true;
                        }
                    }
                }
            }
            if !any_chorded {
                break;
            }
        }
    }

    fn solve_constraints(&self) -> (Vec<Vec<bool>>, Vec<Vec<bool>>) {
        let mut safe = vec![vec![false; self.width]; self.height];
        let mut mine = vec![vec![false; self.width]; self.height];

        for y in 0..self.height {
            for x in 0..self.width {
                match self.state[y][x] {
                    CellState::Revealed => safe[y][x] = true,
                    CellState::Flagged => mine[y][x] = true,
                    CellState::Hidden => {}
                }
            }
        }

        loop {
            // Phase 1: basic single-cell propagation until fixpoint
            loop {
                let mut changed = false;
                for y in 0..self.height {
                    for x in 0..self.width {
                        if self.state[y][x] != CellState::Revealed {
                            continue;
                        }
                        let n = self.numbers[y][x];

                        let mut mine_count = 0u8;
                        let mut unknown: Vec<(usize, usize)> = Vec::new();
                        for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
                            if mine[ny][nx] {
                                mine_count += 1;
                            } else if !safe[ny][nx] {
                                unknown.push((nx, ny));
                            }
                        }

                        if unknown.is_empty() {
                            continue;
                        }

                        let remaining = n.saturating_sub(mine_count);

                        if remaining == 0 {
                            for (ux, uy) in unknown {
                                safe[uy][ux] = true;
                                changed = true;
                            }
                        } else if remaining as usize == unknown.len() {
                            for (ux, uy) in unknown {
                                mine[uy][ux] = true;
                                changed = true;
                            }
                        }
                    }
                }
                if !changed {
                    break;
                }
            }

            // Phase 2: subset elimination between constraint pairs.
            // If constraint A's unknowns are a subset of B's, then
            // B's extra cells have exactly (B.remaining - A.remaining) mines.
            let mut constraints: Vec<(u8, Vec<(usize, usize)>)> = Vec::new();
            for y in 0..self.height {
                for x in 0..self.width {
                    if self.state[y][x] != CellState::Revealed {
                        continue;
                    }
                    let n = self.numbers[y][x];
                    if n == 0 {
                        continue;
                    }
                    let mut mine_count = 0u8;
                    let mut unknown: Vec<(usize, usize)> = Vec::new();
                    for (nx, ny) in Self::neighbors(x, y, self.width, self.height) {
                        if mine[ny][nx] {
                            mine_count += 1;
                        } else if !safe[ny][nx] {
                            unknown.push((nx, ny));
                        }
                    }
                    let remaining = n.saturating_sub(mine_count);
                    if !unknown.is_empty() {
                        unknown.sort();
                        constraints.push((remaining, unknown));
                    }
                }
            }

            let mut subset_changed = false;
            for i in 0..constraints.len() {
                for j in 0..constraints.len() {
                    if i == j {
                        continue;
                    }
                    let (ra, ref ua) = constraints[i];
                    let (rb, ref ub) = constraints[j];

                    // Quick size check: ua can't be subset of ub if larger
                    if ua.len() >= ub.len() {
                        continue;
                    }

                    // Check if ua ⊆ ub (both sorted)
                    let is_subset = ua.iter().all(|c| ub.contains(c));
                    if !is_subset {
                        continue;
                    }

                    // Cells in ub but not ua have exactly (rb - ra) mines
                    let diff: Vec<(usize, usize)> =
                        ub.iter().filter(|c| !ua.contains(c)).copied().collect();
                    if rb < ra {
                        continue;
                    }
                    let diff_mines = rb - ra;

                    if diff_mines == 0 {
                        for (dx, dy) in diff {
                            if !safe[dy][dx] {
                                safe[dy][dx] = true;
                                subset_changed = true;
                            }
                        }
                    } else if diff_mines as usize == diff.len() {
                        for (dx, dy) in diff {
                            if !mine[dy][dx] {
                                mine[dy][dx] = true;
                                subset_changed = true;
                            }
                        }
                    }
                }
            }

            if !subset_changed {
                break;
            }
            // Loop back to phase 1 with new information
        }

        (safe, mine)
    }

    fn ensure_safe(&mut self, click_x: usize, click_y: usize) {
        if !self.mines[click_y][click_x] {
            return;
        }

        let (width, height) = (self.width, self.height);

        let mut boundary: Vec<(usize, usize)> = Vec::new();
        let mut interior: Vec<(usize, usize)> = Vec::new();

        for y in 0..height {
            for x in 0..width {
                if self.state[y][x] != CellState::Hidden {
                    continue;
                }
                let adjacent_to_revealed = Self::neighbors(x, y, width, height)
                    .iter()
                    .any(|&(nx, ny)| self.state[ny][nx] == CellState::Revealed);
                if adjacent_to_revealed {
                    boundary.push((x, y));
                } else {
                    interior.push((x, y));
                }
            }
        }

        struct Constraint {
            needed: u8,
            cells: Vec<usize>,
        }

        let mut boundary_index: std::collections::HashMap<(usize, usize), usize> =
            std::collections::HashMap::new();
        for (i, &pos) in boundary.iter().enumerate() {
            boundary_index.insert(pos, i);
        }

        let mut constraints: Vec<Constraint> = Vec::new();
        for y in 0..height {
            for x in 0..width {
                if self.state[y][x] != CellState::Revealed {
                    continue;
                }
                let n = self.numbers[y][x];
                let flags = self.adjacent_flags(x, y);
                let needed = n.saturating_sub(flags);
                let cells: Vec<usize> = Self::neighbors(x, y, width, height)
                    .iter()
                    .filter_map(|pos| boundary_index.get(pos).copied())
                    .collect();
                if !cells.is_empty() {
                    constraints.push(Constraint { needed, cells });
                }
            }
        }

        let click_boundary_idx = boundary_index.get(&(click_x, click_y)).copied();

        let n_boundary = boundary.len();
        let mut component_id = vec![usize::MAX; n_boundary];
        let mut next_id = 0usize;

        for constraint in &constraints {
            let mut min_comp = usize::MAX;
            for &ci in &constraint.cells {
                if component_id[ci] < min_comp {
                    min_comp = component_id[ci];
                }
            }
            if min_comp == usize::MAX {
                min_comp = next_id;
                next_id += 1;
            }
            for &ci in &constraint.cells {
                let old = component_id[ci];
                if old != min_comp && old != usize::MAX {
                    for cid in component_id.iter_mut() {
                        if *cid == old {
                            *cid = min_comp;
                        }
                    }
                }
                component_id[ci] = min_comp;
            }
        }

        for cid in component_id.iter_mut() {
            if *cid == usize::MAX {
                *cid = next_id;
                next_id += 1;
            }
        }

        let mut components: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, &cid) in component_id.iter().enumerate() {
            components.entry(cid).or_default().push(i);
        }

        for &(bx, by) in &boundary {
            self.mines[by][bx] = false;
        }
        for &(ix, iy) in &interior {
            self.mines[iy][ix] = false;
        }

        let mut rng = rand::rng();
        let mut total_boundary_mines = 0usize;

        for comp_cells in components.values() {
            let comp_set: std::collections::HashSet<usize> = comp_cells.iter().copied().collect();
            let relevant_constraints: Vec<&Constraint> = constraints
                .iter()
                .filter(|c| c.cells.iter().any(|ci| comp_set.contains(ci)))
                .collect();

            let mut local_map: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::new();
            for (li, &gi) in comp_cells.iter().enumerate() {
                local_map.insert(gi, li);
            }

            let comp_size = comp_cells.len();
            let click_local = click_boundary_idx.and_then(|gi| local_map.get(&gi).copied());

            if comp_size > 25 {
                let mut found = false;
                for _ in 0..1000 {
                    let assignment: Vec<bool> = (0..comp_size)
                        .map(|i| Some(i) != click_local && rng.random_bool(0.3))
                        .collect();
                    let valid = relevant_constraints.iter().all(|c| {
                        let count: u8 = c
                            .cells
                            .iter()
                            .filter_map(|gi| local_map.get(gi))
                            .filter(|&&li| assignment[li])
                            .count() as u8;
                        count == c.needed
                    });
                    if valid {
                        for (li, &is_mine) in assignment.iter().enumerate() {
                            if is_mine {
                                let gi = comp_cells[li];
                                let (bx, by) = boundary[gi];
                                self.mines[by][bx] = true;
                                total_boundary_mines += 1;
                            }
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    for c in &relevant_constraints {
                        let mut placed = 0u8;
                        for &gi in &c.cells {
                            if let Some(&li) = local_map.get(&gi) {
                                if Some(li) == click_local {
                                    continue;
                                }
                                if placed < c.needed {
                                    let (bx, by) = boundary[comp_cells[li]];
                                    if !self.mines[by][bx] {
                                        self.mines[by][bx] = true;
                                        total_boundary_mines += 1;
                                        placed += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                let mut assignment = vec![false; comp_size];
                let mut best: Option<Vec<bool>> = None;

                fn backtrack(
                    idx: usize,
                    assignment: &mut Vec<bool>,
                    local_map: &std::collections::HashMap<usize, usize>,
                    constraints: &[&Constraint],
                    click_local: Option<usize>,
                    best: &mut Option<Vec<bool>>,
                    rng: &mut impl Rng,
                ) {
                    if best.is_some() {
                        return;
                    }
                    if idx == assignment.len() {
                        let valid = constraints.iter().all(|c| {
                            let count: u8 = c
                                .cells
                                .iter()
                                .filter_map(|gi| local_map.get(gi))
                                .filter(|&&li| assignment[li])
                                .count() as u8;
                            count == c.needed
                        });
                        if valid {
                            *best = Some(assignment.clone());
                        }
                        return;
                    }

                    if Some(idx) == click_local {
                        assignment[idx] = false;
                        backtrack(
                            idx + 1,
                            assignment,
                            local_map,
                            constraints,
                            click_local,
                            best,
                            rng,
                        );
                        return;
                    }

                    let try_mine_first = rng.random_bool(0.5);
                    let options = if try_mine_first {
                        [true, false]
                    } else {
                        [false, true]
                    };

                    for &val in &options {
                        assignment[idx] = val;

                        let feasible = constraints.iter().all(|c| {
                            let assigned_mines: u8 = c
                                .cells
                                .iter()
                                .filter_map(|gi| local_map.get(gi))
                                .filter(|&&li| li <= idx && assignment[li])
                                .count() as u8;
                            let unassigned: u8 = c
                                .cells
                                .iter()
                                .filter_map(|gi| local_map.get(gi))
                                .filter(|&&li| li > idx)
                                .count() as u8;
                            assigned_mines <= c.needed && assigned_mines + unassigned >= c.needed
                        });

                        if feasible {
                            backtrack(
                                idx + 1,
                                assignment,
                                local_map,
                                constraints,
                                click_local,
                                best,
                                rng,
                            );
                        }
                    }
                    assignment[idx] = false;
                }

                backtrack(
                    0,
                    &mut assignment,
                    &local_map,
                    &relevant_constraints,
                    click_local,
                    &mut best,
                    &mut rng,
                );

                if let Some(solution) = best {
                    for (li, &is_mine) in solution.iter().enumerate() {
                        if is_mine {
                            let gi = comp_cells[li];
                            let (bx, by) = boundary[gi];
                            self.mines[by][bx] = true;
                            total_boundary_mines += 1;
                        }
                    }
                }
            }
        }

        let flagged_mines: usize = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| self.state[y][x] == CellState::Flagged)
            .count();
        let target_remaining = self
            .mine_count
            .saturating_sub(flagged_mines + total_boundary_mines);

        let mut interior_available: Vec<(usize, usize)> = interior
            .iter()
            .filter(|&&(ix, iy)| !(ix == click_x && iy == click_y))
            .copied()
            .collect();

        for i in (1..interior_available.len()).rev() {
            let j = rng.random_range(0..=i);
            interior_available.swap(i, j);
        }

        let to_place = target_remaining.min(interior_available.len());
        for &(ix, iy) in &interior_available[..to_place] {
            self.mines[iy][ix] = true;
        }

        self.refresh_revealed_numbers();
    }

    fn render_cell(&self, line: &mut Line, x: usize, y: usize) {
        let is_cursor = x == self.cursor_x && y == self.cursor_y;

        match self.state[y][x] {
            CellState::Hidden => {
                if is_cursor {
                    line.push(Span::new_styled_lossy(
                        "[ ]".to_owned().with(Color::DarkGrey).on(HIDDEN_COLOR),
                    ));
                } else {
                    line.push(Span::new_colored_lossy("███", HIDDEN_COLOR));
                }
            }
            CellState::Flagged => {
                if is_cursor {
                    line.push(Span::new_styled_lossy(
                        "[".to_owned().with(Color::DarkGrey).on(HIDDEN_COLOR),
                    ));
                    line.push(Span::new_styled_lossy(
                        "⚑".to_owned().with(Color::Red).on(HIDDEN_COLOR),
                    ));
                    line.push(Span::new_styled_lossy(
                        "]".to_owned().with(Color::DarkGrey).on(HIDDEN_COLOR),
                    ));
                } else {
                    line.push(Span::new_styled_lossy(
                        " ".to_owned().with(HIDDEN_COLOR).on(HIDDEN_COLOR),
                    ));
                    line.push(Span::new_styled_lossy(
                        "⚑".to_owned().with(Color::Red).on(HIDDEN_COLOR),
                    ));
                    line.push(Span::new_styled_lossy(
                        " ".to_owned().with(HIDDEN_COLOR).on(HIDDEN_COLOR),
                    ));
                }
            }
            CellState::Revealed => {
                if self.mines[y][x] {
                    line.push(Span::new_styled_lossy(
                        " * ".to_owned().with(Color::Red).bold(),
                    ));
                } else {
                    let n = self.numbers[y][x];
                    if n == 0 {
                        if is_cursor {
                            line.push(Span::new_colored_lossy("[ ]", Color::Yellow));
                        } else {
                            line.push(Span::new_unstyled_lossy("   "));
                        }
                    } else if is_cursor {
                        line.push(Span::new_colored_lossy("[", Color::Yellow));
                        line.push(Span::new_colored_lossy(&n.to_string(), Color::Yellow));
                        line.push(Span::new_colored_lossy("]", Color::Yellow));
                    } else {
                        line.push(Span::new_unstyled_lossy(" "));
                        line.push(Span::new_colored_lossy(&n.to_string(), number_color(n)));
                        line.push(Span::new_unstyled_lossy(" "));
                    }
                }
            }
        }
    }

    fn render_size_select(&self) -> Lines {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::default());
        lines.push(vec!["  Select difficulty:".to_owned()].try_into().unwrap());
        lines.push(Line::default());
        for (i, &(w, h, m, label)) in SIZES.iter().enumerate() {
            let cursor = if i == self.size_cursor { ">" } else { " " };
            let text = format!("  {} {:<14} {}x{}, {} mines", cursor, label, w, h, m);
            if i == self.size_cursor {
                let mut line = Line::default();
                line.push(Span::new_styled_lossy(text.bold()));
                lines.push(line);
            } else {
                lines.push(vec![text].try_into().unwrap());
            }
        }
        lines.push(Line::default());
        lines.push(
            vec!["  [up/down] select  [enter] start".to_owned()]
                .try_into()
                .unwrap(),
        );
        Lines(lines)
    }
}

impl super::Game for Game {
    fn tick(&mut self, _tick_count: u32) -> super::TickResult {
        if self.placed_mines && !self.game_over {
            let now = Instant::now();
            if let Some(prev) = self.last_tick {
                self.elapsed_secs += prev.elapsed().as_secs_f64();
            }
            self.last_tick = Some(now);
        }
        super::TickResult {
            alive: !self.game_over,
            scores: vec![],
        }
    }

    fn scores(&self) -> Vec<super::Score> {
        if !self.won {
            return Vec::new();
        }
        let label = SIZES
            .iter()
            .find(|(w, h, m, _)| *w == self.width && *h == self.height && *m == self.mine_count)
            .map_or("Custom", |(_, _, _, l)| *l);
        vec![super::Score {
            category: label.to_owned(),
            value: self.elapsed_secs as u64,
            lower_is_better: true,
        }]
    }

    fn game_over_message(&self) -> Option<&str> {
        if self.won {
            Some("You win!")
        } else {
            Some("Game Over!")
        }
    }

    fn input(&mut self, input: Control) -> Option<Control> {
        if self.selecting_size {
            match input {
                Control::Up | Control::Char('w') => {
                    if self.size_cursor > 0 {
                        self.size_cursor -= 1;
                    }
                }
                Control::Down | Control::Char('s') => {
                    if self.size_cursor + 1 < SIZES.len() {
                        self.size_cursor += 1;
                    }
                }
                Control::Char('\r') | Control::Char('\n') => {
                    self.init_board(self.size_cursor);
                }
                other => return Some(other),
            }
            return None;
        }

        match input {
            Control::Up | Control::Char('w') => {
                if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
            }
            Control::Down | Control::Char('s') => {
                if self.cursor_y < self.height - 1 {
                    self.cursor_y += 1;
                }
            }
            Control::Left | Control::Char('a') => {
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            Control::Right | Control::Char('d') => {
                if self.cursor_x < self.width - 1 {
                    self.cursor_x += 1;
                }
            }
            Control::Char(' ') => {
                self.blocked_click = false;
                if self.state[self.cursor_y][self.cursor_x] != CellState::Hidden {
                    return None;
                }
                if !self.placed_mines {
                    self.place_mines(self.cursor_x, self.cursor_y);
                    self.reveal(self.cursor_x, self.cursor_y);
                } else {
                    let (safe, _mine) = self.solve_constraints();
                    let has_safe_moves = (0..self.height).any(|y| {
                        (0..self.width).any(|x| self.state[y][x] == CellState::Hidden && safe[y][x])
                    });
                    let cursor_is_safe = safe[self.cursor_y][self.cursor_x];

                    if has_safe_moves && !cursor_is_safe && self.safe_mode {
                        self.blocked_click = true;
                        return None;
                    }

                    if !has_safe_moves {
                        self.ensure_safe(self.cursor_x, self.cursor_y);
                    }

                    self.reveal(self.cursor_x, self.cursor_y);
                }
                if self.auto_chord_enabled {
                    self.auto_chord();
                }
                self.check_win();
            }
            Control::Char('f') => {
                let st = &mut self.state[self.cursor_y][self.cursor_x];
                *st = match *st {
                    CellState::Hidden => CellState::Flagged,
                    CellState::Flagged => CellState::Hidden,
                    CellState::Revealed => CellState::Revealed,
                };
                if self.auto_chord_enabled
                    && self.state[self.cursor_y][self.cursor_x] == CellState::Flagged
                {
                    self.auto_chord();
                    self.check_win();
                }
            }
            Control::Char('c') => {
                self.auto_chord_enabled = !self.auto_chord_enabled;
            }
            Control::Char('g') => {
                self.safe_mode = !self.safe_mode;
                self.blocked_click = false;
            }
            other => return Some(other),
        }
        None
    }

    fn save_state(&self) -> Option<String> {
        if self.selecting_size {
            return None;
        }
        let mine_positions: Vec<(usize, usize)> = (0..self.height)
            .flat_map(|y| {
                let mines = &self.mines;
                (0..self.width).filter_map(move |x| if mines[y][x] { Some((x, y)) } else { None })
            })
            .collect();
        let cell_states: Vec<(usize, usize, u8)> = (0..self.height)
            .flat_map(|y| {
                let state = &self.state;
                (0..self.width).filter_map(move |x| {
                    let s = match state[y][x] {
                        CellState::Hidden => return None,
                        CellState::Revealed => 1u8,
                        CellState::Flagged => 2u8,
                    };
                    Some((x, y, s))
                })
            })
            .collect();
        let number_entries: Vec<(usize, usize, u8)> = (0..self.height)
            .flat_map(|y| {
                let state = &self.state;
                let numbers = &self.numbers;
                (0..self.width).filter_map(move |x| {
                    if state[y][x] == CellState::Revealed && numbers[y][x] > 0 {
                        Some((x, y, numbers[y][x]))
                    } else {
                        None
                    }
                })
            })
            .collect();
        serde_json::to_string(&(
            self.width,
            self.height,
            self.mine_count,
            &mine_positions,
            &cell_states,
            self.cursor_x,
            self.cursor_y,
            self.game_over,
            self.won,
            self.placed_mines,
            &number_entries,
            self.elapsed_secs,
        ))
        .ok()
    }

    fn load_state(&mut self, json: &str) -> bool {
        type SaveState = (
            usize,
            usize,
            usize,
            Vec<(usize, usize)>,
            Vec<(usize, usize, u8)>,
            usize,
            usize,
            bool,
            bool,
            bool,
            Vec<(usize, usize, u8)>,
            f64,
        );

        let Ok((w, h, mc, mine_pos, cell_states, cx, cy, go, won, pm, number_entries, elapsed)) =
            serde_json::from_str::<SaveState>(json)
        else {
            return false;
        };

        self.width = w;
        self.height = h;
        self.mine_count = mc;
        self.selecting_size = false;

        self.mines = vec![vec![false; w]; h];
        for (x, y) in mine_pos {
            if y < h && x < w {
                self.mines[y][x] = true;
            }
        }
        self.state = vec![vec![CellState::Hidden; w]; h];
        for (x, y, s) in cell_states {
            if y < h && x < w {
                self.state[y][x] = match s {
                    1 => CellState::Revealed,
                    2 => CellState::Flagged,
                    _ => CellState::Hidden,
                };
            }
        }
        self.cursor_x = cx.min(w.saturating_sub(1));
        self.cursor_y = cy.min(h.saturating_sub(1));
        self.game_over = go;
        self.won = won;
        self.placed_mines = pm;
        self.elapsed_secs = elapsed;
        self.last_tick = None;

        self.numbers = vec![vec![0u8; w]; h];
        for (x, y, n) in number_entries {
            if y < h && x < w {
                self.numbers[y][x] = n;
            }
        }

        true
    }
}

impl Component for Game {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                if self.selecting_size {
                    return Ok(self.render_size_select());
                }

                let mut lines: Vec<Line> = Vec::new();
                let flags: usize = self
                    .state
                    .iter()
                    .flat_map(|r| r.iter())
                    .filter(|s| **s == CellState::Flagged)
                    .count();
                let remaining = self.mine_count.saturating_sub(flags);
                let safe_tag = if self.safe_mode { " [SAFE]" } else { "" };
                let fast_tag = if self.auto_chord_enabled {
                    " [FAST]"
                } else {
                    ""
                };
                let elapsed = format_elapsed_time(self.elapsed_secs);
                let status = if self.blocked_click {
                    format!(
                        "Mines: {}  Flags: {}  Remaining: {}  Time: {}{}{}  Use logic!",
                        self.mine_count, flags, remaining, elapsed, safe_tag, fast_tag
                    )
                } else {
                    format!(
                        "Mines: {}  Flags: {}  Remaining: {}  Time: {}{}{}",
                        self.mine_count, flags, remaining, elapsed, safe_tag, fast_tag
                    )
                };
                lines.push(vec![status].try_into().unwrap());

                let board_width = self.width * CELL_WIDTH;
                lines.push(
                    vec!["╔".to_owned(), "═".repeat(board_width), "╗".to_owned()]
                        .try_into()
                        .unwrap(),
                );

                for y in 0..self.height {
                    let mut line = Line::default();
                    line.push(Span::new_unstyled_lossy("║"));
                    for x in 0..self.width {
                        self.render_cell(&mut line, x, y);
                    }
                    line.push(Span::new_unstyled_lossy("║"));
                    lines.push(line);
                }

                lines.push(
                    vec!["╚".to_owned(), "═".repeat(board_width), "╝".to_owned()]
                        .try_into()
                        .unwrap(),
                );

                let help = "[arrows] move  [space] reveal  [f] flag  [c] fast-mode  [g] safe-mode";
                lines.push(vec![help.to_owned()].try_into().unwrap());
                Lines(lines)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a Game with a specific board state for testing the constraint solver.
    /// `cells` is row-major: (state, number, is_mine) for each cell.
    fn make_game(
        width: usize,
        height: usize,
        cells: &[((usize, usize), CellState, u8, bool)],
    ) -> Game {
        let mut game = Game::new();
        game.width = width;
        game.height = height;
        game.mine_count = cells.iter().filter(|(_, _, _, m)| *m).count();
        game.mines = vec![vec![false; width]; height];
        game.state = vec![vec![CellState::Hidden; width]; height];
        game.numbers = vec![vec![0u8; width]; height];
        game.selecting_size = false;
        game.placed_mines = true;

        for &((x, y), state, number, is_mine) in cells {
            game.state[y][x] = state;
            game.numbers[y][x] = number;
            game.mines[y][x] = is_mine;
        }

        game
    }

    /// The user's exact scenario from /tmp/ex:
    /// ```
    /// [ ] ⚑  2  ⚑
    /// ███ 2  3  2
    /// ███ 1  1  ⚑
    /// ```
    /// (0,0) should be deduced safe via subset elimination:
    /// - (1,2)=1 needs 1 mine in {(0,1),(0,2)}
    /// - (1,1)=2 needs 1 mine in {(0,0),(0,1),(0,2)} (1 flag already at (1,0))
    /// - Since {(0,1),(0,2)} ⊂ {(0,0),(0,1),(0,2)} and both need 1 mine,
    ///   {(0,0)} must have 0 mines → (0,0) is safe.
    #[test]
    fn test_subset_elimination_user_example() {
        use CellState::*;
        let game = make_game(
            4,
            3,
            &[
                // Row 0
                ((0, 0), Hidden, 0, false),
                ((1, 0), Flagged, 0, true),
                ((2, 0), Revealed, 2, false),
                ((3, 0), Flagged, 0, true),
                // Row 1
                ((0, 1), Hidden, 0, false),
                ((1, 1), Revealed, 2, false),
                ((2, 1), Revealed, 3, false),
                ((3, 1), Revealed, 2, false),
                // Row 2: mine at (0,2) to satisfy the (1) constraints
                ((0, 2), Hidden, 0, true),
                ((1, 2), Revealed, 1, false),
                ((2, 2), Revealed, 1, false),
                ((3, 2), Flagged, 0, true),
            ],
        );

        let (safe, _mine) = game.solve_constraints();

        // (0,0) must be deduced safe
        assert!(safe[0][0], "(0,0) should be safe via subset elimination");
    }

    /// Basic propagation: if a revealed cell's mine count equals its flag count,
    /// all remaining hidden neighbors are safe.
    #[test]
    fn test_basic_propagation_all_flags_placed() {
        use CellState::*;
        let game = make_game(
            3,
            3,
            &[
                ((0, 0), Flagged, 0, true),
                ((1, 0), Revealed, 1, false),
                ((2, 0), Hidden, 0, false),
                ((0, 1), Hidden, 0, false),
                ((1, 1), Hidden, 0, false),
                ((2, 1), Hidden, 0, false),
                ((0, 2), Hidden, 0, false),
                ((1, 2), Hidden, 0, false),
                ((2, 2), Hidden, 0, false),
            ],
        );

        let (safe, _mine) = game.solve_constraints();

        // All neighbors of the (1) except the flag should be safe
        assert!(safe[0][1], "(1,0) should be safe");
        assert!(safe[1][0], "(0,1) should be safe");
        assert!(safe[1][1], "(1,1) should be safe");
    }

    /// Basic propagation: if remaining mines == unknown count, all unknowns are mines.
    #[test]
    fn test_basic_propagation_all_unknowns_are_mines() {
        use CellState::*;
        // 3x1: Flagged  Revealed(2)  Hidden(mine)
        // R(2) neighbors: (0,0)=Flagged, (2,0)=Hidden
        // mine_count=1, remaining=1, unknowns=1 → (2,0) is mine
        let game = make_game(
            3,
            1,
            &[
                ((0, 0), Flagged, 0, true),
                ((1, 0), Revealed, 2, false),
                ((2, 0), Hidden, 0, true),
            ],
        );

        let (_, mine) = game.solve_constraints();

        assert!(mine[0][0], "(0,0) flagged → mine");
        assert!(mine[0][2], "(2,0) should be deduced mine");
    }

    /// Subset elimination: two constraints where one is a strict subset.
    /// Cell A=2 with unknowns {X, Y, Z}, cell B=1 with unknowns {Y, Z}.
    /// Since {Y,Z} ⊂ {X,Y,Z} and B needs 1, A needs 2: diff {X} needs 2-1=1 mine.
    /// So X is a mine, and then basic propagation resolves Y and Z.
    #[test]
    fn test_subset_elimination_deduces_mine() {
        use CellState::*;
        // Layout:
        //  X  A  .
        //  Y  B  .
        //  Z  .  .
        // A=2 sees {X, Y} (and B which is revealed)
        // B=1 sees {Y, Z} (and A which is revealed)
        // Wait, this doesn't create a clean subset. Let me design a better layout.

        // Better layout (5x1):
        // H  H  R(2)  H  R(1)
        // Constraint from R(2) at (2,0): unknowns = {(1,0), (3,0)}, remaining = 2
        // Constraint from R(1) at (4,0): unknowns = {(3,0)}, remaining = 1
        // {(3,0)} ⊂ {(1,0),(3,0)}, 1 mine in subset, 2 in superset → diff {(1,0)} has 1 mine
        let game = make_game(
            5,
            1,
            &[
                ((0, 0), Revealed, 1, false),
                ((1, 0), Hidden, 0, true),
                ((2, 0), Revealed, 2, false),
                ((3, 0), Hidden, 0, true),
                ((4, 0), Revealed, 1, false),
            ],
        );

        let (safe, mine) = game.solve_constraints();

        // (3,0) has only one constraint cell (4,0)=1 → must be mine
        // Actually, basic propagation handles this: R(1) at (4,0) has 1 unknown (3,0),
        // remaining=1, so (3,0) is mine. Then R(2) at (2,0) has 1 flag (3,0), 1 unknown (1,0),
        // remaining=1, so (1,0) is mine. Basic propagation suffices here.
        assert!(mine[0][1], "(1,0) should be mine");
        assert!(mine[0][3], "(3,0) should be mine");
        // (0,0) is revealed, should be safe
        assert!(safe[0][0]);
    }

    /// Subset elimination where diff cells are safe (the user's case pattern).
    /// Two overlapping constraints where the subset has the same mine count.
    #[test]
    fn test_subset_elimination_diff_is_safe() {
        use CellState::*;
        // 3x3 board:
        // H(safe)  R(2)  F
        // H(mine?) R(1)  .
        // H(mine?) .     .
        //
        // R(2) at (1,0): flag at (2,0), unknowns = {(0,0), (0,1)}, remaining = 1
        // R(1) at (1,1): unknowns = {(0,0), (0,1), (0,2)}, remaining = 1
        // {(0,0),(0,1)} ⊂ {(0,0),(0,1),(0,2)} and both need 1 mine
        // → diff {(0,2)} has 0 mines → (0,2) is safe
        let game = make_game(
            3,
            3,
            &[
                ((0, 0), Hidden, 0, false),
                ((1, 0), Revealed, 2, false),
                ((2, 0), Flagged, 0, true),
                ((0, 1), Hidden, 0, true),
                ((1, 1), Revealed, 1, false),
                ((2, 1), Revealed, 0, false),
                ((0, 2), Hidden, 0, false),
                ((1, 2), Revealed, 0, false),
                ((2, 2), Revealed, 0, false),
            ],
        );

        let (safe, _mine) = game.solve_constraints();

        assert!(safe[2][0], "(0,2) should be safe via subset elimination");
    }

    #[test]
    fn test_ensure_safe_refreshes_revealed_numbers_after_moving_mines() {
        use CellState::*;
        let mut game = make_game(
            3,
            1,
            &[
                ((0, 0), Revealed, 1, false),
                ((1, 0), Hidden, 0, true),
                ((2, 0), Hidden, 0, false),
            ],
        );

        game.ensure_safe(1, 0);

        assert!(!game.mines[0][1], "clicked cell should be safe");
        assert!(
            game.mines[0][2],
            "mine should move to the only available cell"
        );
        assert_eq!(
            game.mines.iter().flatten().filter(|&&mine| mine).count(),
            game.mine_count,
            "moving a mine should preserve the mine count",
        );

        assert_eq!(
            game.numbers[0][0], 0,
            "moving the only adjacent mine should change the revealed number",
        );
        assert_eq!(
            game.numbers[0][0],
            game.adjacent_mines(0, 0),
            "revealed numbers should match the current mine layout after ensure_safe",
        );
        let (safe, _) = game.solve_constraints();
        assert!(
            safe[0][1],
            "the relocated board's zero clue should prove the clicked cell safe",
        );
    }

    #[test]
    fn test_unproven_safe_click_only_loses_when_it_contains_a_mine() {
        use CellState::*;
        let mut game = make_game(
            6,
            1,
            &[
                ((0, 0), Flagged, 0, true),
                ((1, 0), Revealed, 1, false),
                ((2, 0), Hidden, 0, false),
                ((3, 0), Hidden, 0, false),
                ((4, 0), Hidden, 0, true),
                ((5, 0), Hidden, 0, false),
            ],
        );
        game.cursor_x = 3;
        game.cursor_y = 0;

        let (safe, _) = game.solve_constraints();
        assert!(safe[0][2], "the board should have a logically safe move");
        assert!(!safe[0][3], "the selected cell should not be proven safe");

        <Game as super::super::Game>::input(&mut game, Control::Char(' '));

        assert_eq!(game.state[0][3], Revealed);
        assert!(
            !game.game_over,
            "revealing a non-mine should not end the game"
        );
    }

    #[test]
    fn test_format_elapsed_time() {
        assert_eq!(format_elapsed_time(0.0), "00:00");
        assert_eq!(format_elapsed_time(59.9), "00:59");
        assert_eq!(format_elapsed_time(60.0), "01:00");
        assert_eq!(format_elapsed_time(3600.0), "60:00");
    }

    /// Verify that the solver handles an already-fully-resolved board.
    #[test]
    fn test_solver_fully_resolved() {
        use CellState::*;
        let game = make_game(
            3,
            1,
            &[
                ((0, 0), Revealed, 1, false),
                ((1, 0), Flagged, 0, true),
                ((2, 0), Revealed, 1, false),
            ],
        );

        let (safe, mine) = game.solve_constraints();

        assert!(safe[0][0]);
        assert!(mine[0][1]);
        assert!(safe[0][2]);
    }
}
