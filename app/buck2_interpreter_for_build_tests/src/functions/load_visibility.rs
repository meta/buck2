/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::expect_error;

fn eval_bzl(content: &str) -> buck2_error::Result<()> {
    let mut t = Tester::new()?;
    t.add_import(&ImportPath::testing_new("root//pkg:defs.bzl"), content)?;
    Ok(())
}

fn expect_bzl_error(content: &str, expected: &str) {
    expect_error(eval_bzl(content), content, expected);
}

#[test]
fn test_visibility_accepts_package_patterns() -> buck2_error::Result<()> {
    eval_bzl(r#"visibility(["//foo:", "//bar/...", "root//baz:"])"#)?;
    eval_bzl(r#"visibility("public")"#)?;
    eval_bzl(r#"visibility(["PUBLIC"])"#)?;
    eval_bzl(r#"visibility("private")"#)?;
    eval_bzl(r#"visibility([])"#)?;
    Ok(())
}

#[test]
fn test_visibility_rejects_target_label() {
    expect_bzl_error(
        r#"visibility(["//foo:bar"])"#,
        "`//foo:bar` is a target label, but `visibility()` takes package patterns (`//pkg:` or `//pkg/...`)",
    );
}

#[test]
fn test_visibility_rejects_missing_colon() {
    expect_bzl_error(
        r#"visibility(["//foo"])"#,
        "`//foo` is not a package pattern; `visibility()` takes package patterns (`//pkg:` or `//pkg/...`)",
    );
}

#[test]
fn test_visibility_rejects_wrong_case() {
    expect_bzl_error(
        r#"visibility("Public")"#,
        "`\"Public\"` is not valid in `visibility()`; use `\"PUBLIC\"` or `\"public\"`",
    );
    expect_bzl_error(
        r#"visibility("PRIVATE")"#,
        "`\"PRIVATE\"` is not valid in `visibility()`; use `\"private\"`",
    );
}

#[test]
fn test_visibility_rejects_negative_pattern() {
    expect_bzl_error(
        r#"visibility(["//...", "-//x/..."])"#,
        "Negative patterns are not supported in `visibility()`: `-//x/...`",
    );
}

#[test]
fn test_visibility_must_be_top_level() {
    expect_bzl_error(
        r#"
def _set():
    visibility("private")
_set()
"#,
        "`visibility()` must be called at the top level of the `.bzl` file it applies to",
    );
}

#[test]
fn test_visibility_not_from_loaded_helper() -> buck2_error::Result<()> {
    let mut t = Tester::new()?;
    t.add_import(
        &ImportPath::testing_new("root//helper:helper.bzl"),
        r#"
def set_visibility():
    visibility("public")
"#,
    )?;
    let content = r#"
load("//helper:helper.bzl", "set_visibility")
set_visibility()
"#;
    expect_error(
        t.add_import(&ImportPath::testing_new("root//pkg:defs.bzl"), content),
        content,
        "`visibility()` must be called at the top level of the `.bzl` file it applies to",
    );
    Ok(())
}

#[test]
fn test_visibility_at_most_once() {
    expect_bzl_error(
        r#"
visibility("public")
visibility("public")
"#,
        "`visibility()` can be called at most once per `.bzl` file",
    );
}
