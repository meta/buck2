/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::io::BufRead;

use anyhow::Context;
use anyhow::bail;
use setsketch::SetSketch;

fn main() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let lines: Vec<String> = stdin.lock().lines().collect::<Result<_, _>>()?;

    if lines.is_empty() {
        bail!("No input provided. Please provide one or more sketch strings.");
    }

    // Decode all sketches
    let mut sketches: Vec<SetSketch> = Vec::new();
    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let sketch = SetSketch::from_base64_maybe_versioned(trimmed)
                .context("Failed to decode sketch")?;
            sketches.push(sketch);
        }
    }

    if sketches.is_empty() {
        bail!("No valid sketches found in input");
    }

    // Merge all sketches
    let mut sketches_iter = sketches.into_iter();
    let mut merged = sketches_iter.next().unwrap();
    for sketch in sketches_iter {
        merged.merge(&sketch);
    }

    // Compute and print cardinality
    let cardinality = merged.cardinality();
    println!("{}", cardinality);

    Ok(())
}
