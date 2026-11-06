/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::strip_glog_lines;
use buck2_e2e::golden::strip_jemalloc_invalid_conf;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_digest;
use buck2_e2e::utils::replace_hash;

fn sanitize(s: &str) -> String {
    let s = strip_jemalloc_invalid_conf(&strip_glog_lines(s));
    // Remove configuration hashes
    let s = replace_hash(&s);
    // And action digests
    replace_digest(&s)
}

#[derive(Clone, Copy)]
enum Command {
    Build,
    Cquery,
    Bxl,
}

async fn error_formatting_test(buck: &Buck, name: &str, command: &[&str], command_name: Command) {
    let args = [&["--console=none"], command].concat();
    let res = match command_name {
        Command::Build => buck.build(&args),
        Command::Cquery => buck.cquery(&args),
        Command::Bxl => buck.bxl(&args),
    }
    .await
    .expect_failure();
    golden(
        &sanitize(&res.stderr),
        &format!("fixtures/{name}.golden.stderr"),
    );
}

#[buck_test]
async fn test_action_fail(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_action_fail",
        &["//:action_fail"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_missing_dep(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_missing_dep",
        &["//:missing_dep"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_missing_dep_cquery(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_missing_dep_cquery",
        &["//:missing_dep"],
        Command::Cquery,
    )
    .await;
}

#[buck_test]
async fn test_attr_coercion(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_attr_coercion",
        &["//attr_coercion:int_rule"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_during_load(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_during_load",
        &["//during_load:whatever"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_during_load_via_dep(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_during_load_via_dep",
        &["//during_load/via_dep:via_dep"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_during_parse(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_during_parse",
        &["//during_parse:whatever"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_during_select_map(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_during_select_map",
        &["//during_select:map"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_bxl_no_stacktrace(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_bxl_no_stacktrace",
        &["//fail_no_stacktrace.bxl:fail_no_stacktrace_test"],
        Command::Bxl,
    )
    .await;
}

#[buck_test]
async fn test_bxl_no_stacktrace_verbose(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_bxl_no_stacktrace_verbose",
        &["//fail_no_stacktrace.bxl:fail_no_stacktrace_test", "-v5"],
        Command::Bxl,
    )
    .await;
}

#[buck_test]
async fn test_bxl_with_stacktrace(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_bxl_with_stacktrace",
        &["//fail_no_stacktrace.bxl:fail_with_stacktrace_test"],
        Command::Bxl,
    )
    .await;
}

#[buck_test]
async fn test_bxl_attr_coercion(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_bxl_attr_coercion",
        &["//fail_attr_coercion.bxl:int_rule"],
        Command::Bxl,
    )
    .await;
}

#[buck_test]
async fn test_duplicate_target(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_duplicate_target",
        &["//duplicate_target:foo"],
        Command::Build,
    )
    .await;
}

#[buck_test]
async fn test_duplicate_target_with_stacktrace(buck: &Buck) {
    error_formatting_test(
        buck,
        "test_duplicate_target_with_stacktrace",
        &["//duplicate_target:foo", "--stack"],
        Command::Build,
    )
    .await;
}
