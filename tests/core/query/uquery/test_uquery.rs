/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! If you need to add a directory that's isolated in buck2/test/targets
//! (ex. some test of form @buck_test( data_dir=some_new_directory)),
//! then you will need to update isolated_targets in buck2/test/targets/TARGETS.
//! Otherwise the test will fail because it cannot recognize the new directory.

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::manifold::Bucket;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_hash;

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_none(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["none"])
        .await
        .expect_failure()
        .assert_stderr_matches("Error parsing target pattern `none`");

    buck.uquery(&["None"])
        .await
        .expect_failure()
        .assert_stderr_matches("expected value of type `targets`, got `None`:");

    let result = buck.uquery(&[":none"]).await?;
    assert_eq!(result.stdout, "root//:none\n");

    let result = buck.uquery(&[":None"]).await?;
    assert_eq!(result.stdout, "root//:None\n");

    let result = buck.uquery(&["':none'"]).await?;
    assert_eq!(result.stdout, "root//:none\n");

    let result = buck.uquery(&["':None'"]).await?;
    assert_eq!(result.stdout, "root//:None\n");

    buck.uquery(&["set(none)"])
        .await
        .expect_failure()
        .assert_stderr_matches("Error parsing target pattern `none`");

    buck.uquery(&["set(None)"])
        .await
        .expect_failure()
        // .assert_stderr_matches("expected value of type `targets`, got `None`:");
        .assert_stderr_matches("Error parsing target pattern `None`");

    buck.uquery(&["set('none')"])
        .await
        .expect_failure()
        .assert_stderr_matches("Error parsing target pattern `none`");

    buck.uquery(&["set('None')"])
        .await
        .expect_failure()
        .assert_stderr_matches("Error parsing target pattern `None`");

    buck.uquery(&["filter('', none)"])
        .await
        .expect_failure()
        .assert_stderr_matches("Error parsing target pattern `none`");

    buck.uquery(&["filter('', None)"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"None is not a valid value for function `filter` argument \[1\] `set: \*target or file expression\*`",
        );

    let result = buck.uquery(&["filter(none, :none)"]).await?;
    assert_eq!(result.stdout, "root//:none\n");

    buck.uquery(&["filter(None, :None)"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"None is not a valid value for function `filter` argument \[0\] `regex: \*string\*`",
        );

    let result = buck.uquery(&["filter('none', :none)"]).await?;
    assert_eq!(result.stdout, "root//:none\n");

    let result = buck.uquery(&["filter('None', :None)"]).await?;
    assert_eq!(result.stdout, "root//:None\n");

    let result = buck.uquery(&["filter(none, ':none')"]).await?;
    assert_eq!(result.stdout, "root//:none\n");

    buck.uquery(&["filter(None, ':None')"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"None is not a valid value for function `filter` argument \[0\] `regex: \*string\*`",
        );

    let result = buck.uquery(&["filter('none', ':none')"]).await?;
    assert_eq!(result.stdout, "root//:none\n");

    let result = buck.uquery(&["filter('None', ':None')"]).await?;
    assert_eq!(result.stdout, "root//:None\n");

    buck.uquery(&["none()"])
        .await
        .expect_failure()
        .assert_stderr_matches("unknown function `none`:");
    buck.uquery(&["None()"])
        .await
        .expect_failure()
        .assert_stderr_matches("in Eof");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["inputs(set(root//bin:the_binary //lib:file1))"])
        .await?;
    assert_eq!(result.stdout, "bin/TARGETS.fixture\n");

    let result = buck.uquery(&["inputs(set())"]).await?;
    assert_eq!(result.stdout, "");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_union(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["deps(root//lib:lib1) + set(root//data:data)"])
        .await?;
    assert_eq!(
        result.stdout,
        "root//lib:file1\nroot//lib:lib1\nroot//data:data\n"
    );

    let result = buck
        .uquery(&["buildfile(root//bin:the_binary) + inputs(deps(root//lib:lib1))"])
        .await?;
    assert_eq!(result.stdout, "bin/TARGETS.fixture\nlib/TARGETS.fixture\n");

    let result = buck
        .uquery(&["'root//bin:the_binary' + set(root//data:data)"])
        .await?;
    assert_eq!(result.stdout, "root//bin:the_binary\nroot//data:data\n");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_owner(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.uquery(&["owner(bin/TARGETS.fixture)"]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "root//bin:the_binary\n");

    let result = buck.uquery(&["owner(data/buck/build/data.file)"]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "root//data:data\n");

    // there's no buildfile in the root of the special buck, make sure that works
    std::fs::write(buck.cwd().join("special/file"), "")?;
    let result = buck.uquery(&["owner(special/file)"]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");

    // there's a buildfile here, but no target owns the file
    let result = buck.uquery(&["owner(.buckconfig)"]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");

    let result = buck
        .uquery(&["owner(../data/buck/build/data.file)"])
        .rel_cwd("special")
        .await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "root//data:data\n");

    let result = buck.uquery(&["owner(root//bin/TARGETS.fixture)"]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "root//bin:the_binary\n");
    Ok(())
}

async fn log_show(buck: &Buck) -> anyhow::Result<String> {
    Ok(buck.log(&["show"]).await?.stdout)
}

#[buck_test(data_dir = "bxl_simple", allow_soft_errors = true)]
async fn test_uquery_owner_missing_file(buck: &Buck) -> anyhow::Result<()> {
    let absolute = buck
        .cwd()
        .join("data")
        .join("buck")
        .join("build")
        .join("missing.file")
        .to_string_lossy()
        .into_owned();
    for path in [
        "data/buck/build/missing.file",
        "missing/file",
        "root//data/buck/build/missing.file",
        &absolute,
    ] {
        let result = buck.uquery(&[&format!("owner('{path}')")]).await?;
        assert_eq!(result.exit_code(), Some(0));
        assert_eq!(result.stdout, "");
        assert!(log_show(buck).await?.contains("owner_invalid_path"));
    }

    let result = buck
        .uquery(&["owner(data/buck/build/data.file)"])
        .rel_cwd("special")
        .await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    let log = log_show(buck).await?;
    assert!(log.contains("owner_invalid_path"));
    assert!(log.contains("File not found: special//data/buck/build/data.file"));
    Ok(())
}

#[buck_test(data_dir = "bxl_simple", allow_soft_errors = true)]
async fn test_uquery_owner_mixed_existing_and_missing_files(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["owner(fileset(bin/TARGETS.fixture missing/file))"])
        .await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "root//bin:the_binary\n");
    assert!(log_show(buck).await?.contains("owner_invalid_path"));
    Ok(())
}

#[buck_test(data_dir = "bxl_simple", allow_soft_errors = true)]
async fn test_uquery_owner_file_created(buck: &Buck) -> anyhow::Result<()> {
    let path = buck.cwd().join("data/buck/build/new.file");
    let query = "owner(data/buck/build/new.file)";
    let result = buck.uquery(&[query]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    assert!(log_show(buck).await?.contains("owner_invalid_path"));

    std::fs::write(&path, "")?;
    let result = buck.uquery(&[query]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "root//data:data\n");
    assert!(!log_show(buck).await?.contains("owner_invalid_path"));

    std::fs::remove_file(&path)?;
    let result = buck.uquery(&[query]).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    assert!(log_show(buck).await?.contains("owner_invalid_path"));
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_query_owner_with_explicit_package_boundary_violation(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["owner(package_boundary_violation/bin)"])
        .await?;
    assert!(
        result
            .stdout
            .contains("root//package_boundary_violation:bin")
    );
    assert!(result.stdout.contains("root//:package_boundary_violation"));
    Ok(())
}

#[buck_test(data_dir = "bxl_simple", allow_soft_errors = true)]
async fn test_uquery_buildfile(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.uquery(&["buildfile(root//bin:the_binary)"]).await?;
    assert_eq!(result.stdout, "bin/TARGETS.fixture\n");

    let result = buck
        .uquery(&["buildfile(root//bin: + root//data:)"])
        .await?;
    assert_eq!(result.stdout, "bin/TARGETS.fixture\ndata/TARGETS.fixture\n");

    let result = buck
        .uquery(&["buildfile(owner(../data/buck/build/data.file))"])
        .rel_cwd("special")
        .await?;
    assert_eq!(result.stdout, "data/TARGETS.fixture\n");
    Ok(())
}

/// `lines`, each followed by a newline.
fn lines(lines: &[&str]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_targets_in_buildfile(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["targets_in_buildfile(bin/TARGETS.fixture)"])
        .await?;
    assert_eq!(
        result.stdout,
        lines(&[
            "root//bin:setting",
            "root//bin:my_config",
            "root//bin:my_platform",
            "root//bin:the_binary",
            "root//bin:the_binary_with_dir_srcs",
            "root//bin:platform",
        ])
    );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_query_configuration_deps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["deps(root//bin:the_binary, 1, configuration_deps())"])
        .await?;
    assert!(result.stdout.contains("root//bin:my_config"));
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_deps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.uquery(&["deps(root//bin:the_binary)"]).await?;
    assert_eq!(
        result.stdout,
        lines(&[
            "root//:foo_toolchain",
            "root//:bin",
            "root//lib:file3",
            "root//lib:lib3",
            "root//lib:file2",
            "root//lib:lib2",
            "root//lib:file1",
            "root//lib:lib1",
            "root//:genrule_binary",
            "root//:data",
            "root//bin:the_binary",
        ])
    );

    let target_deps_expr = "deps(root//bin:the_binary, 100, target_deps())";

    let result = buck.uquery(&[target_deps_expr]).await?;
    assert_eq!(
        result.stdout,
        lines(&[
            "root//bin:the_binary",
            "root//:data",
            "root//lib:lib1",
            "root//lib:lib2",
            "root//lib:lib3",
            "root//lib:file1",
            "root//lib:file2",
            "root//lib:file3",
        ])
    );

    // this is a little subtle, query's deps() function always forms a graph
    // with the nodes themselves so we subtract them out. It's not quite right
    // if a node in the graph of target deps were to have an exec dep on another.
    let result = buck
        .uquery(&[&format!(
            "deps({target_deps_expr}, 1, exec_deps()) - {target_deps_expr}"
        )])
        .await?;
    assert_eq!(
        result.stdout,
        lines(&[
            "root//:foo_toolchain",
            "root//:bin",
            "root//:genrule_binary"
        ])
    );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_cell(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.uquery(&["//stuff:magic"]).rel_cwd("special").await?;
    assert_eq!(result.stdout, "special//stuff:magic\n");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_relative(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.uquery(&["..."]).rel_cwd("special").await?;
    assert_eq!(result.stdout, "special//stuff:magic\n");
    let result = buck.uquery(&["..."]).rel_cwd("bin").await?;
    assert!(result.stdout.contains("root//bin:the_binary\n"));
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_provider_names(buck: &Buck) -> anyhow::Result<()> {
    // FIXME(JakobDegen): Providers in a query literal should be an error; they are dropped.
    let result = buck
        .uquery(&["'root//bin:the_binary[provider_name]'"])
        .await?;
    assert_eq!(result.stdout, "root//bin:the_binary\n");

    buck.uquery(&["'root//bin:the_binary#some_flavor'"])
        .await
        .expect_failure()
        // Soft error, upgraded to a hard error by the test harness.
        .assert_stderr_matches(
            "buck1 flavor `#some_flavor` on `root//bin:the_binary#some_flavor` is deprecated",
        );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_query_filter(buck: &Buck) -> anyhow::Result<()> {
    // Test uquery/cquery on target and file sets
    let out = buck.uquery(&["filter('the_binary$', root//...)"]).await?;
    assert_eq!(out.stdout, "root//bin:the_binary\n");
    let out = buck.cquery(&["filter('the_binary\\w', root//...)"]).await?;
    assert_eq!(
        replace_hash(&out.stdout),
        "root//bin:the_binary_with_dir_srcs (root//platforms:platform1#<HASH>)\n"
    );
    let out = buck
        .uquery(&["filter('fixture$', inputs(root//bin:the_binary))"])
        .await?;
    assert_eq!(out.stdout, "bin/TARGETS.fixture\n");
    let out = buck
        .cquery(&["filter('fixture$', inputs(root//bin:the_binary))"])
        .await?;
    assert_eq!(out.stdout, "bin/TARGETS.fixture\n");
    Ok(())
}

#[buck_test(setup_eden = true, data_dir = "bxl_simple")]
async fn test_attributes(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["set(root//bin:the_binary //lib:file1)"])
        .await?;
    assert_eq!(out.stdout, "root//bin:the_binary\nroot//lib:file1\n");

    let json_out = buck
        .uquery(&["--json", "set(root//bin:the_binary //lib:file1)"])
        .await?;
    let json_out: serde_json::Value = serde_json::from_str(&json_out.stdout)?;
    assert_eq!(
        serde_json::json!(["root//bin:the_binary", "root//lib:file1"]),
        json_out
    );

    let attrs_out = buck
        .uquery(&[
            "--output-attribute",
            "buck\\..*",
            "--output-attribute",
            "srcs",
            "--output-attribute",
            "deps",
            "set(root//bin:the_binary //lib:file1)",
        ])
        .await?;
    let attrs_json_out = buck
        .uquery(&[
            "--output-attribute",
            "buck\\..*",
            "--output-attribute",
            "srcs",
            "--output-attribute",
            "deps",
            "--json",
            "set(root//bin:the_binary //lib:file1)",
        ])
        .await?;
    // specifying any attrs enables json output
    assert_eq!(attrs_json_out.stdout, attrs_out.stdout);
    let attrs_json_out: serde_json::Value = serde_json::from_str(&attrs_json_out.stdout)?;
    assert_eq!(
        serde_json::json!({
            "root//bin:the_binary": {
                "buck.deps": [
                    "root//:data",
                    "root//lib:lib1",
                    "root//lib:lib2",
                    "root//lib:lib3",
                    "root//:foo_toolchain",
                    "root//:bin",
                ],
                "buck.package": "root//bin:TARGETS.fixture",
                "buck.tree_modifiers": ["cfg//os:linux"],
                "buck.type": "_foo_binary",
                "buck.configuration_deps": [
                    "root//bin:my_platform",
                    "root//bin:my_config",
                ],
                "buck.oncall": null,
                "deps": ["root//lib:lib1", "root//lib:lib2", "root//lib:lib3"],
                "srcs": ["root//bin/TARGETS.fixture"],
            },
            "root//lib:file1": {
                "buck.deps": [],
                "buck.package": "root//lib:TARGETS.fixture",
                "buck.tree_modifiers": ["cfg//os:linux"],
                "buck.type": "_foo_genrule",
                "buck.configuration_deps": ["root//platforms:platform1"],
                "buck.oncall": null,
            },
        }),
        attrs_json_out
    );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_dot(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["--dot", "deps(root//bin:the_binary, 100, target_deps())"])
        .await?;
    golden(&out.stdout, "bxl_simple/expected/dot/deps.golden");

    let out = buck
        .uquery(&[
            "--dot",
            "--output-attribute=name",
            "--output-attribute=^deps",
            "--output-attribute=cmd",
            "deps(root//bin:the_binary, 100, target_deps()) - //platforms:",
        ])
        .await?;
    golden(&out.stdout, "bxl_simple/expected/dot/attrs.golden");

    let out = buck
        .uquery(&[
            "--dot",
            "deps(root//bin:the_binary, 100, target_deps()) - set(//lib: //platforms:)",
        ])
        .await?;
    golden(&out.stdout, "bxl_simple/expected/dot/subgraph.golden");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_dot_compact(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&[
            "--dot-compact",
            "deps(root//bin:the_binary, 100, target_deps())",
        ])
        .await?;
    golden(&out.stdout, "bxl_simple/expected/dot_compact/deps.golden");

    let out = buck
        .uquery(&[
            "--dot-compact",
            "--output-attribute=name",
            "--output-attribute=^deps",
            "--output-attribute=cmd",
            "deps(root//bin:the_binary, 100, target_deps()) - //platforms:",
        ])
        .await?;
    golden(&out.stdout, "bxl_simple/expected/dot_compact/attrs.golden");

    let out = buck
        .uquery(&[
            "--dot-compact",
            "deps(root//bin:the_binary, 100, target_deps()) - set(//lib: //platforms:)",
        ])
        .await?;
    golden(
        &out.stdout,
        "bxl_simple/expected/dot_compact/subgraph.golden",
    );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_html(buck: &Buck) -> anyhow::Result<()> {
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    buck.uquery(&[
        "--output-format=html",
        "deps(root//bin:the_binary, 100, target_deps())",
    ])
    .env("BUCK_WRAPPER_UUID", uuid)
    .await?;
    assert!(
        Bucket::new("buck2_logs", "buck2_logs-key")
            .exists(&format!("flat/{uuid}-graph.html"))
            .await?
    );
    Ok(())
}

// Tests for "%Ss" uses
#[buck_test(data_dir = "bxl_simple")]
async fn test_args_as_set(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["%Ss", "root//bin:the_binary", "//lib:file1"])
        .await?;
    assert_eq!(out.stdout, "root//bin:the_binary\nroot//lib:file1\n");

    let result = buck
        .uquery(&["--json", "%Ss", "root//bin:the_binary", "//lib:file1"])
        .await?;
    let json_out: serde_json::Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(
        json_out,
        serde_json::json!(["root//bin:the_binary", "root//lib:file1"])
    );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_multi_uquery(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["%s", "root//bin:the_binary", "//lib:file1"])
        .await?;
    assert_eq!(out.stdout, "root//bin:the_binary\nroot//lib:file1\n");

    let result = buck
        .uquery(&[
            "owner(%s)",
            "bin/TARGETS.fixture",
            "data/buck/build/data.file",
        ])
        .await?;
    assert_eq!(result.stdout, "root//bin:the_binary\nroot//data:data\n");

    let result = buck
        .uquery(&[
            "--json",
            "owner(%s)",
            "bin/TARGETS.fixture",
            "data/buck/build/data.file",
        ])
        .await?;
    let json_out: serde_json::Value = serde_json::from_str(&result.stdout)?;

    assert_eq!(
        json_out,
        serde_json::json!({
            "bin/TARGETS.fixture": ["root//bin:the_binary"],
            "data/buck/build/data.file": ["root//data:data"],
        })
    );

    // match buck1's strange handling of multi-query with --output-attribute
    let result = buck
        .uquery(&[
            "--json",
            "--output-attribute=name",
            "owner(%s)",
            "bin/TARGETS.fixture",
            "data/buck/build/data.file",
        ])
        .await?;
    let json_out: serde_json::Value = serde_json::from_str(&result.stdout)?;

    assert_eq!(
        json_out,
        serde_json::json!({
            "root//bin:the_binary": {"name": "the_binary"},
            "root//data:data": {"name": "data"},
        })
    );

    // test a case where the query for one arg fails. The process should exit with a non-zero code, but
    // the produced output should be valid json with an appropriate error indicator.
    let failure = buck
        .uquery(&["--json", "inputs(%s)", "//data:data", "xyz"])
        .await
        .expect_failure();
    let json_out: serde_json::Value = serde_json::from_str(&failure.stdout)?;
    assert!(json_out["xyz"].get("$error").is_some());
    assert_eq!(
        json_out["//data:data"],
        serde_json::json!(["data/buck/build/data.file"])
    );

    // Test where the parameter is not a literal, but a query fragment
    let out = buck.uquery(&["%s", "deps(root//lib:lib1)"]).await?;
    assert_eq!(out.stdout, "root//lib:file1\nroot//lib:lib1\n");

    let out = buck
        .uquery(&["owner(%s)", "inputs(root//bin:the_binary)"])
        .await?;
    assert_eq!(out.stdout, "root//bin:the_binary\n");

    let out = buck
        .uquery(&["owner(%s)", "data/buck/build/data.file"])
        .await?;
    assert_eq!(out.stdout, "root//data:data\n");

    // We'd really prefer this to be an error, but Buck1 allows it
    let out = buck
        .uquery(&["owner(%s", "data/buck/build/data.file)"])
        .await?;
    assert_eq!(out.stdout, "root//data:data\n");
    Ok(())
}

#[buck_test(data_dir = "testsof")]
async fn test_testsof(buck: &Buck) -> anyhow::Result<()> {
    let out = buck.uquery(&["testsof(//:foo_lib)"]).await?;

    assert!(out.stdout.contains("root//:foo_test"));
    assert!(out.stdout.contains("root//:foo_extra_test"));
    assert!(!out.stdout.contains("root//:foo_lib"));
    Ok(())
}

#[buck_test(data_dir = "directory_sources")]
async fn test_directory_source(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":a_file"]).await?;
    buck.build(&[":a_dir"]).await?;

    let result = buck.query(&["owner(dir/file1.txt)"]).await?;
    assert_eq!(result.stdout, "root//:a_dir\n");
    let result = buck.query(&["inputs(:a_dir)"]).await?;
    assert_eq!(
        result.stdout,
        "dir/file1.txt\ndir/subdir/file2.txt\ndir/subdir/file3.txt\n"
    );

    // A source that does not exist is only an error once something needs it
    buck.build(&["does_not_exist:"])
        .await
        .expect_failure()
        .assert_stderr_matches("File not found: `root//does_not_exist/does_not_exist`");

    // FIXME(JakobDegen): A source that names a subpackage directory crosses a package
    // boundary and should be an error, but nothing checks it.
    buck.build(&["subpackage:"]).await?;

    // FIXME(JakobDegen): Likewise for a directory source that contains a subpackage; it
    // silently covers only this package's files.
    buck.build(&["dir_with_subpackage:"]).await?;
    let result = buck.query(&["inputs(dir_with_subpackage:)"]).await?;
    assert!(
        !result
            .stdout
            .replace("dir_with_subpackage", "")
            .contains("subpackage")
    );
    Ok(())
}

#[buck_test(data_dir = "oncall")]
async fn test_oncall(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["//:foo", "--output-attribute=oncall"])
        .await?;
    assert!(out.stdout.contains("\"magic\""));
    let out = buck
        .cquery(&["//:bar", "--output-attribute=oncall"])
        .await?;
    assert!(out.stdout.contains("\"magic\""));
    Ok(())
}

#[buck_test(data_dir = "oncall")]
async fn test_output_all_attributes(buck: &Buck) -> anyhow::Result<()> {
    fn contains(out: &BuckOutput, want: &[&str], notwant: &[&str]) -> anyhow::Result<()> {
        let json: serde_json::Value = serde_json::from_str(&out.stdout)?;
        let x = json
            .get("root//:foo")
            .and_then(serde_json::Value::as_object)
            .context("No `root//:foo` in the output")?;
        for w in want {
            assert!(x.contains_key(*w), "{w} not in {x:?}");
        }
        for w in notwant {
            assert!(!x.contains_key(*w), "{w} in {x:?}");
        }
        Ok(())
    }

    let out = buck
        .uquery(&["//:foo", "--output-all-attributes", "--json"])
        .await?;
    contains(
        &out,
        &[
            "buck.type",
            "name",
            "buck.oncall",
            "buck.package",
            "buck.configuration_deps",
            "buck.deps",
            "visibility",
        ],
        &["madeup"],
    )?;
    let out = buck
        .uquery(&["//:foo", "--output-basic-attributes", "--json"])
        .await?;
    contains(
        &out,
        &["buck.type", "name", "buck.package", "visibility"],
        &["buck.oncall", "buck.configuration_deps"],
    )?;
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_output_format_starlark_golden(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["--output-format=starlark", "--stack", "//lib:"])
        .await?;

    golden(&result.stdout, "output_starlark.golden.out");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_uquery_rdeps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .query(&["rdeps(root//bin:the_binary, //lib:file1)"])
        .await?;
    assert_eq!(
        result.stdout,
        "root//bin:the_binary\nroot//lib:lib1\nroot//lib:file1\n"
    );

    let result = buck
        .query(&["rdeps(root//bin:the_binary, //lib:file1, 0)"])
        .await?;
    assert_eq!(result.stdout, "root//lib:file1\n");

    let result = buck
        .query(&["rdeps(root//bin:the_binary, //lib:file1, 1)"])
        .await?;
    assert_eq!(result.stdout, "root//lib:lib1\nroot//lib:file1\n");

    let result = buck
        .query(&["rdeps(root//bin:the_binary, //lib:file1, 100)"])
        .await?;
    assert_eq!(
        result.stdout,
        "root//bin:the_binary\nroot//lib:lib1\nroot//lib:file1\n"
    );
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_query_attrfilter_special_attribute(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["attrfilter(buck.package, 'root//bin:TARGETS.fixture',root//bin:the_binary)"])
        .await?;
    assert_eq!(out.stdout.trim(), "root//bin:the_binary");
    Ok(())
}

// Tests for intersect and except operators on FileSet, TargetSet, and String types
// These tests verify the fix for https://github.com/facebook/buck2/issues/1109
/// Test FileSet intersect FileSet using inputs().
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_fileset_intersect(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["inputs(root//:lib_a) intersect inputs(root//:lib_b)"])
        .await?;
    assert_eq!(result.stdout, "common.txt\n");
    Ok(())
}

