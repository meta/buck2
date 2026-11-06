/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;

// builds targets in an fbcode target configuration, unsupported on mac RE workers
#[cfg(target_os = "linux")]
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_swig_pp(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["fbcode//security/ca/lib:CAUtils-py-gen"])
        .await?;
    Ok(())
}

// builds targets in an fbcode target configuration, unsupported on mac RE workers
#[cfg(target_os = "linux")]
#[buck_test(inplace = true, env = [("BUCK2_KEEP_DEP_FILE_DIRECTORIES", "true")])]
async fn test_arvr_cuda_dep_files(buck: &Buck) -> anyhow::Result<()> {
    use regex::Regex;

    let target = "fbsource//arvr/tools/buck/tests/cuda:test_cuda_arvr";
    let mode_file = "@fbsource//arvr/mode/platform010/cuda13_3/opt";
    buck.build(&[mode_file, target]).await?;
    let res = buck
        .audit_dep_files(&[target, "cuda_compile", "main.cu", mode_file])
        .await?;
    let out = &res.stdout;
    let search = |pattern: &str| Regex::new(pattern).unwrap().is_match(out);

    // Check that we are tracking our dependency on stdlib headers, even
    // though they are neither explicitly included nor tagged.
    assert!(search(
        "untagged.*arvr/third-party/toolchains/platform010/build/glibc"
    ));

    // Check that we are tracking directly-included headers
    assert!(search("headers.*arvr/tools/buck/tests/cuda/direct_dep.h"));

    // Check that we are tracking transitively-included headers
    assert!(search(
        "headers.*arvr/tools/buck/tests/cuda/transitive_dep.h"
    ));

    // Check that we are not tracking irrelevant headers
    assert!(!search(
        "headers.*arvr/tools/buck/tests/cuda/unrelated_dep.h"
    ));
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_swig_pp_unit(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//tools/build/buck:swig_filter_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_windows_dummy() {
    // None of the tests in this file pass on Windows and that upsets CI.
}
