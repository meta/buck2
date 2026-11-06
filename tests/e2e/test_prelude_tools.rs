/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Runs prelude tools the way the rules run them: as actions of a buck2 build, through the
//! tools' command lines, in a workspace that uses the bundled prelude.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

/// The compile of `r.c`, built with the real clang the test target names in `CLANG` (clang from
/// `PATH` when it is unset). It runs locally: the compiler is a path on this host.
const DEP_FILES_LIB: &str = "root//:lib";
/// The test writes this header: a checked-in file name may not contain a space.
const SPACED_HEADER: &str = "sub/my dir/s.h";
const DEP_FILES_HEADERS: [&str; 3] = [".gen/g.h", SPACED_HEADER, "v.h"];

async fn output(buck: &Buck, target: &str) -> anyhow::Result<PathBuf> {
    let res = buck
        .build_without_report(&[target, "--show-full-simple-output"])
        .await?;
    Ok(PathBuf::from(res.stdout.trim()))
}

/// What executing the Python module at `path` binds `name` to, as JSON. It runs on the interpreter
/// the fixture's `system_python_bootstrap_toolchain` runs the tools with.
async fn exec_python(path: &Path, name: &str) -> anyhow::Result<Value> {
    let interpreter = if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    };
    let output = tokio::process::Command::new(interpreter)
        .args([
            "-c",
            "import json, sys\nnamespace = {}\nexec(open(sys.argv[1]).read(), namespace)\nprint(json.dumps(namespace[sys.argv[2]]))",
        ])
        .arg(path)
        .arg(name)
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "Executing `{}` failed ({}):\n{}",
        path.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

// prelude/python/tools/make_py_package_manifest_module.py
#[buck_test(inplace = false)]
async fn test_manifest_module_lists_import_names(buck: &Buck) -> anyhow::Result<()> {
    let module = output(buck, "root//:manifest_module").await?;
    assert_eq!(
        exec_python(&module, "modules").await?,
        serde_json::json!(["pkg.__init__", "pkg._speedups"])
    );
    Ok(())
}

fn write_spaced_header(buck: &Buck) -> anyhow::Result<()> {
    let path = buck.cwd().join(SPACED_HEADER);
    std::fs::create_dir_all(path.parent().context("The header has no directory")?)?;
    std::fs::write(path, "#define S 1\n")?;
    Ok(())
}

fn clang_config(buck: &Buck) -> Vec<String> {
    match buck.env_var("CLANG") {
        Some(clang) if !clang.is_empty() => {
            vec![
                "-c".to_owned(),
                format!("test.clang={}", clang.to_string_lossy()),
            ]
        }
        _ => Vec::new(),
    }
}

async fn build_object(buck: &Buck) -> anyhow::Result<()> {
    let target = format!("{DEP_FILES_LIB}[objects][r.c.o]");
    let clang = clang_config(buck);
    let mut args = vec![target.as_str(), "--local-only"];
    args.extend(clang.iter().map(String::as_str));
    buck.build(&args).await?;
    Ok(())
}

/// The headers buck2 tracks for the compile, from the dep file the processor wrote.
async fn tagged_headers(buck: &Buck) -> anyhow::Result<Vec<String>> {
    let clang = clang_config(buck);
    let mut args = vec![DEP_FILES_LIB, "c_compile", "r.c"];
    args.extend(clang.iter().map(String::as_str));
    let res = buck.audit_dep_files(&args).await?;
    let mut headers: Vec<String> = res
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("headers\t"))
        .map(str::to_owned)
        .collect();
    headers.sort();
    Ok(headers)
}

/// How the last command ran the compile of `r.c`: `Local`, or `LocalDepFileCache` when the dep file
/// said that no used input changed and the old object was kept.
async fn compile_executors(buck: &Buck) -> anyhow::Result<Vec<String>> {
    read_what_ran(buck, &[])
        .await?
        .iter()
        .filter(|entry| {
            entry["identity"]
                .as_str()
                .is_some_and(|identity| identity.contains("(c_compile r.c)"))
        })
        .map(|entry| {
            entry["reproducer"]["executor"]
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("No executor in {entry}"))
        })
        .collect()
}

// prelude/cxx/tools/show_headers_to_dep_file.py
#[buck_test(inplace = false, skip_for_os = ["darwin", "windows"])]
async fn test_show_headers_dep_file_keeps_the_headers_found_next_to_the_source(
    buck: &Buck,
) -> anyhow::Result<()> {
    write_spaced_header(buck)?;
    build_object(buck).await?;
    // The rules compile `./r.c`, so clang prints every header it found relative to the source as
    // `. ./<path>`; the dep file keeps each path as printed.
    assert_eq!(tagged_headers(buck).await?, DEP_FILES_HEADERS);

    // So an edit to the three headers rebuilds the object.
    for header in DEP_FILES_HEADERS {
        let path = buck.cwd().join(header);
        let content = std::fs::read_to_string(&path)?;
        std::fs::write(&path, content.replace(" 1\n", " 2\n"))?;
    }
    build_object(buck).await?;
    assert_eq!(compile_executors(buck).await?, ["Local"]);
    Ok(())
}