/// Test FileSet except FileSet using inputs().
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_fileset_except(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["inputs(root//:lib_a) except inputs(root//:lib_b)"])
        .await?;
    assert_eq!(result.stdout, "lib_a.txt\n");
    Ok(())
}

/// Test FileSet intersect String.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_fileset_intersect_string(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["inputs(root//:lib_a) intersect \"common.txt\" "])
        .await?;
    assert_eq!(result.stdout, "common.txt\n");
    Ok(())
}

/// Test FileSet except String.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_fileset_except_string(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["inputs(root//:lib_a) except \"common.txt\" "])
        .await?;
    assert_eq!(result.stdout, "lib_a.txt\n");
    Ok(())
}

/// Test String intersect FileSet.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_string_intersect_fileset(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[" \"common.txt\" intersect inputs(root//:lib_a)"])
        .await?;
    assert_eq!(result.stdout, "common.txt\n");
    Ok(())
}

/// Test String except FileSet (string not in fileset).
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_string_except_fileset(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[" \"lib_a.txt\" except inputs(root//:lib_b)"])
        .await?;
    assert_eq!(result.stdout, "lib_a.txt\n");
    Ok(())
}

/// Test TargetSet intersect TargetSet using set().
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_targetset_intersect(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["set(root//:lib_a root//:app) intersect set(root//:lib_b root//:app)"])
        .await?;
    assert_eq!(result.stdout, "root//:app\n");
    Ok(())
}

