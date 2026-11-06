/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::http::StaticHttpServer;
use buck2_e2e::http::download_configs;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

/// A ZIP archive of archive-relative file paths and their contents, stored uncompressed. A random
/// comment changes the digest without changing the extracted files, so that a previous CAS entry
/// cannot satisfy a test intended to exercise HTTP.
fn make_zip(files: &[(&str, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive.set_comment(random_string())?;
    for (name, content) in files {
        archive.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )?;
        archive.write_all(content)?;
    }
    Ok(archive.finish()?.into_inner())
}

fn archive_files() -> Vec<(&'static str, Vec<u8>)> {
    ["foo2", "foo/bar2", "foo/bar/qux"]
        .into_iter()
        .map(|name| (name, format!("{name}\n").into_bytes()))
        .collect()
}

fn manifest_strings(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|v| v.as_str().expect("a string"))
        .collect()
}

#[track_caller]
fn assert_path_in_manifest(path: &str, manifest_paths: &Value) {
    assert!(
        manifest_strings(manifest_paths).contains(&path),
        "expected manifest to contain {path}"
    );
}

#[track_caller]
fn assert_link_in(needle: &Value, haystack: &Value) {
    assert!(
        haystack.as_array().expect("a list").contains(needle),
        "expected haystack to contain link: {} --> {}",
        needle["link"],
        needle["target"]
    );
}

#[track_caller]
fn assert_path_exists(path: &Path) {
    assert!(path.exists(), "expected {} to exist", path.display());
}

#[track_caller]
fn assert_buck_out_paths_materialized(buck_cwd: &Path, paths: &Value) {
    let materialized = Regex::new(r"^buck-out/.+/(art|offline-cache)/.+/.+/.+").unwrap();
    for path in manifest_strings(paths) {
        if materialized.is_match(path) {
            assert_path_exists(&buck_cwd.join(path));
        }
    }
}

fn run(args: &[&str], cwd: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .status()?;
    anyhow::ensure!(status.success(), "`{}` failed: {status}", args.join(" "));
    Ok(())
}

fn hg_init(cwd: &Path) -> anyhow::Result<()> {
    run(&["hg", "init"], cwd)?;
    hg_config_reponame(cwd)
}

fn hg_config_reponame(cwd: &Path) -> anyhow::Result<()> {
    run(
        &[
            "hg",
            "config",
            "remotefilelog.reponame",
            "--local",
            "no-repo",
        ],
        cwd,
    )
}

fn setup_buckconfig_digest_algorithms(buck: &Buck) -> anyhow::Result<()> {
    // The digests in `//cas_artifact:` require the buckconfig.
    let mut buckconfig = fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?;
    buckconfig.write_all(b"[buck2]\n")?;
    buckconfig.write_all(b"digest_algorithms = BLAKE3-KEYED,SHA1\n")?;
    Ok(())
}

/// Symlinks link --> target. Assumes we're based in the buck cwd, so link must be relative.
fn symlink(buck: &Buck, link: &str, target: impl AsRef<Path>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, buck.cwd().join(link))
    }
    #[cfg(not(unix))]
    {
        let _unused = (buck, link, target);
        unreachable!("the symlink tests do not run on Windows")
    }
}

async fn export_manifest(buck: &Buck) -> anyhow::Result<Value> {
    let out = buck.debug(&["trace-io", "export-manifest"]).await?;
    Ok(serde_json::from_str(&out.stdout)?)
}

/// Where the offline cache keeps `file` next to the output `output`. This is hacky, but there's no
/// other good way to discover the offline-cache path.
fn offline_cache_path(output: &Path, file: &str) -> PathBuf {
    PathBuf::from(output.to_string_lossy().replace("/art/", "/offline-cache/"))
        .parent()
        .expect("an output in a directory")
        .join(file)
}

fn concat_args<'a>(first: &[&'a str], rest: &'a [String]) -> Vec<&'a str> {
    first
        .iter()
        .copied()
        .chain(rest.iter().map(String::as_str))
        .collect()
}

