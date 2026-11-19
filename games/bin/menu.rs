/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use futures::pin_mut;
use games::console::Control;
use games::console::RawMode;
use games::console::Stdin;
use games::console::control_reader;
use games::console::with_sigint_handler;
use games::games::ExitReason;
use games::games::FixedHeight;
use games::games::Game;
use games::games::HighScores;
use games::games::menu::Menu;
use games::games::menu::MenuItem;
use games::games::menu::MenuResult;
use games::games::menu::bordered_line;
use games::games::menu::hline;
use games::games::run_game;
use superconsole::Dimensions;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::SuperConsole;
use superconsole::components::Component;
use superconsole::components::DrawMode;
use superconsole::style::Stylize;
use tokio::select;
use tokio::time;

const GAMES: &[(&str, &str)] = &[
    ("blocks", "color matching"),
    ("snake", "classic snake"),
    ("jump", "side scroller"),
    ("2048", "tile slider"),
    ("minesweeper", "mine finder"),
    ("breakout", "brick breaker"),
    ("sokoban", "box pusher"),
    ("life", "cellular automaton"),
    ("static", "random colors"),
];

/// The tallest game renders 43 lines (snake: 1 score + 1 top + 40 board + 1 bottom;
/// blocks: 1 top + 40 cells + 1 bottom + 1 state). Use this as a fixed height so
/// switching between menu and game doesn't cause the display to jump.
const FIXED_HEIGHT: usize = 43;

fn save_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(home).join(".buck2_games")
}

fn save_path_for(game_name: &str) -> PathBuf {
    save_dir().join(format!("{}.json", game_name))
}

fn has_save(game_name: &str) -> bool {
    save_path_for(game_name).exists()
}

async fn run_menu(
    renderer: &mut SuperConsole,
    stdin: &mut Stdin,
) -> anyhow::Result<Option<MenuResult>> {
    let delay = Duration::from_millis(1000 / 30);
    let mut interval = time::interval(delay);

    let console_reader = control_reader(stdin);
    pin_mut!(console_reader);

    let items: Vec<MenuItem> = GAMES
        .iter()
        .map(|(name, desc)| MenuItem {
            name,
            description: desc,
        })
        .collect();
    let mut menu = Menu::new(items, Box::new(has_save));

    let mut tick_count = 0;
    let selected = loop {
        select! {
            _ = interval.tick() => {
                tick_count += 1;
                menu.tick(tick_count);
                renderer.render(&FixedHeight::new(&menu, FIXED_HEIGHT)).unwrap();
            }
            c = console_reader.next() => {
                match c {
                    None => { return Ok(None); }
                    Some(c) => {
                        if let Some(result) = menu.input(c?) {
                            break result;
                        }
                    }
                }
            }
        }
    };

    Ok(Some(selected))
}