/// Test TargetSet except TargetSet using set().
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_targetset_except(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["set(root//:lib_a root//:app) except set(root//:app)"])
        .await?;
    assert_eq!(result.stdout, "root//:lib_a\n");
    Ok(())
}

/// Test TargetSet intersect String.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_targetset_intersect_string(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["set(root//:lib_a root//:app) intersect \"root//:lib_a\" "])
        .await?;
    assert_eq!(result.stdout, "root//:lib_a\n");
    Ok(())
}

/// Test TargetSet except String.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_targetset_except_string(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["set(root//:lib_a root//:app) except \"root//:app\" "])
        .await?;
    assert_eq!(result.stdout, "root//:lib_a\n");
    Ok(())
}

/// Test String intersect TargetSet.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_string_intersect_targetset(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[" \"root//:lib_a\" intersect set(root//:lib_a root//:app)"])
        .await?;
    assert_eq!(result.stdout, "root//:lib_a\n");
    Ok(())
}

/// Test String except TargetSet (string not in targetset).
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_string_except_targetset(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[" \"root//:app\" except set(root//:lib_a root//:lib_b)"])
        .await?;
    assert_eq!(result.stdout, "root//:app\n");
    Ok(())
}

/// Test String intersect String for targets.
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_string_intersect_string(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[" \"root//:lib_a\" intersect \"root//:lib_a\" "])
        .await?;
    assert_eq!(result.stdout, "root//:lib_a\n");
    Ok(())
}

