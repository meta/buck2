/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

fn read_text(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

fn read_rendered_inputs(script: &Path, origin: &Path) -> anyhow::Result<Vec<String>> {
    let text = read_text(script)?;
    let [inline, macro_, direct] = text.lines().collect::<Vec<_>>()[..] else {
        anyhow::bail!("Expected three lines in `{}`:\n{text}", script.display());
    };
    assert!(macro_.starts_with('@'));
    let macro_path = origin.join(&macro_[1..]);
    Ok(vec![
        read_text(&origin.join(inline))?,
        read_text(&origin.join(read_text(&macro_path)?))?,
        read_text(&origin.join(direct))?,
    ])
}

/// The paths of the inputs `inputs.bxl` lists for `key`.
fn inputs_of<'a>(inputs: &'a Value, key: &str) -> anyhow::Result<BTreeSet<&'a str>> {
    inputs[key]
        .as_array()
        .with_context(|| format!("No inputs for `{key}`"))?
        .iter()
        .map(|path| path.as_str().context("A non-string input"))
        .collect()
}

#[buck_test]
async fn test_writer_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//:mixed",
            "//:inline[primary]",
            "//:macro[primary]",
            "//:direct[primary]",
        ])
        .await?;
    let script = result.build_report()?.output_for_target("root//:mixed")?;
    assert_eq!(
        read_rendered_inputs(&script, buck.cwd())?,
        ["inline", "macro", "direct"]
    );

    let query = buck.bxl(&["//inputs.bxl:inputs"]).await?;
    let inputs: Value = serde_json::from_str(&query.stdout)?;
    let macro_ = inputs
        .as_object()
        .context("The inputs by path")?
        .keys()
        .find(|path| path.ends_with(".macro"))
        .context("No `.macro` in the inputs")?
        .as_str();
    assert_eq!(
        inputs_of(&inputs, "script")?,
        BTreeSet::from(["script", "inline.primary", "direct.primary", macro_])
    );
    assert_eq!(
        inputs_of(&inputs, macro_)?,
        BTreeSet::from([macro_, "macro.primary"])
    );
    assert_eq!(
        inputs_of(&inputs, "inputs.json")?,
        BTreeSet::from(["inputs.json", "inline.primary", "direct.primary"])
    );
    assert!(
        BTreeSet::from([
            "inline.associated",
            "inline.other",
            "macro.associated",
            "macro.other",
            "direct.associated",
            "hidden.primary",
        ])
        .is_subset(&inputs_of(&inputs, "run")?)
    );

    buck.build(&["//:mixed[run]"])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution resource requested");
    buck.build(&["//:retained"])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution resource requested");
    let result = buck.build(&["//:mixed[json]"]).await?;
    let output = match result
        .build_report()?
        .outputs_for_target("root//:mixed", "json")?
        .as_slice()
    {
        [output] => output.clone(),
        outputs => anyhow::bail!("`root//:mixed[json]` has {} outputs", outputs.len()),
    };
    let paths: Vec<String> = serde_json::from_str(&read_text(&output)?)?;
    assert_eq!(
        paths
            .iter()
            .map(|path| read_text(&buck.cwd().join(path)))
            .collect::<anyhow::Result<Vec<_>>>()?,
        ["inline", "direct"]
    );
    buck.build(&["//:retained[json]"])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution resource requested");
    Ok(())
}

#[buck_test]
async fn test_macro_writer_ignores_inline_inputs(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:macro_only[macros]"]).await?;
    buck.build(&["//:macro_only"])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution resource requested");
    Ok(())
}

#[buck_test]
async fn test_hidden_macro_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//:hidden_macro_writer",
            "//:inline[primary]",
            "//:macro[primary]",
            "//:direct[primary]",
        ])
        .await?;
    let script = result
        .build_report()?
        .output_for_target("root//:hidden_macro_writer")?;
    assert_eq!(
        read_rendered_inputs(&script, buck.cwd())?,
        ["inline", "macro", "direct"]
    );
    Ok(())
}

#[buck_test]
async fn test_relative_macro_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//:relative",
            "//:origin",
            "//:inline[primary]",
            "//:macro[primary]",
            "//:direct[primary]",
        ])
        .await?;
    let report = result.build_report()?;
    let script = report.output_for_target("root//:relative")?;
    let origin = report.output_for_target("root//:origin")?;
    let origin = origin.parent().context("An output in a directory")?;
    assert_eq!(
        read_rendered_inputs(&script, origin)?,
        ["inline", "macro", "direct"]
    );
    Ok(())
}
