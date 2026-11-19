/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Read;
use std::path::Path;

use buck2_e2e::prelude::*;

fn read_gz(path: &Path) -> anyhow::Result<String> {
    let mut text = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path)?).read_to_string(&mut text)?;
    Ok(text)
}

#[buck_test]
async fn test_debug_legacy_dice_dump(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let file_path = tmp_path.join("dump");

    buck.uquery(&["//..."]).await?;
    buck.debug(&["dice-dump", "--path", file_path.to_str().unwrap()])
        .await?;

    assert!(file_path.join("nodes.gz").exists());
    assert!(file_path.join("edges.gz").exists());
    assert!(file_path.join("nodes_currently_running.gz").exists());

    let nodes = read_gz(&file_path.join("nodes.gz"))?;
    assert!(nodes.contains("BuildDataKey"));
    assert!(nodes.contains("IoReadDirKey"));

    let edges = read_gz(&file_path.join("edges.gz"))?;
    eprintln!("{edges}");
    assert!(!edges.is_empty());

    let nodes_currently_running = read_gz(&file_path.join("nodes_currently_running.gz"))?;
    eprintln!("{nodes_currently_running}");
    assert_eq!(nodes_currently_running, "");
    Ok(())
}
