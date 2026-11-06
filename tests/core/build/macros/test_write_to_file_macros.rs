/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use buck2_e2e::prelude::*;
use regex::Regex;

fn normalize_path(p: &str) -> String {
    static HASH16: LazyLock<Regex> = LazyLock::new(|| Regex::new("/([a-f0-9]{16})/").unwrap());
    static HASH40: LazyLock<Regex> = LazyLock::new(|| Regex::new("/([a-f0-9]{40})/").unwrap());
    let p = p.replace('\\', "/");
    let p = HASH16.replace_all(&p, "/<HASH>/");
    HASH40.replace_all(&p, "/<HASH>/").into_owned()
}

/// The one file named `name` under `dir`, without following symlinks to directories.
fn find_file(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    fn walk(dir: &Path, name: &str, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(&path, name, files)?;
            } else if !path.is_dir() && entry.file_name() == name {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, name, &mut files)?;
    match <[PathBuf; 1]>::try_from(files) {
        Ok([f]) => Ok(f),
        Err(files) => anyhow::bail!("Expected one `{name}`, found {files:?}"),
    }
}

async fn xxx(buck: &Buck, fingerprint: &str) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//:test_rule",
            "-c",
            &format!("test.dep_files_fingerprint_using_canonical_paths={fingerprint}"),
            "-c",
            // The Python harness passed its own interpreter; the fixture's other actions run
            // `fbpython`.
            "test.python=fbpython",
        ])
        .await?;
    let out = result
        .build_report()?
        .output_for_target("root//:test_rule")?;

    // Out contents is:
    // ```
    // @buck-out/v2/art/root/6dd044292ff31ae1/__test_rule__/__macros/1e0e364a22c69340e6f02604520fdeb7674264c0/0.macro
    // @../__macros/1e0e364a22c69340e6f02604520fdeb7674264c0/1.macro
    // @../__macros/1e0e364a22c69340e6f02604520fdeb7674264c0/2.macro
    // @../__macros/1e0e364a22c69340e6f02604520fdeb7674264c0/3.macro
    // ```

    let text = std::fs::read_to_string(&out)?;
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.trim().replace('\\', "/"))
        .collect();
    let [a, b, c, d] = &lines[..] else {
        anyhow::bail!("Expected four lines:\n{text}");
    };

    assert_eq!(
        "@buck-out/v2/art/root/<HASH>/__test_rule__/__macros/<HASH>/0.macro",
        normalize_path(a)
    );
    assert_eq!("@../__macros/<HASH>/1.macro", normalize_path(b));
    assert_eq!("@../__macros/<HASH>/2.macro", normalize_path(c));
    assert_eq!("@../__macros/<HASH>/3.macro", normalize_path(d));

    let buck_out = buck.cwd().join("buck-out");
    let a_x = find_file(&buck_out, "0.macro")?;
    let a_contents = normalize_path(&std::fs::read_to_string(&a_x)?);
    assert_eq!(
        "buck-out/v2/art/root/<HASH>/__write_file__/write_file.txt",
        a_contents
    );

    // TODO(nga): contents of `{1,2,3}.macro` should be identical.

    let b_x = find_file(&buck_out, "1.macro")?;
    let b_contents = normalize_path(&std::fs::read_to_string(&b_x)?);
    assert_eq!(
        "buck-out/v2/art/root/<HASH>/__write_file__/write_file.txt",
        b_contents
    );

    let c_x = find_file(&buck_out, "2.macro")?;
    let c_contents = normalize_path(&std::fs::read_to_string(&c_x)?);
    assert_eq!("../../__write_file__/write_file.txt", c_contents);

    let d_x = find_file(&buck_out, "3.macro")?;
    let d_contents = normalize_path(&std::fs::read_to_string(&d_x)?);
    assert_eq!("../../__write_file__/write_file.txt", d_contents);
    Ok(())
}

#[buck_test(setup_eden = true)]
async fn test_xxx_true(buck: &Buck) -> anyhow::Result<()> {
    xxx(buck, "true").await
}

#[buck_test(setup_eden = true)]
async fn test_xxx_false(buck: &Buck) -> anyhow::Result<()> {
    xxx(buck, "false").await
}

/// The one output of `target`'s `sub_target`.
fn sub_target_output(
    result: &buck2_e2e::BuckOutput,
    target: &str,
    sub_target: &str,
) -> anyhow::Result<PathBuf> {
    match <[PathBuf; 1]>::try_from(
        result
            .build_report()?
            .outputs_for_target(target, sub_target)?,
    ) {
        Ok([output]) => Ok(output),
        Err(outputs) => anyhow::bail!("`{target}[{sub_target}]` has outputs {outputs:?}"),
    }
}

