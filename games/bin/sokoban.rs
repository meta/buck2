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
use games::console::RawMode;
use games::console::Stdin;
use games::console::with_sigint_handler;
use games::games::run_game;
use games::games::sokoban::Game;
use games::games::sokoban::parse_levels_text;
use games::games::sokoban::solve_from_levels;
use games::games::sokoban::solve_level;
use superconsole::SuperConsole;

#[derive(Parser)]
struct Args {
    #[clap(long)]
    save_state: Option<PathBuf>,
    #[clap(long)]
    load_state: Option<PathBuf>,
    /// Solve a level using A* and print the solution (1-indexed)
    #[clap(long)]
    solve: Option<usize>,
    /// Start at a specific level (1-indexed)
    #[clap(long)]
    level: Option<usize>,
    /// Show box numbers instead of box borders
    #[clap(long)]
    numbered: bool,
    /// Load levels from a file instead of the built-in levels
    #[clap(long)]
    levels_file: Option<PathBuf>,
}

fn load_external_levels(path: &std::path::Path) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Failed to read levels file {}: {}", path.display(), e);
        std::process::exit(1);
    });
    let levels = parse_levels_text(&text);
    if levels.is_empty() {
        eprintln!("No levels found in {}", path.display());
        std::process::exit(1);
    }
    levels
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if let Some(level) = args.solve {
        if level == 0 {
            eprintln!("Level numbers are 1-indexed");
            std::process::exit(1);
        }
        let idx = level - 1;
        eprint!("Solving level {}...", level);
        let result = if let Some(ref path) = args.levels_file {
            let levels = load_external_levels(path);
            solve_from_levels(&levels, idx)
        } else {
            solve_level(idx)
        };
        match result {
            Some(solution) => {
                let push_count = solution.split_whitespace().count();
                eprintln!(" solved in {} pushes!", push_count);
                println!("{}", solution);
            }
            None => {
                eprintln!(" no solution found.");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    with_sigint_handler(async {
        let _raw = RawMode::enable()?;
        let mut renderer = SuperConsole::new().unwrap();
        let mut stdin = Stdin::new(8192);
        let start_level = args.level.map_or(0, |l| {
            if l == 0 {
                eprintln!("Level numbers are 1-indexed");
                std::process::exit(1);
            }
            l - 1
        });
        let mut game = if let Some(ref path) = args.levels_file {
            let levels = load_external_levels(path);
            Game::new_from_levels(levels, start_level)
        } else if start_level > 0 {
            Game::new_at_level(start_level)
        } else {
            Game::new()
        };
        game.set_numbered(args.numbered);
        run_game(
            &mut renderer,
            &mut stdin,
            &mut game,
            None,
            args.save_state.as_deref(),
            args.load_state.as_deref(),
            |_| {},
        )
        .await?;
        Ok(())
    })
    .await
    .unwrap_or(Ok(()))
}
