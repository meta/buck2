/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Game selection menu with save/continue support and high score viewing.
//!
//! Beyond a basic menu:
//!
//! - **Two-level selection**: selecting a game opens a New Game / Continue
//!   sub-menu. Continue is greyed out (via `has_save_fn` callback) when
//!   no save file exists, preventing invalid selections.
//!
//! - **Animated title**: "BUCK2 GAMES" with a cycling dot animation
//!   (`. .. ... `) driven by the tick counter.
//!
//! - **High scores entry**: the game list includes a "High Scores" item
//!   at the bottom, separated by a horizontal rule, for viewing the
//!   per-game high score table.
//!
//! - **Reusable bordered layout**: `bordered_line()` and `hline()` are
//!   public helpers used by both the menu and the high scores display,
//!   ensuring consistent box-drawing formatting across screens.
//!
//! - **Fixed-height rendering**: the menu binary uses `FixedHeight` to
//!   pad output to the height of the tallest game, preventing display
//!   jumps when switching between menu and gameplay.

use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Color;
use superconsole::style::Stylize;

use crate::console::Control;

pub struct MenuItem {
    pub name: &'static str,
    pub description: &'static str,
}

/// Result from the menu: either a game selection with new/continue, view high scores, or quit.
pub enum MenuResult {
    /// Start a new game at index
    NewGame(usize),
    /// Continue saved game at index
    Continue(usize),
    /// View high scores
    ViewHighScores,
}

enum MenuState {
    GameSelect,
    NewOrContinue { game_idx: usize, has_save: bool },
}

pub struct Menu {
    items: Vec<MenuItem>,
    selected: usize,
    tick_count: u32,
    state: MenuState,
    /// sub-menu cursor: 0 = New Game, 1 = Continue
    sub_selected: usize,
    /// Callback to check if a save exists for a game name
    has_save_fn: Box<dyn Fn(&str) -> bool + Send>,
}

// Total width between the two border chars ║...║
// "  ║" (4 display cols) + INNER_WIDTH + "║" (1 display col)
const INNER_WIDTH: usize = 45;

/// Build a line: "  ║" + content padded to INNER_WIDTH + "║"
/// Content is a vec of spans. Padding is added to reach INNER_WIDTH.
/// Panics if content is wider than INNER_WIDTH.
pub fn bordered_line(spans: Vec<Span>) -> Line {
    let mut line = Line::default();
    line.push(Span::new_unstyled_lossy("  ║"));
    let content_width: usize = spans.iter().map(|s| s.len()).sum();
    assert!(
        content_width <= INNER_WIDTH,
        "content width {} exceeds INNER_WIDTH {}",
        content_width,
        INNER_WIDTH
    );
    for s in spans {
        line.push(s);
    }
    if content_width < INNER_WIDTH {
        line.push(Span::new_unstyled_lossy(
            " ".repeat(INNER_WIDTH - content_width),
        ));
    }
    line.push(Span::new_unstyled_lossy("║"));
    line
}

/// Build a horizontal border line like "  ╔════...════╗"
pub fn hline(left: &str, right: &str) -> Line {
    let bar = "═".repeat(INNER_WIDTH);
    vec![format!("  {left}{bar}{right}")].try_into().unwrap()
}

impl Menu {
    pub fn new(items: Vec<MenuItem>, has_save_fn: Box<dyn Fn(&str) -> bool + Send>) -> Self {
        Self {
            items,
            selected: 0,
            tick_count: 0,
            state: MenuState::GameSelect,
            sub_selected: 0,
            has_save_fn,
        }
    }

    pub fn tick(&mut self, tick_count: u32) {
        self.tick_count = tick_count;
    }

