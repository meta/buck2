/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;
use serde_json::json;

/// The JSON that a fixture prints after `DATA_LOAD_TEST_OUTPUT: `.
fn extract_test_output(stderr: &str) -> anyhow::Result<Value> {
    let marker = "DATA_LOAD_TEST_OUTPUT: ";
    let output = stderr
        .lines()
        .find_map(|line| line.find(marker).map(|idx| &line[idx + marker.len()..]))
        .with_context(|| format!("Could not find '{marker}' in stderr:\n{stderr}"))?;
    serde_json::from_str(output).with_context(|| format!("Parsing `{output}`"))
}

fn package_names(lock: &Value) -> Vec<&str> {
    lock["package"]
        .as_array()
        .expect("`package` is a list")
        .iter()
        .map(|p| p["name"].as_str().expect("`name` is a string"))
        .collect()
}

/// The hint, not the extension, picks the parser: `deps.lock` is TOML and `metadata.txt` is JSON.
#[buck_test(data_dir = "format")]
async fn test_load_as_selects_the_parser(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.targets(&["root//:"]).await?;
    let data = extract_test_output(&result.stderr)?;

    assert_eq!(data["lock"]["version"], 3);
    assert_eq!(package_names(&data["lock"]), ["serde", "toml"]);
    assert_eq!(data["metadata"], json!({"name": "example", "version": 1}));
    Ok(())
}

/// The point of carrying the format on `ImportPath` is that the loaded file is a dependency of the
/// parse, so editing it invalidates the build file.
#[buck_test(data_dir = "format")]
async fn test_load_as_files_are_tracked_dependencies(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["root//:"]).await?;

    std::fs::write(
        buck.cwd().join("deps.lock"),
        "version = 4\n\n[[package]]\nname = \"anyhow\"\n",
    )?;
    std::fs::write(
        buck.cwd().join("metadata.txt"),
        "{\"name\": \"renamed\", \"version\": 2}\n",
    )?;

    let result = buck.targets(&["root//:"]).await?;
    let data = extract_test_output(&result.stderr)?;

    assert_eq!(data["lock"]["version"], 4);
    assert_eq!(package_names(&data["lock"]), ["anyhow"]);
    assert_eq!(data["metadata"], json!({"name": "renamed", "version": 2}));
    Ok(())
}

/// An unparseable file fails the whole package at load time. Because the extension no longer
/// implies the parser, the error has to say which one ran and where that choice came from.
#[buck_test(data_dir = "format")]
async fn test_load_as_parse_error_names_the_format(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("deps.lock"), "this is not valid toml {{{\n")?;

    buck.targets(&["root//:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Parsing `root//deps\.lock` as toml, as requested by `\?as=toml`");
    Ok(())
}

/// The extension is the reader's only clue as to which parser ran, and it is an easy one to
/// misread, so name the parser even when nothing overrode it.
#[buck_test(data_dir = "redundant")]
async fn test_parse_error_names_the_format_without_a_hint(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("data.toml"), "this is not valid toml {{{\n")?;

    let output = buck.targets(&["root//:"]).await.expect_failure();
    output.assert_stderr_matches(r"Parsing `root//data\.toml` as toml");
    // Nothing overrode the format, so nothing follows the parser name.
    assert!(
        !output.stderr.contains("as requested by"),
        "{}",
        output.stderr
    );
    Ok(())
}

/// An unrecognised hint must be reported as such, rather than being left on the path to resurface
/// as a confusing complaint about the path itself.
#[buck_test(data_dir = "format")]
async fn test_load_as_rejects_an_unknown_format(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join("TARGETS.fixture"),
        "load(\":metadata.txt?as=zorblax\", \"value\")\n",
    )?;

    buck.targets(&["root//:"])
        .await
        .expect_failure()
        .assert_stderr_matches("unknown import format `zorblax`");
    Ok(())
}

/// `?as=` names a format the extension cannot. Starlark files can already say so with `.bzl`, and
/// accepting `?as=bzl` would make any extension, including `.bxl`, which `load()` may not reach,
/// evaluable as code.
#[buck_test(data_dir = "format")]
async fn test_load_as_bzl_is_rejected(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join("TARGETS.fixture"),
        "load(\":deps.lock?as=bzl\", \"value\")\n",
    )?;

    buck.targets(&["root//:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"import format `bzl` cannot be requested with `\?as=`");
    Ok(())
}

#[buck_test(data_dir = "format")]
async fn test_load_without_a_hint_still_requires_a_known_extension(
    buck: &Buck,
) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join("TARGETS.fixture"),
        "load(\":metadata.txt\", \"value\")\n",
    )?;

    buck.targets(&["root//:"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"must have suffix `\.bzl`, `\.json`, or `\.toml`");
    Ok(())
}

/// `audit includes` walks the loaded modules of a build file, so it meets data files. It has to ask
/// each path how it is parsed rather than assume Starlark, or it reports a syntax error inside the
/// data file.
#[buck_test(data_dir = "format")]
async fn test_audit_includes_lists_data_loads(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["includes", "TARGETS.fixture"]).await?;

    assert!(result.stdout.contains("deps.lock"), "{}", result.stdout);
    assert!(result.stdout.contains("metadata.txt"), "{}", result.stdout);
    Ok(())
}

/// `audit starlark module` on a data file must parse it as data. The load here carries no hint, so
/// nothing but the extension says it isn't Starlark.
#[buck_test(data_dir = "redundant")]
async fn test_audit_starlark_module_parses_a_data_file(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["starlark", "module", "root//:data.toml"])
        .await?;

    assert!(
        result.stdout.contains("root//data.toml"),
        "{}",
        result.stdout
    );
    Ok(())
}