// Tracing I/O not implemented for Windows.
#[buck_test(skip_for_os = ["windows"])]
async fn test_simple_binary_build(buck: &Buck) -> anyhow::Result<()> {
    // Since this is an inplace test, we need to fake an hg repo so that export-manifest
    // can extract the repo revision.
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//hello_world:welcome"]).await?;
    let manifest = export_manifest(buck).await?;

    assert_eq!(
        manifest["repository"]["revision"], "0000000000000000000000000000000000000000",
        "expected manifest to be at null revision"
    );
    assert_eq!(
        manifest["repository"]["name"], "no-repo",
        "expected repo name to be no-repo"
    );

    assert_path_in_manifest("hello_world/main.cpp", &manifest["paths"]);
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_external_buckconfig_path_included_in_manifest(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    let tmp = tmp_path.join("config.bcfg");
    fs::write(&tmp, "[buck2]\n  foo = bar\n")?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&[
        "root//hello_world:welcome",
        "--config-file",
        tmp.to_str().unwrap(),
    ])
    .await?;
    let manifest = export_manifest(buck).await?;

    assert_path_in_manifest(
        fs::canonicalize(&tmp)?.to_str().unwrap(),
        &manifest["external_paths"],
    );
    Ok(())
}

// More complicated example with binary depending on multiple libraries.
#[buck_test(skip_for_os = ["windows"])]
async fn test_binary_with_deps(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//linking:root"]).await?;
    let manifest = export_manifest(buck).await?;

    assert_eq!(
        manifest["repository"]["revision"], "0000000000000000000000000000000000000000",
        "expected manifest to be at null revision"
    );
    assert_eq!(
        manifest["repository"]["name"], "no-repo",
        "expected repo name to be no-repo"
    );

    assert_path_in_manifest("linking/main.cpp", &manifest["paths"]);
    assert_path_in_manifest("linking/static.cpp", &manifest["paths"]);
    assert_path_in_manifest("linking/static.h", &manifest["paths"]);
    assert_path_in_manifest("linking/shared.h", &manifest["paths"]);
    Ok(())
}

// Multiple builds should be logical union of all input files of all builds.
#[buck_test(skip_for_os = ["windows"])]
async fn test_multiple_builds(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//linking:root"]).await?;
    buck.build(&["root//hello_world:welcome"]).await?;
    let manifest = export_manifest(buck).await?;

    // From first build
    assert_path_in_manifest("linking/shared.h", &manifest["paths"]);
    // From second build
    assert_path_in_manifest("hello_world/main.cpp", &manifest["paths"]);
    Ok(())
}

// Symlinks should show up in the *_symlinks attributes of the manifest.
#[buck_test(
    setup_eden = true,
    skip_for_os = ["windows"],
    env = [("BUCK2_HARD_ERROR", "false")],
)]
async fn test_symlinks(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    hg_config_reponame(buck.cwd())?;

    // Set up symlinks during the test; buck will read everything behind symlinks while
    // setting up for the test otherwise.
    // Symlinks for root//symlinks:relative_link
    symlink(buck, "symlinks/main.cpp", "../hello_world/main.cpp")?;

    // Symlinks for root//symlinks:external_link, to a temporary directory that is gone by the
    // time the manifest is exported.
    let t = tmp_path.join("tempdir");
    let absolute_target = t
        .join("include")
        .join("clang")
        .join("Basic")
        .join("Visibility.h");
    fs::create_dir_all(absolute_target.parent().unwrap())?;
    fs::File::create(&absolute_target)?;

    let traverses_symlink = t.join("include").join("llvm").join("PassRegistry.h");
    fs::create_dir_all(traverses_symlink.parent().unwrap())?;
    fs::File::create(&traverses_symlink)?;

    symlink(buck, "symlinks/PassRegistry.h", &absolute_target)?;
    symlink(buck, "symlinks/include", t.join("include"))?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//symlinks:relative_link"]).await?;
    buck.build(&["root//symlinks:external_link"]).await?;
    fs::remove_dir_all(&t)?;

    let manifest = export_manifest(buck).await?;

    assert_link_in(
        &json!({"link": "symlinks/main.cpp", "target": "hello_world/main.cpp"}),
        &manifest["relative_symlinks"],
    );
    assert_link_in(
        &json!({
            "link": "symlinks/PassRegistry.h",
            "target": absolute_target.to_str().unwrap(),
            "remaining_path": null,
        }),
        &manifest["external_symlinks"],
    );
    assert_link_in(
        &json!({
            "link": "symlinks/include",
            "target": t.join("include").to_str().unwrap(),
            "remaining_path": "clang/Basic/Visibility.h",
        }),
        &manifest["external_symlinks"],
    );
    assert_path_in_manifest("symlinks/other.cpp", &manifest["paths"]);
    Ok(())
}