    /// Returns Some(MenuResult) if a selection was made, None otherwise.
    pub fn input(&mut self, input: Control) -> Option<MenuResult> {
        match &self.state {
            MenuState::GameSelect => match input {
                Control::Up | Control::Char('w') => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                Control::Down | Control::Char('s') => {
                    if self.selected < self.items.len() {
                        self.selected += 1;
                    }
                }
                Control::Char('\r') | Control::Char('\n') => {
                    if self.selected == self.items.len() {
                        return Some(MenuResult::ViewHighScores);
                    }
                    let has_save = (self.has_save_fn)(self.items[self.selected].name);
                    self.sub_selected = 0;
                    self.state = MenuState::NewOrContinue {
                        game_idx: self.selected,
                        has_save,
                    };
                }
                _ => {}
            },
            MenuState::NewOrContinue { game_idx, has_save } => {
                let game_idx = *game_idx;
                let has_save = *has_save;
                match input {
                    Control::Escape => {
                        self.state = MenuState::GameSelect;
                    }
                    Control::Up | Control::Char('w') => {
                        self.sub_selected = 0;
                    }
                    Control::Down | Control::Char('s') => {
                        if has_save {
                            self.sub_selected = 1;
                        }
                    }
                    Control::Char('\r') | Control::Char('\n') => {
                        self.state = MenuState::GameSelect;
                        if self.sub_selected == 0 {
                            return Some(MenuResult::NewGame(game_idx));
                        } else {
                            return Some(MenuResult::Continue(game_idx));
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn render_game_row(&self, item: &MenuItem, is_selected: bool) -> Line {
        let cursor = if is_selected { "▶ " } else { "  " };
        let name_padded = format!("{:<12}", item.name);
        let desc = item.description;
        // " " + cursor(2) + name(12) + "  " + desc(right-aligned) + " "
        let target = INNER_WIDTH - 2; // space on each side
        let used = 2 + 12 + 2; // cursor + name + gap = 16
        let desc_padded = format!("{:>width$}", desc, width = target - used);

        let content = format!(" {cursor}{name_padded}  {desc_padded} ");
        let spans = if is_selected {
            vec![Span::new_styled_lossy(content.bold())]
        } else {
            vec![Span::new_unstyled_lossy(&content)]
        };
        bordered_line(spans)
    }

    fn render_sub_row(&self, label: &str, is_selected: bool, enabled: bool) -> Line {
        let cursor = if is_selected && enabled { "▶ " } else { "  " };
        let content = format!("    {cursor}{label}");
        let spans = if !enabled {
            // Grey out just the label, keep borders normal
            let prefix = "    ";
            let cursor_space = "  "; // no cursor shown
            vec![
                Span::new_unstyled_lossy(format!("{prefix}{cursor_space}")),
                Span::new_colored_lossy(label, Color::DarkGrey),
            ]
        } else if is_selected {
            vec![Span::new_styled_lossy(content.bold())]
        } else {
            vec![Span::new_unstyled_lossy(&content)]
        };
        bordered_line(spans)
    }
}

impl Component for Menu {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => {
                let mut lines: Vec<Line> = Vec::new();

                lines.push(Line::default());
                lines.push(hline("╔", "╗"));

                // Animate the title
                let dots = ".".repeat((self.tick_count as usize / 8) % 4);
                let title = format!(" BUCK2 GAMES{:<17}", dots);
                lines.push(bordered_line(vec![Span::new_unstyled_lossy(&title)]));

                lines.push(hline("╠", "╣"));

                match &self.state {
                    MenuState::GameSelect => {
                        for (i, item) in self.items.iter().enumerate() {
                            lines.push(self.render_game_row(item, i == self.selected));
                        }
                        lines.push(hline("╠", "╣"));
                        let is_hs_selected = self.selected == self.items.len();
                        let cursor = if is_hs_selected { "▶ " } else { "  " };
                        let content = format!(" {cursor}High Scores");
                        let spans = if is_hs_selected {
                            vec![Span::new_styled_lossy(content.bold())]
                        } else {
                            vec![Span::new_unstyled_lossy(&content)]
                        };
                        lines.push(bordered_line(spans));
                        lines.push(hline("╚", "╝"));
                        lines.push(Line::default());
                        lines.push(vec!["  ↑/↓ to move, Enter to select"].try_into().unwrap());
                    }
                    MenuState::NewOrContinue { game_idx, has_save } => {
                        let game_name = self.items[*game_idx].name;
                        lines.push(bordered_line(vec![Span::new_unstyled_lossy(format!(
                            " {game_name}"
                        ))]));
                        lines.push(bordered_line(vec![]));
                        lines.push(self.render_sub_row("New Game", self.sub_selected == 0, true));
                        lines.push(self.render_sub_row(
                            "Continue",
                            self.sub_selected == 1,
                            *has_save,
                        ));
                        lines.push(bordered_line(vec![]));
                        lines.push(hline("╚", "╝"));
                        lines.push(Line::default());
                        lines.push(
                            vec!["  ↑/↓ to move, Enter to select, Esc to go back"]
                                .try_into()
                                .unwrap(),
                        );
                    }
                }

                Lines(lines)
            }
        })
    }
}