fn make_game(idx: usize) -> (Box<dyn Game>, &'static str) {
    match idx {
        0 => (Box::new(games::games::blocks::Game::new()), "blocks"),
        1 => (Box::new(games::games::snake::Game::new()), "snake"),
        2 => (Box::new(games::games::jump::Game::new()), "jump"),
        3 => (Box::new(games::games::twenty48::Game::new()), "2048"),
        4 => (
            Box::new(games::games::minesweeper::Game::new()),
            "minesweeper",
        ),
        5 => (Box::new(games::games::breakout::Game::new()), "breakout"),
        6 => (Box::new(games::games::sokoban::Game::new()), "sokoban"),
        7 => (Box::new(games::games::life::Game::new()), "life"),
        8 => (Box::new(games::games::r#static::Game::new()), "static"),
        _ => unreachable!(),
    }
}

struct StaticLines(Lines);

impl Component for StaticLines {
    type Error = anyhow::Error;

    fn draw_unchecked(&self, _dimensions: Dimensions, mode: DrawMode) -> anyhow::Result<Lines> {
        Ok(match mode {
            DrawMode::Final => Lines::new(),
            DrawMode::Normal => self.0.clone(),
        })
    }
}

async fn run_high_scores(renderer: &mut SuperConsole, stdin: &mut Stdin) -> anyhow::Result<()> {
    let hs_path = save_dir().join("high_scores.json");
    let high_scores = HighScores::load(&hs_path);

    let mut lines: Vec<Line> = vec![
        Line::default(),
        hline("╔", "╗"),
        bordered_line(vec![Span::new_styled_lossy(
            " HIGH SCORES".to_owned().bold(),
        )]),
        hline("╠", "╣"),
    ];

    let mut any_scores = false;
    for &(game_name, _) in GAMES {
        if let Some(entries) = high_scores.get(game_name) {
            if entries.is_empty() {
                continue;
            }
            any_scores = true;
            lines.push(bordered_line(vec![]));
            lines.push(bordered_line(vec![Span::new_styled_lossy(
                format!("  {game_name}").bold(),
            )]));
            for (category, entry) in entries {
                let label = format!("    {:<30}{:>7}", category, entry.value);
                lines.push(bordered_line(vec![Span::new_unstyled_lossy(&label)]));
            }
        }
    }

    if !any_scores {
        lines.push(bordered_line(vec![]));
        lines.push(bordered_line(vec![Span::new_unstyled_lossy(
            " No high scores yet.",
        )]));
    }

    lines.push(bordered_line(vec![]));
    lines.push(hline("╚", "╝"));
    lines.push(Line::default());
    lines.push(vec!["  Esc to go back"].try_into().unwrap());

    let content = StaticLines(Lines(lines));
    renderer
        .render(&FixedHeight::new(&content, FIXED_HEIGHT))
        .unwrap();

    let console_reader = control_reader(stdin);
    pin_mut!(console_reader);

    while let Some(c) = console_reader.next().await {
        match c? {
            Control::Escape | Control::Char('q') => break,
            _ => {}
        }
    }

    Ok(())
}

#[derive(Parser)]
struct Args {
    /// Clear all saved games and high scores
    #[clap(long)]
    clear: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.clear {
        let dir = save_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
            eprintln!("Cleared {}", dir.display());
        } else {
            eprintln!("Nothing to clear");
        }
        return Ok(());
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    with_sigint_handler(async {
        let _raw = RawMode::enable()?;
        let mut renderer = SuperConsole::new().unwrap();
        let mut stdin = Stdin::new(8192);

        loop {
            let result = match run_menu(&mut renderer, &mut stdin).await? {
                Some(r) => r,
                None => return Ok(()),
            };

            let (load, game_idx) = match result {
                MenuResult::NewGame(idx) => {
                    // Delete existing save if any
                    let game_name = GAMES[idx].0;
                    let path = save_path_for(game_name);
                    drop(std::fs::remove_file(&path));
                    (false, idx)
                }
                MenuResult::Continue(idx) => (true, idx),
                MenuResult::ViewHighScores => {
                    run_high_scores(&mut renderer, &mut stdin).await?;
                    continue;
                }
            };

            let (mut game, game_name) = make_game(game_idx);
            let path = save_path_for(game_name);
            let load_path = if load { Some(path.as_path()) } else { None };

            let hs_path = save_dir().join("high_scores.json");
            match run_game(
                &mut renderer,
                &mut stdin,
                game.as_mut(),
                Some(FIXED_HEIGHT),
                Some(path.as_path()),
                load_path,
                |scores| {
                    let mut high_scores = HighScores::load(&hs_path);
                    high_scores.record(game_name, scores);
                    high_scores.save(&hs_path);
                },
            )
            .await?
            {
                ExitReason::Escape => continue,
                ExitReason::GameOver(scores) => {
                    if !scores.is_empty() {
                        let mut high_scores = HighScores::load(&hs_path);
                        high_scores.record(game_name, &scores);
                        high_scores.save(&hs_path);
                    }
                    continue;
                }
                ExitReason::Finished => return Ok(()),
            }
        }
    })
    .await
    .unwrap_or(Ok(()))
}