// Validate that manifest includes downloaded http_archive path in buck-out.
#[buck_test(skip_for_os = ["windows"])]
async fn test_includes_http_archive_in_manifest(buck: &Buck) -> anyhow::Result<()> {
    let content = make_zip(&archive_files())?;
    let server = StaticHttpServer::start([("/archive", content.clone())]).await?;
    let config = download_configs(&server.url("/archive"), &content);
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&concat_args(&["root//http_archive:test_zip"], &config))
        .await?;
    let manifest = export_manifest(buck).await?;

    let cached = Regex::new(r"^buck-out/.+/offline-cache/.+/http_archive/__test_zip__/download")?;
    assert!(
        manifest_strings(&manifest["paths"])
            .iter()
            .any(|path| cached.is_match(path)),
        "manifest should contain http_archive cached output"
    );
    assert_buck_out_paths_materialized(buck.cwd(), &manifest["paths"]);
    Ok(())
}

// Ensure offline-cache buck-out dir is _not_ created when not doing I/O tracing.
#[buck_test(skip_for_os = ["windows"])]
async fn test_no_tracing_does_not_write_offline_cache_for_http_archive(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = make_zip(&archive_files())?;
    let server = StaticHttpServer::start([("/archive", content.clone())]).await?;
    let config = download_configs(&server.url("/archive"), &content);
    buck.build(&concat_args(&["root//http_archive:test_zip"], &config))
        .await?;
    assert!(
        !buck.cwd().join("buck-out/offline-cache").exists(),
        "offline cache should not exist when not doing I/O tracing"
    );
    Ok(())
}

// Validate that when buckconfig use_network_action_output_cache=true is set we use the
// offline-cache action output instead of fetching from the network.
#[buck_test(
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_materializer_state", "false")],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_fake_offline_http_archive_uses_offline_cache(buck: &Buck) -> anyhow::Result<()> {
    let content = make_zip(&archive_files())?;
    let server = StaticHttpServer::start([("/archive", content.clone())]).await?;
    let config = download_configs(&server.url("/archive"), &content);
    hg_init(buck.cwd())?;

    // This should materialize the offline-cache dir.
    let target = "root//http_archive:test_zip";
    buck.debug(&["trace-io", "enable"]).await?;
    let result = buck.build(&concat_args(&[target], &config)).await?;
    eprintln!("stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("/offline-cache/"),
        "materializer should declare offline-cache materialization"
    );

    // Validate that offline-cache path doesn't exist prior to manifest export.
    let http_download_path = result.build_report()?.output_for_target(target)?;
    let offline_cache_path = offline_cache_path(&http_download_path, "download");
    assert!(
        !offline_cache_path.exists(),
        "offline cache path should not exist before manifest export"
    );

    // Ensure buck-out/offline-cache paths are materialized.
    buck.debug(&["trace-io", "export-manifest"]).await?;
    assert!(
        offline_cache_path.exists(),
        "offline cache path should exist after manifest export"
    );

    buck.kill().await?;
    assert!(server.count("GET", "/archive") > 0);
    drop(server);

    let mut args: Vec<&str> = config.iter().map(String::as_str).collect();
    args.extend([
        "root//http_archive:test_zip",
        "--config",
        "buck2.use_network_action_output_cache=true",
        "--no-remote-cache",
        "--local-only",
    ]);
    let result = buck.build(&args).await?;
    assert!(
        result.stderr.contains("LocalCopy"),
        "offline-cache path should be copied to output"
    );
    assert!(
        http_download_path.exists(),
        "http download output path should exist"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_includes_cas_artifact_in_manifest(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    setup_buckconfig_digest_algorithms(buck)?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["//cas_artifact:tree"]).await?;
    let manifest = export_manifest(buck).await?;

    let cached = Regex::new(r"^buck-out/.+/offline-cache/root/.+/cas_artifact/__tree__/tree")?;
    assert!(
        manifest_strings(&manifest["paths"])
            .iter()
            .any(|path| cached.is_match(path)),
        "offline cache should contain cas artifact tree"
    );

    assert_buck_out_paths_materialized(buck.cwd(), &manifest["paths"]);
    Ok(())
}

