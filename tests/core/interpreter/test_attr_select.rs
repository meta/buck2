/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::golden::sanitize_hashes;
use buck2_e2e::prelude::*;

/// Runs commands, recording each with its stdout, or its stderr if it fails.
struct Transcript<'a> {
    buck: &'a Buck,
    outputs: Vec<String>,
}

impl<'a> Transcript<'a> {
    fn new(buck: &'a Buck) -> Self {
        Self {
            buck,
            outputs: Vec::new(),
        }
    }

    async fn run(&mut self, args: &[&str]) -> Result<BuckOutput, BuckError> {
        self.outputs.push(format!("$ buck {}", args.join(" ")));
        let res = self.buck.command(args).await;
        match &res {
            Ok(out) => self.outputs.push(out.stdout.clone()),
            Err(e) => self.outputs.push(e.stderr.clone()),
        }
        res
    }

    fn output(&self) -> String {
        sanitize_hashes(&self.outputs.join("\n"))
    }
}

#[buck_test]
async fn test_select_fail(buck: &Buck) -> anyhow::Result<()> {
    let mut t = Transcript::new(buck);

    t.run(&[
        "cquery",
        "root//:foo",
        "-v0",
        "--target-platforms=//:platform1",
    ])
    .await?;
    t.run(&[
        "cquery",
        "root//:foo",
        "--console=none",
        "-v0",
        "--target-platforms=//:platform2",
    ])
    .await
    .expect_failure();
    t.run(&[
        "cquery",
        "root//:foo",
        "--console=none",
        "-v0",
        "--target-platforms=//:platform3",
    ])
    .await
    .expect_failure();

    t.run(&["uquery", "root//:foo", "-v0", "--output-format=starlark"])
        .await?;
    t.run(&["uquery", "root//:foo", "-v0", "--output-format=json", "-B"])
        .await?;

    t.run(&["uquery", "root//:foo2", "-v0", "--output-format=starlark"])
        .await?;
    t.run(&[
        "uquery",
        "root//:foo2",
        "--console=none",
        "-v0",
        "--output-format=starlark",
        "-c",
        "user.check_bad_coercion=1",
    ])
    .await
    .expect_failure();

    golden(&t.output(), "golden/test_select_fail.golden.stderr");
    Ok(())
}

#[buck_test]
async fn test_select_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let mut t = Transcript::new(buck);

    // Compatible platform: constraint1 matches, target resolves normally.
    t.run(&[
        "cquery",
        "root//:incompat_foo",
        "-v0",
        "--target-platforms=//:platform1",
    ])
    .await?;

    // Incompatible platform: constraint2 matches select_incompatible branch.
    // Target should be skipped (empty output), not error.
    t.run(&[
        "cquery",
        "root//:incompat_foo",
        "-v0",
        "--target-platforms=//:platform2",
    ])
    .await?;

    // Default branch is select_incompatible: target should be skipped.
    t.run(&[
        "cquery",
        "root//:incompat_foo",
        "-v0",
        "--target-platforms=//:platform3",
    ])
    .await?;

    // Build with incompatible platform and explicit target: fails with incompatible message
    // (same as target_compatible_with behavior for explicit targets).
    t.run(&[
        "build",
        "root//:incompat_foo",
        "--console=none",
        "-v0",
        "--target-platforms=//:platform2",
    ])
    .await
    .expect_failure();

    // Build with --skip-incompatible-targets: should skip gracefully.
    t.run(&[
        "build",
        "root//:incompat_foo",
        "-v0",
        "--target-platforms=//:platform2",
        "--skip-incompatible-targets",
    ])
    .await?;

    // uquery starlark output: select_incompatible values preserved.
    t.run(&[
        "uquery",
        "root//:incompat_foo",
        "-v0",
        "--output-format=starlark",
    ])
    .await?;

    // uquery json output: select_incompatible serialized.
    t.run(&[
        "uquery",
        "root//:incompat_foo",
        "-v0",
        "--output-format=json",
        "-B",
    ])
    .await?;

    // select_incompatible used outside select is a coercion error (same as select_fail).
    t.run(&[
        "uquery",
        "root//:incompat_foo2",
        "-v0",
        "--output-format=starlark",
    ])
    .await?;
    t.run(&[
        "uquery",
        "root//:incompat_foo2",
        "--console=none",
        "-v0",
        "--output-format=starlark",
        "-c",
        "user.check_bad_coercion_incompatible=1",
    ])
    .await
    .expect_failure();

    // Dep propagation: depends_on_incompat depends on incompat_foo.
    // On platform1 (compatible), both targets resolve normally.
    t.run(&[
        "cquery",
        "root//:depends_on_incompat",
        "-v0",
        "--target-platforms=//:platform1",
    ])
    .await?;

    // On platform2 (incompatible), incompat_foo is incompatible,
    // so depends_on_incompat should also be incompatible (dep-only).
    // This is currently a soft error upgraded to hard error via BUCK2_HARD_ERROR.
    t.run(&[
        "cquery",
        "root//:depends_on_incompat",
        "--console=none",
        "-v0",
        "--target-platforms=//:platform2",
    ])
    .await
    .expect_failure();

    golden(&t.output(), "golden/test_select_incompatible.golden.stderr");
    Ok(())
}
