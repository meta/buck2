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
use games::games::twenty48::Game;
use superconsole::SuperConsole;

#[derive(Parser)]
struct Args {
    #[clap(long)]
    save_state: Option<PathBuf>,
    #[clap(long)]
    load_state: Option<PathBuf>,
    #[clap(long)]
    render_test: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tokio::time::sleep(Duration::from_secs(1)).await;
    let args = Args::parse();
    with_sigint_handler(async {
        let _raw = RawMode::enable()?;
        let mut renderer = SuperConsole::new().expect("failed to create superconsole");
        let mut stdin = Stdin::new(8192);
        let mut game = if args.render_test {
            Game::render_test()
        } else {
            Game::new()
        };
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