// Ensure offline-cache buck-out dir is _not_ created when not doing I/O tracing.
#[buck_test(skip_for_os = ["windows"])]
async fn test_no_tracing_does_not_write_offline_cache_for_cas_artifact(
    buck: &Buck,
) -> anyhow::Result<()> {
    setup_buckconfig_digest_algorithms(buck)?;

    buck.build(&["//cas_artifact:tree"]).await?;
    assert!(
        !buck.cwd().join("buck-out/offline-cache").exists(),
        "offline cache should not exist when not doing I/O tracing"
    );
    Ok(())
}

// Validate that when buckconfig use_network_action_output_cache=true is set we use the
// offline-cache action output instead of fetching from the network.
#[buck_test(
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_materializer_state", "false")],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_fake_offline_cas_artifact_uses_offline_cache(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    setup_buckconfig_digest_algorithms(buck)?;

    // This should materialize the offline-cache dir.
    let target = "root//cas_artifact:tree";
    buck.debug(&["trace-io", "enable"]).await?;
    let result = buck.build(&[target]).await?;
    eprintln!("stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("/offline-cache/"),
        "materializer should declare offline-cache materialization"
    );

    // Validate that offline-cache path doesn't exist prior to manifest export.
    let cas_download_path = result.build_report()?.output_for_target(target)?;
    let offline_cache_path = offline_cache_path(&cas_download_path, "tree");
    assert!(
        !offline_cache_path.exists(),
        "offline cache path should not exist before manifest export"
    );

    // Ensure buck-out/offline-cache paths are materialized.
    buck.debug(&["trace-io", "export-manifest"]).await?;
    assert!(
        offline_cache_path.exists(),
        "offline cache path should exist after manifest export"
    );

    buck.kill().await?;

    let result = buck
        .build(&[
            target,
            "--config",
            "buck2.use_network_action_output_cache=true",
            "--no-remote-cache",
            "--local-only",
        ])
        .await?;
    assert!(
        result.stderr.contains("LocalCopy"),
        "offline-cache path should be copied to output"
    );
    assert!(
        cas_download_path.exists(),
        "cas action output path should exist"
    );
    Ok(())
}

/// Whether `list` is sorted by `link`.
fn sorted_by_link(list: &Value) -> bool {
    let list = list.as_array().expect("a list");
    list.windows(2)
        .all(|w| w[0]["link"].as_str().expect("a link") <= w[1]["link"].as_str().expect("a link"))
}