async fn macro_writer_ignores_inline_inputs(buck: &Buck, tagged: bool) -> anyhow::Result<()> {
    let target = if tagged {
        "macro_only_tagged"
    } else {
        "macro_only"
    };
    let result = buck
        .build(&[&format!("//:{target}[macros]"), "//:payload"])
        .await?;
    let macro_ = sub_target_output(&result, &format!("root//:{target}"), "macros")?;
    let payload = result.build_report()?.output_for_target("root//:payload")?;
    assert_eq!(
        std::fs::canonicalize(buck.cwd().join(std::fs::read_to_string(&macro_)?))?,
        std::fs::canonicalize(payload)?
    );
    buck.build(&[&format!("//:{target}")])
        .await
        .expect_failure()
        .assert_stderr_matches("Unrelated input requested");
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_macro_writer_ignores_inline_inputs_False(buck: &Buck) -> anyhow::Result<()> {
    macro_writer_ignores_inline_inputs(buck, false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_macro_writer_ignores_inline_inputs_True(buck: &Buck) -> anyhow::Result<()> {
    macro_writer_ignores_inline_inputs(buck, true).await
}

async fn hidden_macro_inputs(buck: &Buck, tagged: bool) -> anyhow::Result<()> {
    let target = if tagged {
        "hidden_macro_tagged"
    } else {
        "hidden_macro"
    };
    let result = buck
        .build(&[&format!("//:{target}[macros]"), "//:payload"])
        .await?;
    let macro_ = sub_target_output(&result, &format!("root//:{target}"), "macros")?;
    let payload = result.build_report()?.output_for_target("root//:payload")?;
    assert_eq!(
        std::fs::canonicalize(buck.cwd().join(std::fs::read_to_string(&macro_)?))?,
        std::fs::canonicalize(payload)?
    );
    Ok(())
}

#[buck_test]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_hidden_macro_inputs_False(buck: &Buck) -> anyhow::Result<()> {
    hidden_macro_inputs(buck, false).await
}

#[buck_test]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_hidden_macro_inputs_True(buck: &Buck) -> anyhow::Result<()> {
    hidden_macro_inputs(buck, true).await
}

async fn tagged_hidden_writer_inputs(buck: &Buck, target: &str) -> anyhow::Result<()> {
    let result = buck.build(&[&format!("//:{target}[script]")]).await?;
    let script = sub_target_output(&result, &format!("root//:{target}"), "script")?;
    assert_eq!(std::fs::read_to_string(&script)?, "payload");
    let result = buck.build(&[&format!("//:{target}[json]")]).await?;
    let output = sub_target_output(&result, &format!("root//:{target}"), "json")?;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&output)?)?,
        serde_json::json!(["payload"])
    );
    for retained in [
        format!("{target}[run]"),
        format!("{target}_retained[script]"),
        format!("{target}_retained[json]"),
    ] {
        buck.build(&[&format!("//:{retained}")])
            .await
            .expect_failure()
            .assert_stderr_matches("Unrelated input requested");
    }
    Ok(())
}

#[buck_test]
async fn test_tagged_hidden_writer_inputs_tagged(buck: &Buck) -> anyhow::Result<()> {
    tagged_hidden_writer_inputs(buck, "tagged").await
}

#[buck_test]
async fn test_tagged_hidden_writer_inputs_tagged_inputs(buck: &Buck) -> anyhow::Result<()> {
    tagged_hidden_writer_inputs(buck, "tagged_inputs").await
}

async fn tagged_hidden_macro_payload_inputs(buck: &Buck, target: &str) -> anyhow::Result<()> {
    let result = buck.build(&[&format!("//:{target}[macros]")]).await?;
    let output = sub_target_output(&result, &format!("root//:{target}"), "macros")?;
    assert_eq!(std::fs::read_to_string(&output)?, "payload");
    Ok(())
}

#[buck_test]
async fn test_tagged_hidden_macro_payload_inputs_tagged_macro_payload(
    buck: &Buck,
) -> anyhow::Result<()> {
    tagged_hidden_macro_payload_inputs(buck, "tagged_macro_payload").await
}

#[buck_test]
async fn test_tagged_hidden_macro_payload_inputs_tagged_inputs_macro_payload(
    buck: &Buck,
) -> anyhow::Result<()> {
    tagged_hidden_macro_payload_inputs(buck, "tagged_inputs_macro_payload").await
}
