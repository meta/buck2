/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

/// The JSON objects of `buck2 log what-ran --format json`, one per non-empty line.
fn json_lines(stdout: &str) -> anyhow::Result<Vec<Value>> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).with_context(|| format!("Parsing `{line}`")))
        .collect()
}

/// The `reason` of a record, or the empty string if it has none.
fn reason(repro: &Value) -> anyhow::Result<&str> {
    match repro.get("reason") {
        None => Ok(""),
        Some(reason) => reason
            .as_str()
            .with_context(|| format!("A `reason` that is not a string in {repro}")),
    }
}

// TODO(marwhal): Fix and enable on Windows
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_what_ran_json_target_without_explicit_test_cases(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test"])
        .await?;
    let out = buck.log(&["what-ran", "--format", "json"]).await?;
    let out = json_lines(&out.stdout)?;
    assert_eq!(out.len(), 1, "out should have 1 line: `{out:?}`");

    let repro = &out[0];
    assert_eq!(repro["reason"], "test.run");
    assert_eq!(
        repro["identity"],
        "fbcode//buck2/tests/targets/rules/sh_test:test"
    );
    assert_eq!(repro["reproducer"]["executor"], "Local");
    assert_eq!(repro["reproducer"]["details"]["command"][1], "arg1");
    assert_eq!(repro["extra"]["testcases"], serde_json::json!([]));
    Ok(())
}

// Linux only: the next five tests build targets in an fbcode target configuration, unsupported on
// mac RE workers.

#[cfg(target_os = "linux")]
#[buck_test(inplace = true)]
async fn test_what_ran(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["fbcode//buck2/tests/targets/rules/cxx/hello_world:welcome"])
        .await?;
    let out = buck.log(&["what-ran"]).await?;
    assert!(out.stdout.contains("welcome"));

    buck.test(&["fbcode//buck2/tests/targets/rules/sh_test:test"])
        .await?;
    let out = buck.log(&["what-ran"]).await?;
    assert!(out.stdout.contains("sh_test/test.py arg1"));
    Ok(())
}