// Validate that all lists in the exported manifest are sorted.
#[buck_test(
    setup_eden = true,
    skip_for_os = ["windows"],
    env = [("BUCK2_HARD_ERROR", "false")],
)]
async fn test_manifest_lists_are_sorted(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    hg_config_reponame(buck.cwd())?;

    // Set up multiple files in reverse alphabetical order to ensure they need sorting
    symlink(buck, "symlinks/zz_last.cpp", "../hello_world/main.cpp")?;
    symlink(buck, "symlinks/aa_first.cpp", "../linking/main.cpp")?;
    symlink(buck, "symlinks/mm_middle.cpp", "../linking/static.cpp")?;

    let t = tmp_path.join("tempdir");
    fs::create_dir(&t)?;

    let zz_file = t.join("zz_external.h");
    fs::File::create(&zz_file)?;

    let aa_file = t.join("aa_external.h");
    fs::File::create(&aa_file)?;

    let mm_file = t.join("mm_external.h");
    fs::File::create(&mm_file)?;

    symlink(buck, "symlinks/ext_1.h", &zz_file)?;
    symlink(buck, "symlinks/ext_2.h", &aa_file)?;
    symlink(buck, "symlinks/ext_3.h", &mm_file)?;

    buck.debug(&["trace-io", "enable"]).await?;

    // Build multiple targets to create entries in non-alphabetical order
    buck.build(&["root//symlinks:zz_last"]).await?;
    buck.build(&["root//symlinks:aa_first"]).await?;
    buck.build(&["root//symlinks:mm_middle"]).await?;

    let tmpname1 = tmp_path.join("config1.bcfg");
    fs::write(&tmpname1, "[buck2]\n  foo = bar\n")?;

    let tmpname2 = tmp_path.join("config2.bcfg");
    fs::write(&tmpname2, "[buck2]\n  baz = qux\n")?;

    // Build with config files in reverse order to create unsorted external entries
    buck.build(&[
        "root//hello_world:welcome",
        "--config-file",
        tmpname2.to_str().unwrap(),
    ])
    .await?;
    buck.build(&[
        "root//hello_world:welcome",
        "--config-file",
        tmpname1.to_str().unwrap(),
    ])
    .await?;

    let manifest = export_manifest(buck).await?;
    fs::remove_file(&tmpname1)?;
    fs::remove_file(&tmpname2)?;
    fs::remove_dir_all(&t)?;

    let paths = manifest_strings(&manifest["paths"]);
    assert!(paths.is_sorted(), "paths list is not sorted: {paths:?}");

    let external_paths = manifest_strings(&manifest["external_paths"]);
    assert!(
        external_paths.is_sorted(),
        "external_paths list is not sorted: {external_paths:?}"
    );

    let relative_symlinks = &manifest["relative_symlinks"];
    assert!(
        sorted_by_link(relative_symlinks),
        "relative_symlinks list is not sorted by link: {relative_symlinks}"
    );

    let external_symlinks = &manifest["external_symlinks"];
    assert!(
        sorted_by_link(external_symlinks),
        "external_symlinks list is not sorted by link: {external_symlinks}"
    );

    assert_link_in(
        &json!({"link": "symlinks/zz_last.cpp", "target": "hello_world/main.cpp"}),
        relative_symlinks,
    );
    assert_link_in(
        &json!({"link": "symlinks/aa_first.cpp", "target": "linking/main.cpp"}),
        relative_symlinks,
    );
    assert_link_in(
        &json!({"link": "symlinks/mm_middle.cpp", "target": "linking/static.cpp"}),
        relative_symlinks,
    );
    Ok(())
}

/// Test RunAction caching when allow_offline_output_cache=True.
#[buck_test(
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_materializer_state", "false")],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_run_action_with_allow_offline_output_cache(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    let target = "root//run_action_cache:cached_target";

    // Build with trace mode to populate offline cache
    buck.debug(&["trace-io", "enable"]).await?;
    let result = buck.build(&[target]).await?;
    eprintln!("stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("/offline-cache/"),
        "materializer should declare offline-cache materialization"
    );

    // Get output path
    let output_path = result.build_report()?.output_for_target(target)?;

    // Compute offline cache path (hacky but same as other tests)
    let offline_cache_path = offline_cache_path(&output_path, "out.txt");
    assert!(
        !offline_cache_path.exists(),
        "offline cache path should not exist before manifest export"
    );

    // Export manifest to materialize offline-cache
    buck.debug(&["trace-io", "export-manifest"]).await?;
    assert!(
        offline_cache_path.exists(),
        "offline cache path should exist after manifest export"
    );

    buck.kill().await?;

    // Rebuild with offline cache enabled
    let result = buck
        .build(&[
            target,
            "--config",
            "buck2.use_network_action_output_cache=true",
            "--no-remote-cache",
            "--local-only",
        ])
        .await?;
    assert!(
        result.stderr.contains("LocalCopy"),
        "offline-cache path should be copied to output"
    );
    assert!(output_path.exists(), "action output path should exist");
    Ok(())
}

