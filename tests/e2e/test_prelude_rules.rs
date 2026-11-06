/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::http::StaticHttpServer;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use serde_json::Value;
use sha2::Digest;

async fn output(buck: &Buck, target: &str) -> anyhow::Result<PathBuf> {
    let res = buck
        .build_without_report(&[target, "--show-full-simple-output"])
        .await?;
    Ok(PathBuf::from(res.stdout.trim()))
}

/// The `value` attribute of every target in the package, by target name.
async fn values(buck: &Buck, package: &str) -> anyhow::Result<BTreeMap<String, Value>> {
    let res = buck
        .uquery(&["--output-attribute", "value", &format!("root//{package}:")])
        .await?;
    let targets: serde_json::Map<String, Value> = serde_json::from_str(&res.stdout)?;
    targets
        .iter()
        .map(|(target, attrs)| {
            let name = target
                .split(':')
                .nth(1)
                .with_context(|| format!("`{target}` has no name"))?;
            let value = attrs
                .get("value")
                .with_context(|| format!("`{target}` has no `value`"))?;
            Ok((name.to_owned(), value.clone()))
        })
        .collect()
}

fn expected(values: &[(&str, &str)]) -> BTreeMap<String, Value> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::from(*value)))
        .collect()
}

// prelude/apple/apple_bundle_destination.bzl `bundle_relative_path_for_destination`
#[buck_test(inplace = false)]
async fn test_extensionkit_extensions_destination(buck: &Buck) -> anyhow::Result<()> {
    assert_eq!(
        values(buck, "bundle").await?,
        expected(&[
            ("iphoneos_app_extensionkit", "Extensions"),
            ("macosx_app_extensionkit", "Contents/Extensions"),
            ("maccatalyst_app_extensionkit", "Contents/Extensions"),
        ])
    );
    Ok(())
}

// prelude/cxx/headers.bzl `_infer_include_prefix_from_paths`
#[buck_test(inplace = false)]
async fn test_include_prefix_needs_a_whole_path_suffix(buck: &Buck) -> anyhow::Result<()> {
    assert_eq!(
        values(buck, "headers").await?,
        expected(&[
            ("renamed_header_prefix", "None"),
            ("subdir_header_prefix", "third-party/foo/config"),
        ])
    );
    Ok(())
}

// prelude/cxx/cxx_sources.bzl `get_srcs_with_flags`
#[buck_test(inplace = false)]
async fn test_distinct_flag_sets_compile_separately(buck: &Buck) -> anyhow::Result<()> {
    for target in ["colliding_flag_sets", "quote_colliding_flag_sets"] {
        let out = output(buck, &format!("root//srcs_flags:{target}")).await?;
        assert_eq!(
            std::fs::read_to_string(out)?
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["0", "1"],
            "{target}"
        );
    }
    let out = output(buck, "root//srcs_flags:repeated_flag_set").await?;
    assert_eq!(
        std::fs::read_to_string(out)?
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["None"]
    );
    Ok(())
}

// prelude/sh_binary.bzl `_generate_script`
#[buck_test(inplace = false, skip_for_os = ["windows"])]
async fn test_two_resources_at_one_link_are_an_error(buck: &Buck) {
    buck.run(&["root//sh_resources:show"])
        .await
        .expect_failure()
        .assert_stderr_matches("would both be placed at `sh_resources/gen`");
}

#[buck_test(inplace = false, skip_for_os = ["windows"])]
async fn test_a_resource_at_the_main_link_is_an_error(buck: &Buck) {
    buck.build(&["root//sh_resources:resource_at_main_link"])
        .await
        .expect_failure()
        .assert_stderr_matches("would both be placed at `show.sh`");
}

// prelude/http_archive/unarchive.bzl `unarchive`
#[buck_test(inplace = false, skip_for_os = ["windows", "darwin"])]
async fn test_excluded_names_are_read_by_tar_as_globs(buck: &Buck) -> anyhow::Result<()> {
    // The exclusion list holds the member names that matched the `excludes`
    // regexes, but tar reads every `--exclude-from` line as a glob: `[id].js`
    // keeps the excluded file and drops `i.js` and `d.js` instead.
    let out = output(buck, "root//archive:unpacked").await?;
    let mut names = std::fs::read_dir(out.join("pages"))?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<anyhow::Result<Vec<_>>>()?;
    names.sort();
    assert_eq!(names, ["[id].js", "keep.js"]);
    Ok(())
}

// prelude/http_file.bzl `http_file_shared`, through `http_file` and `remote_file`
#[buck_test(inplace = false, skip_for_os = ["windows"])]
async fn test_a_sha512_alone_is_a_checksum(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let sha512 = hex::encode(sha2::Sha512::digest(&content));
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let url_config = format!("test.url={}", server.url("/file"));
    let sha512_config = format!("test.sha512={sha512}");
    for target in ["root//download:http_file", "root//download:remote_file"] {
        let mut args = vec![target, "--show-full-simple-output"];
        args.extend(["-c", &url_config, "-c", &sha512_config]);
        let res = buck.build_without_report(&args).await?;
        assert_eq!(std::fs::read(res.stdout.trim())?, content, "{target}");
    }
    Ok(())
}

// prelude/http_file.bzl `http_file_shared`, through `http_file` and `remote_file`
#[buck_test(inplace = false, skip_for_os = ["windows"])]
async fn test_a_wrong_sha512_fails_the_download(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string().into_bytes();
    let wrong = "0".repeat(128);
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let url = server.url("/file");
    for target in ["root//download:http_file", "root//download:remote_file"] {
        buck.build(&[
            target,
            "-c",
            &format!("test.url={url}"),
            "-c",
            &format!("test.sha512={wrong}"),
        ])
        .await
        .expect_failure()
        .assert_stderr_matches(&format!(
            "Invalid sha512 digest. Expected {wrong}, got {}",
            hex::encode(sha2::Sha512::digest(&content))
        ));
    }
    Ok(())
}
