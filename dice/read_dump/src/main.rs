/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs::File;
use std::path::PathBuf;

use clap::CommandFactory;
use clap::FromArgMatches;
use dice::introspection::graph::SerializedGraphNodeForKey;

#[derive(Debug, clap::Parser)]
#[clap(name = "read_dump", about = "dice dump reader")]
pub(crate) struct Opt {
    #[clap(name = "DICE_DUMP", help = "The dice dump")]
    file: PathBuf,
    #[clap(long = "out", help = "Copy the output to this path")]
    out: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let clap = <Opt as CommandFactory>::command();
    let matches = clap.get_matches_from(std::env::args().collect::<Vec<String>>());
    let opt = Opt::from_arg_matches(&matches)?;

    let mut file = File::open(opt.file)?;

    let out: Vec<SerializedGraphNodeForKey> =
        bincode::serde::decode_from_std_read(&mut file, bincode::config::legacy())?;

    match opt.out {
        Some(path) => {
            serde_json::to_writer_pretty(File::create(path)?, &out)?;
        }
        None => {
            serde_json::to_writer_pretty(std::io::stdout(), &out)?;
        }
    };

    Ok(())
}
