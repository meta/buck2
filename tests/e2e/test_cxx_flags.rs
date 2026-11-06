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
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;
use serde_json::Value;

const FIXTURE: &str = "fbcode//buck2/tests/targets/rules/cxx/cxx_flags";

/// The one output `--show-full-output` printed.
async fn single_output(buck: &Buck, target: &str) -> anyhow::Result<String> {
    let result = buck
        .build(&[
            target,
            "--show-full-output",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    let output_dict = result.target_to_build_output()?;
    assert_eq!(output_dict.len(), 1, "{output_dict:?}");
    Ok(output_dict.into_values().next().expect("one output"))
}

async fn compile_args(buck: &Buck, target: &str, source_name: &str) -> anyhow::Result<Vec<String>> {
    let output = single_output(buck, &format!("{target}[compilation-database]")).await?;
    let comp_db: Value = serde_json::from_str(&std::fs::read_to_string(output)?)?;
    let mut entries = Vec::new();
    for entry in comp_db.as_array().context("The comp-db is not a list")? {
        let file = entry
            .get("file")
            .and_then(Value::as_str)
            .context("A comp-db entry without a `file`")?;
        if file.ends_with(source_name) {
            entries.push(entry);
        }
    }
    assert_eq!(
        entries.len(),
        1,
        "expected one comp-db entry for {source_name}"
    );
    entries[0]
        .get("arguments")
        .and_then(Value::as_array)
        .context("A comp-db entry without `arguments`")?
        .iter()
        .map(|a| {
            a.as_str()
                .map(str::to_owned)
                .context("A non-string argument")
        })
        .collect()
}

async fn shared_cxx_flags_argsfile(buck: &Buck, target: &str) -> anyhow::Result<String> {
    let output = single_output(buck, &format!("{target}[argsfiles]")).await?;
    let target_argsfile = std::fs::read_to_string(output)?;
    let repo_root = buck.cwd().parent().context("The cell root has a parent")?;
    let mut paths = std::fs::read_to_string(repo_root.join(&target_argsfile))?
        .lines()
        .filter(|line| line.contains("_cxx_flags_argsfile_anon_rule"))
        .map(|line| {
            let words = shlex::split(line).with_context(|| format!("Splitting `{line}`"))?;
            let first = words
                .first()
                .with_context(|| format!("An empty line `{line}`"))?;
            Ok(first.strip_prefix('@').unwrap_or(first).to_owned())
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(
        paths.len(),
        1,
        "expected one cxx_flags argsfile in {target_argsfile}"
    );
    Ok(paths.remove(0))
}

#[track_caller]
fn assert_before(args: &[String], first: &str, second: &str) {
    let position = |arg: &str| args.iter().position(|a| a == arg);
    let first_index = position(first).unwrap_or_else(|| panic!("{first} not in compile command"));
    let second_index =
        position(second).unwrap_or_else(|| panic!("{second} not in compile command"));
    assert!(
        first_index < second_index,
        "expected {first} before {second} in {args:?}"
    );
}

fn count(args: &[String], arg: &str) -> usize {
    args.iter().filter(|a| *a == arg).count()
}

fn contains(args: &[String], arg: &str) -> bool {
    args.iter().any(|a| a == arg)
}

#[buck_test(inplace = true)]
async fn test_shared_flags_precede_target_flags_on_cxx_library(buck: &Buck) -> anyhow::Result<()> {
    let args = compile_args(buck, &format!("{FIXTURE}:lib"), "lib.cpp").await?;
    assert_before(
        &args,
        "-DCXX_FLAGS_BASE_COMPILER=1",
        "-DCXX_FLAGS_A_COMPILER=1",
    );
    assert_before(
        &args,
        "-DCXX_FLAGS_A_COMPILER=1",
        "-DCXX_FLAGS_B_COMPILER=1",
    );
    assert_before(
        &args,
        "-DCXX_FLAGS_B_COMPILER=1",
        "-DTARGET_COMPILER_FLAG=1",
    );
    assert_eq!(count(&args, "-DCXX_FLAGS_BASE_COMPILER=1"), 1);
    assert_before(
        &args,
        "-DCXX_FLAGS_A_LANG_CXX=1",
        "-DCXX_FLAGS_B_LANG_CXX=1",
    );
    assert_before(&args, "-DCXX_FLAGS_B_LANG_CXX=1", "-DTARGET_LANG_CXX=1");
    assert!(contains(&args, "-DCXX_FLAGS_A_PP=1"));
    assert!(contains(&args, "-DCXX_FLAGS_A_LANG_PP=1"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_no_shared_flags_means_no_shared_tokens(buck: &Buck) -> anyhow::Result<()> {
    let args = compile_args(buck, &format!("{FIXTURE}:lib_no_flags"), "lib.cpp").await?;
    assert!(contains(&args, "-DTARGET_COMPILER_FLAG=1"));
    for token in [
        "-DCXX_FLAGS_A_COMPILER=1",
        "-DCXX_FLAGS_B_COMPILER=1",
        "-DCXX_FLAGS_BASE_COMPILER=1",
        "-DCXX_FLAGS_A_LANG_CXX=1",
        "-DCXX_FLAGS_A_PP=1",
    ] {
        assert!(
            !contains(&args, token),
            "unexpected shared flag token {token}"
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_shared_flags_on_cxx_binary_and_cxx_test(buck: &Buck) -> anyhow::Result<()> {
    for target in [format!("{FIXTURE}:bin"), format!("{FIXTURE}:test")] {
        let args = compile_args(buck, &target, "main.cpp").await?;
        assert_before(
            &args,
            "-DCXX_FLAGS_A_COMPILER=1",
            "-DTARGET_COMPILER_FLAG=1",
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_repeated_flags_label_contributes_once(buck: &Buck) -> anyhow::Result<()> {
    let args = compile_args(buck, &format!("{FIXTURE}:lib_dup"), "lib.cpp").await?;
    assert_eq!(count(&args, "-DCXX_FLAGS_A_COMPILER=1"), 1);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_consumers_share_combined_cxx_flags_argsfile(buck: &Buck) -> anyhow::Result<()> {
    let lib_argsfile = shared_cxx_flags_argsfile(buck, &format!("{FIXTURE}:lib")).await?;
    let shared_argsfile = shared_cxx_flags_argsfile(buck, &format!("{FIXTURE}:lib_shared")).await?;
    assert_eq!(lib_argsfile, shared_argsfile);
    let repo_root = buck.cwd().parent().context("The cell root has a parent")?;
    let flags = std::fs::read_to_string(repo_root.join(&lib_argsfile))?;
    assert!(flags.contains("-DCXX_FLAGS_BASE_COMPILER=1"));
    assert!(flags.contains("-DCXX_FLAGS_A_COMPILER=1"));
    assert!(flags.contains("-DCXX_FLAGS_B_COMPILER=1"));
    assert!(!flags.contains("-DTARGET_COMPILER_FLAG=1"));
    assert!(!flags.contains("-DSHARED_TARGET_COMPILER_FLAG=1"));
    Ok(())
}

async fn shared_linker_flags_precede_target_linker_flags(
    buck: &Buck,
    target: &str,
) -> anyhow::Result<()> {
    let output = single_output(buck, &format!("{FIXTURE}:{target}[linker.argsfile]")).await?;
    let argsfile = std::fs::read_to_string(output)?;
    let shared_sentinel = "cxx_flags_a_linker_sentinel";
    let target_sentinel = "target_linker_sentinel";
    let shared_index = argsfile
        .find(shared_sentinel)
        .with_context(|| argsfile.clone())?;
    let target_index = argsfile
        .find(target_sentinel)
        .with_context(|| argsfile.clone())?;
    assert!(shared_index < target_index);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_shared_linker_flags_precede_target_linker_flags_bin(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_linker_flags_precede_target_linker_flags(buck, "bin").await
}

#[buck_test(inplace = true)]
async fn test_shared_linker_flags_precede_target_linker_flags_lib_shared_shared(
    buck: &Buck,
) -> anyhow::Result<()> {
    shared_linker_flags_precede_target_linker_flags(buck, "lib_shared[shared]").await
}

#[buck_test(inplace = true)]
async fn test_cython_flags_are_not_cxx_flags(buck: &Buck) -> anyhow::Result<()> {
    let output = single_output(
        buck,
        "fbcode//buck2/tests/targets/rules/python/cython_test:hello_cython",
    )
    .await?;
    assert!(output.ends_with("hello.cpp"), "{output}");
    assert!(std::fs::read_to_string(&output)?.contains("hello from cython"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_non_cxx_flags_dep_is_rejected(buck: &Buck) {
    buck.build(&[
        &format!("{FIXTURE}/bad:consumer[compilation-database]"),
        get_mode_from_platform(Mode::Dev),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("CxxFlagsInfo");
}

async fn assembly_language_flags(buck: &Buck, kind: &str, extension: &str) -> anyhow::Result<()> {
    let args = compile_args(
        buck,
        &format!("{FIXTURE}:assembly_{kind}"),
        &format!("assembly.{extension}"),
    )
    .await?;
    let (expected, unexpected) = if ["asm", "asmpp"].contains(&extension) {
        ("NASM", "GNU")
    } else {
        ("GNU", "NASM")
    };
    for category in ["COMPILER", "PREPROCESSOR"] {
        assert!(contains(&args, &format!("-D{expected}_{category}=1")));
        assert!(!contains(&args, &format!("-D{unexpected}_{category}=1")));
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_assembly_language_flags_gnu_s(buck: &Buck) -> anyhow::Result<()> {
    assembly_language_flags(buck, "s", "s").await
}

#[buck_test(inplace = true)]
async fn test_assembly_language_flags_gnu_sx(buck: &Buck) -> anyhow::Result<()> {
    assembly_language_flags(buck, "sx", "sx").await
}

#[buck_test(inplace = true)]
async fn test_assembly_language_flags_gnu_upper_s(buck: &Buck) -> anyhow::Result<()> {
    assembly_language_flags(buck, "upper_s", "S").await
}

#[buck_test(inplace = true)]
async fn test_assembly_language_flags_nasm(buck: &Buck) -> anyhow::Result<()> {
    assembly_language_flags(buck, "asm", "asm").await
}

#[buck_test(inplace = true)]
async fn test_assembly_language_flags_nasm_cpp(buck: &Buck) -> anyhow::Result<()> {
    assembly_language_flags(buck, "asmpp", "asmpp").await
}
