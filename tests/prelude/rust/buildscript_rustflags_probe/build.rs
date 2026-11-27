/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let rustc = env::var_os("RUSTC").unwrap();
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let rustflags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let probe = Path::new(&out_dir).join("probe.rs");
    fs::write(&probe, "").unwrap();
    let compiles = Command::new(rustc)
        .args(rustflags.split('\x1f').filter(|flag| !flag.is_empty()))
        .args(["--crate-type=lib", "--emit=metadata", "--out-dir"])
        .arg(&out_dir)
        .arg(&probe)
        .status()
        .unwrap()
        .success();
    if compiles {
        println!("cargo:rustc-cfg=probe_compiles");
    }
}