#[cfg(target_os = "linux")]
#[buck_test(inplace = true)]
async fn test_what_ran_filter_category(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["fbsource//fbobjc/buck2/samples/hello_world:HelloWorldBundle"])
        .await?;
    let out = buck
        .log(&[
            "what-ran",
            "--filter-category",
            ".*cxx.*",
            "--format",
            "json",
        ])
        .await?;
    let out = json_lines(&out.stdout)?;
    let mut matches = false;
    for rec in &out {
        let identity = rec["identity"]
            .as_str()
            .with_context(|| format!("No `identity` in {rec}"))?;
        if identity.contains("cxx_link_executable") || identity.contains("cxx_compile") {
            matches = true;
        }
    }
    assert!(
        matches,
        "action identity must contain the filtered category: `{out:?}`"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[buck_test(inplace = true)]
async fn test_what_ran_show_std_err(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["fbcode//buck2/tests/targets/rules/genrule/bad/..."])
        .await
        .expect_failure();
    let out = buck
        .log(&["what-ran", "--show-std-err", "--format", "json"])
        .await?;
    let out = json_lines(&out.stdout)?;
    let mut matched = false;
    for rec in &out {
        let std_err = rec.get("std_err").context("A record without `std_err`")?;
        if std_err == "" || std_err == "HELLO_STDERR\n" {
            matched = true;
            break;
        }
    }
    assert!(
        matched,
        "we should have some empty std_errs and also HELLO_STDERR since we echo it in TARGETS: `{out:?}`"
    );

    let out = buck
        .log(&[
            "what-ran",
            "--show-std-err",
            "--omit-empty-std-err",
            "--format",
            "json",
        ])
        .await?;
    let out = json_lines(&out.stdout)?;
    for rec in &out {
        let std_err = rec
            .get("std_err")
            .with_context(|| format!("No `std_err` in {rec}"))?;
        assert_ne!(std_err, "", "we should have no empty std_errs: `{out:?}`");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[buck_test(inplace = true)]
async fn test_what_ran_json_target_with_test_cases(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/go/test:test"])
        .await?;
    let out = buck.log(&["what-ran", "--format", "json"]).await?;
    let mut tests = Vec::new();
    for repro in json_lines(&out.stdout)? {
        if reason(&repro)?.starts_with("test.") {
            tests.push(repro);
        }
    }
    let out = tests;
    assert_eq!(out.len(), 2, "out should have 2 test lines: `{out:?}`");

    let repros: BTreeMap<&str, &Value> = out
        .iter()
        .map(|repro| Ok((reason(repro)?, repro)))
        .collect::<anyhow::Result<_>>()?;

    // test discovery
    let discovery = repros
        .get("test.discovery")
        .context("No `test.discovery` record")?;
    assert_eq!(
        discovery["identity"],
        "fbcode//buck2/tests/targets/rules/go/test:test"
    );

    // test running
    let repro = repros.get("test.run").context("No `test.run` record")?;
    assert_eq!(repro["reason"], "test.run");
    assert_eq!(
        repro["identity"],
        "fbcode//buck2/tests/targets/rules/go/test:test"
    );
    assert_eq!(repro["reproducer"]["executor"], "Local");
    assert_eq!(repro["extra"]["testcases"], serde_json::json!(["TestFoo"]));
    Ok(())
}

#[cfg(target_os = "linux")]
#[buck_test(inplace = true)]
async fn test_what_ran_csv_target_with_test_cases(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/go/test:test"])
        .await?;
    let out = buck.log(&["what-ran", "--format", "csv"]).await?;
    let lines = out
        .stdout
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    let header = ["reason", "identity", "executor", "reproducer"];
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(lines.as_bytes());
    let mut out: Vec<BTreeMap<&str, String>> = Vec::new();
    for record in reader.records() {
        let record = record?;
        if record.is_empty() {
            continue;
        }
        out.push(
            header
                .iter()
                .copied()
                .zip(record.iter().map(str::to_owned))
                .collect(),
        );
    }
    let header_record: BTreeMap<&str, String> = header
        .iter()
        .map(|field| (*field, (*field).to_owned()))
        .collect();
    assert_eq!(
        out[0], header_record,
        "ensure that first entry in csv is the header"
    );
    let out: Vec<_> = out
        .into_iter()
        .filter(|repro| {
            repro
                .get("reason")
                .is_some_and(|reason| reason.starts_with("test."))
        })
        .collect();
    assert_eq!(out.len(), 2, "out should have 2 test lines: `{out:?}`");
    let repros: BTreeMap<&str, &BTreeMap<&str, String>> = out
        .iter()
        .map(|repro| Ok((repro.get("reason").context("No `reason`")?.as_str(), repro)))
        .collect::<anyhow::Result<_>>()?;

    // test discovery
    let discovery = repros
        .get("test.discovery")
        .context("No `test.discovery` record")?;
    assert_eq!(
        discovery.get("identity").map(String::as_str),
        Some("fbcode//buck2/tests/targets/rules/go/test:test")
    );

    // test running
    let repro = repros.get("test.run").context("No `test.run` record")?;
    assert_eq!(repro.get("reason").map(String::as_str), Some("test.run"));
    assert_eq!(
        repro.get("identity").map(String::as_str),
        Some("fbcode//buck2/tests/targets/rules/go/test:test")
    );
    Ok(())
}

// TODO: This would be more reliable if it were an isolated test.
#[buck_test(inplace = true)]
async fn test_what_ran_local(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/genrule:mktemp";
    buck.build(&[target, "--no-remote-cache"]).await?;
    let out = buck.log(&["what-ran"]).await?;
    let out: Vec<&str> = out
        .stdout
        .lines()
        .filter(|line| line.contains(target))
        .map(str::trim)
        .collect();
    assert_eq!(out.len(), 1);

    assert!(out[0].contains("\tlocal\t"));
    assert!(!out[0].contains("\tre\t"));
    Ok(())
}
