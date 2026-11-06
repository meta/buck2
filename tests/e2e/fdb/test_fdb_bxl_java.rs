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

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use serde_json::Value;

/// What `fdb.bxl:inspect_target` writes to the file whose path it prints.
struct ExecInfo {
    data: Value,
}

impl ExecInfo {
    fn from_buck_result(result: &BuckOutput) -> anyhow::Result<Self> {
        let path = result.stdout.trim_matches('\n');
        let json: Value = serde_json::from_str(
            &std::fs::read_to_string(path).with_context(|| format!("Reading `{path}`"))?,
        )?;
        Ok(Self {
            data: json
                .get("data")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
        })
    }

    /// The names of the classes of the class map, whose path is relative to `root`, entry by entry.
    fn read_class_map_names(&self, root: &str) -> anyhow::Result<Vec<String>> {
        let classmap_file = self
            .data
            .get("java")
            .and_then(|java| java.get("classmap_file"))
            .and_then(Value::as_str)
            .context("No `java.classmap_file` in the exec info")?;
        assert!(
            !Path::new(classmap_file).is_absolute(),
            "{classmap_file} should be relative to {root}"
        );

        let classmap: Value = serde_json::from_str(&std::fs::read_to_string(
            Path::new(root).join(classmap_file),
        )?)?;
        let names = classmap
            .as_array()
            .context("The class map is not a list")?
            .iter()
            .flat_map(|entry| {
                entry
                    .get("classes")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
            })
            .map(|class_ref| {
                class_ref
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .with_context(|| format!("A class without a `name`: {class_ref}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(names)
    }
}

async fn project_root(buck: &Buck) -> anyhow::Result<String> {
    Ok(buck
        .root(&["--kind", "project"])
        .await?
        .stdout
        .trim_matches('\n')
        .to_owned())
}

async fn inspect_target(buck: &Buck, target: &str) -> anyhow::Result<ExecInfo> {
    let result = buck
        .bxl(&[
            "prelude//debugging/fdb.bxl:inspect_target",
            "--",
            "--target",
            target,
        ])
        .await?;
    ExecInfo::from_buck_result(&result)
}

/// The names in the class map of `target`.
async fn class_names(buck: &Buck, target: &str) -> anyhow::Result<Vec<String>> {
    let root = project_root(buck).await?;
    let exec_info = inspect_target(buck, target).await?;
    exec_info.read_class_map_names(&root)
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_java_test(buck: &Buck) -> anyhow::Result<()> {
    let names = class_names(
        buck,
        "//buck2/tests/targets/rules/java/java_test:simple_junit_test_java11",
    )
    .await?;
    assert_eq!(names, ["com.example.SimpleJUnitTest"]);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_java_binary(buck: &Buck) -> anyhow::Result<()> {
    let names = class_names(
        buck,
        "//buck2/tests/targets/rules/java/good/java_binary_with_native_libs:binary_with_native_lib",
    )
    .await?;
    assert_eq!(names, ["JavaBinaryWithNativeLibs"]);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_java_library(buck: &Buck) -> anyhow::Result<()> {
    let names = class_names(
        buck,
        "//buck2/tests/targets/rules/java/good/java_binary_with_native_libs:lib",
    )
    .await?;
    assert_eq!(names, ["JavaBinaryWithNativeLibs"]);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_kotlin_test(buck: &Buck) -> anyhow::Result<()> {
    let names = class_names(
        buck,
        "//buck2/tests/targets/rules/kotlin/kotlin_test:simple_kotlin_test",
    )
    .await?;
    assert_eq!(names, ["com.example.SimpleKotlinTest"]);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_kotlin_library(buck: &Buck) -> anyhow::Result<()> {
    let names = class_names(
        buck,
        "//buck2/tests/targets/rules/kotlin/kotlin_library:lib_with_source_only_abi_generation",
    )
    .await?;
    assert_eq!(names, ["A", "B"]);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_apk_gen_rule(buck: &Buck) -> anyhow::Result<()> {
    let names = class_names(
        buck,
        "fbsource//fbandroid/buck2/tests/good/apk:zip_align_basic_apk",
    )
    .await?;
    assert_eq!(
        names,
        [
            "com.example.sampleapp.MainActivity",
            "com.example.sampleapp.Helper",
            "com.example.sampleapp.Helper$SomeInterface",
        ]
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_instrumentation_test(buck: &Buck) -> anyhow::Result<()> {
    let exec_info = inspect_target(
        buck,
        "fbsource//fbandroid/buck2/tests/good/instrumentation_test:single_apk_test",
    )
    .await?;
    let program = exec_info
        .data
        .get("program")
        .and_then(Value::as_array)
        .context("No `program` in the exec info")?;
    assert!(program.iter().any(|arg| match arg {
        Value::String(arg) => arg.contains("args_file"),
        arg => arg.to_string().contains("args_file"),
    }));
    Ok(())
}

// This is to ensure at least one of the tests is passing on Windows otherwise CI fails
#[buck_test(inplace = true)]
async fn test_noop(_buck: &Buck) {}