/// Test that RunAction without allow_offline_output_cache doesn't cache.
#[buck_test(skip_for_os = ["windows"])]
async fn test_run_action_without_parameter_does_not_cache(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//run_action_cache:uncached_target"])
        .await?;
    let manifest = export_manifest(buck).await?;

    // Verify that offline-cache paths do NOT include uncached_target
    let offline_cache_paths: Vec<&str> = manifest_strings(&manifest["paths"])
        .into_iter()
        .filter(|path| path.contains("/offline-cache/") && path.contains("uncached_target"))
        .collect();
    assert!(
        offline_cache_paths.is_empty(),
        "uncached target should not appear in offline-cache: {offline_cache_paths:?}"
    );
    Ok(())
}

/// Test that cached RunAction outputs appear in trace manifest.
#[buck_test(skip_for_os = ["windows"])]
async fn test_run_action_cache_includes_in_manifest(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//run_action_cache:cached_target"])
        .await?;
    let manifest = export_manifest(buck).await?;

    let cached = Regex::new(
        r"^buck-out/.+/offline-cache/root/.+/run_action_cache/__cached_target__/out.txt",
    )?;
    assert!(
        manifest_strings(&manifest["paths"])
            .iter()
            .any(|path| cached.is_match(path)),
        "offline cache should contain cached run action output"
    );

    assert_buck_out_paths_materialized(buck.cwd(), &manifest["paths"]);
    Ok(())
}

/// Test genrule caching when allow_offline_output_cache=True.
#[buck_test(
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_materializer_state", "false")],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_genrule_with_allow_offline_output_cache(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    let target = "root//genrule_cache:cached";

    // Build with trace mode to populate offline cache
    buck.debug(&["trace-io", "enable"]).await?;
    let result = buck.build(&[target]).await?;
    eprintln!("stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("/offline-cache/"),
        "materializer should declare offline-cache materialization"
    );

    // Get output path
    let output_path = result.build_report()?.output_for_target(target)?;

    // Compute offline cache path
    let offline_cache_path = offline_cache_path(&output_path, "output.txt");
    assert!(
        !offline_cache_path.exists(),
        "offline cache path should not exist before manifest export"
    );

    // Export manifest to materialize offline-cache
    buck.debug(&["trace-io", "export-manifest"]).await?;
    assert!(
        offline_cache_path.exists(),
        "offline cache path should exist after manifest export"
    );

    buck.kill().await?;

    // Rebuild with offline cache enabled
    let result = buck
        .build(&[
            target,
            "--config",
            "buck2.use_network_action_output_cache=true",
            "--no-remote-cache",
            "--local-only",
        ])
        .await?;
    assert!(
        result.stderr.contains("LocalCopy"),
        "offline-cache path should be copied to output"
    );
    assert!(output_path.exists(), "genrule output path should exist");
    Ok(())
}

/// Test that genrule without allow_offline_output_cache doesn't cache.
#[buck_test(skip_for_os = ["windows"])]
async fn test_genrule_without_parameter_does_not_cache(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//genrule_cache:uncached"]).await?;
    let manifest = export_manifest(buck).await?;

    // Verify that offline-cache paths do NOT include uncached genrule
    let offline_cache_paths: Vec<&str> = manifest_strings(&manifest["paths"])
        .into_iter()
        .filter(|path| path.contains("/offline-cache/") && path.contains("uncached"))
        .collect();
    assert!(
        offline_cache_paths.is_empty(),
        "uncached genrule should not appear in offline-cache: {offline_cache_paths:?}"
    );
    Ok(())
}

/// Test that cached genrule outputs appear in trace manifest.
#[buck_test(skip_for_os = ["windows"])]
async fn test_genrule_cache_includes_in_manifest(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd())?;

    buck.debug(&["trace-io", "enable"]).await?;
    buck.build(&["root//genrule_cache:cached"]).await?;
    let manifest = export_manifest(buck).await?;

    let cached =
        Regex::new(r"^buck-out/.+/offline-cache/root/.+/genrule_cache/__cached__/output.txt")?;
    assert!(
        manifest_strings(&manifest["paths"])
            .iter()
            .any(|path| cached.is_match(path)),
        "offline cache should contain cached genrule output"
    );

    assert_buck_out_paths_materialized(buck.cwd(), &manifest["paths"]);
    Ok(())
}

// No-op test for windows.
#[buck_test]
async fn test_noop(_buck: &Buck) {}