/// Test String except String (different targets).
#[buck_test(data_dir = "set_operators")]
async fn test_uquery_string_except_string(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[" \"root//:app\" except \"root//:lib_a\" "])
        .await?;
    assert_eq!(result.stdout, "root//:app\n");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_multi_query_mixed_files_and_targets(buck: &Buck) -> anyhow::Result<()> {
    // With `--json` every argument gets its own result.
    let result = buck
        .uquery(&[
            "--json",
            "%s",
            "inputs(root//bin:the_binary)",
            "root//bin:the_binary",
        ])
        .await?;
    let json: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&result.stdout)?;
    let mut keys: Vec<&str> = json.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["inputs(root//bin:the_binary)", "root//bin:the_binary"]
    );
    // Without it the results are merged. One argument returns files and the other targets, which
    // is an input error.
    buck.uquery(&["%s", "inputs(root//bin:the_binary)", "root//bin:the_binary"])
        .await
        .expect_failure()
        .assert_stderr_matches("can only merge results of one kind");
    Ok(())
}

#[buck_test(data_dir = "bxl_simple")]
async fn test_nattrfilter_special_attribute(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["attrfilter(buck.type, other_rule, root//bin:the_binary)"])
        .await?;
    assert_eq!(out.stdout, "");
    let out = buck
        .uquery(&["nattrfilter(buck.type, other_rule, root//bin:the_binary)"])
        .await?;
    assert_eq!(out.stdout, "root//bin:the_binary\n");
    Ok(())
}

#[buck_test(data_dir = "query_attrs")]
async fn test_query_attribute_text(buck: &Buck) -> anyhow::Result<()> {
    // Query text in attributes and macros renders as the parsed expression:
    // labels resolve to canonical form, and a label that is not a bare query
    // word is quoted.
    let out = buck
        .uquery(&[
            "--output-attribute",
            "^q$",
            "--output-attribute",
            "^arg$",
            "//:holder",
        ])
        .await?;
    let json: serde_json::Value = serde_json::from_str(&out.stdout)?;
    assert_eq!(
        json,
        serde_json::json!({
            "root//:holder": {
                "arg": "$(query_targets deps('root//:libc++'))",
                "q": "deps(root//:dep)",
            }
        })
    );
    Ok(())
}
