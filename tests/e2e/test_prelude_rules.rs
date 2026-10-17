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
use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::prelude::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;

async fn output(buck: &Buck, target: &str, args: &[&str]) -> anyhow::Result<PathBuf> {
    let res = buck
        .build_without_report(&[&[target, "--show-full-simple-output"][..], args].concat())
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
        let out = output(buck, &format!("root//srcs_flags:{target}"), &[]).await?;
        assert_eq!(
            std::fs::read_to_string(out)?
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["0", "1"],
            "{target}"
        );
    }
    let out = output(buck, "root//srcs_flags:repeated_flag_set", &[]).await?;
    assert_eq!(
        std::fs::read_to_string(out)?
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["None"]
    );
    Ok(())
}

/// The targets that a uquery prints, sorted.
async fn query(buck: &Buck, query: &str) -> anyhow::Result<Vec<String>> {
    let res = buck.uquery(&[query]).await?;
    let mut targets: Vec<String> = res.stdout.lines().map(str::to_owned).collect();
    targets.sort();
    Ok(targets)
}

// prelude/lean/library.bzl `covers`
#[buck_test(inplace = false)]
async fn test_queries_find_the_lean_target_that_covers_a_file(buck: &Buck) -> anyhow::Result<()> {
    let spec = ["root//lean_covers:spec"];
    let covered = "lean_covers/code/add.txt";
    assert_eq!(
        query(buck, &format!("kind('^lean_', owner({covered}))")).await?,
        spec
    );
    assert_eq!(
        query(
            buck,
            &format!(r"attrregexfilter(covers, 'code/add\.txt$', owner({covered}))")
        )
        .await?,
        spec
    );

    // A file that the library reads, but does not cover: the library owns it too, and only the
    // query on `covers` tells the two apart.
    let resource = "lean_covers/Spec/notes.txt";
    assert_eq!(
        query(buck, &format!("kind('^lean_', owner({resource}))")).await?,
        spec
    );
    assert_eq!(
        query(
            buck,
            &format!(r"attrregexfilter(covers, 'Spec/notes\.txt$', owner({resource}))")
        )
        .await?,
        Vec::<String>::new()
    );

    assert_eq!(
        query(buck, "owner(lean_covers/code/sub.txt)").await?,
        Vec::<String>::new()
    );
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_covers_attribute_has_the_files_of_a_lean_target(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .uquery(&["--output-attribute", "^covers$", "root//lean_covers:"])
        .await?;
    let targets: Value = serde_json::from_str(&res.stdout)?;
    assert_eq!(
        targets,
        serde_json::json!({
            "root//lean_covers:spec": {
                "covers": ["root//lean_covers/code/add.txt", "root//lean_covers/code/mul.txt"],
            },
            "root//lean_covers:spec_check": {"covers": null},
        })
    );
    Ok(())
}

// prelude/lean/library.bzl `lean_default_info`
#[buck_test(inplace = false)]
async fn test_covers_sub_target_is_a_directory_of_the_files(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["root//lean_covers_dir:covered[covers]"])
        .await?;
    let dir = res
        .build_report()?
        .output_for_sub_target("root//lean_covers_dir:covered", "covers")?;
    assert_eq!(std::fs::read_to_string(dir.join("a.txt"))?, "a\n");
    assert_eq!(
        std::fs::read_to_string(dir.join("sub").join("b.txt"))?,
        "b\n"
    );
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_covers_without_a_file_is_an_error(buck: &Buck) {
    buck.build(&["root//lean_covers_dir:no_file"])
        .await
        .expect_failure()
        .assert_stderr_matches("`covers` names no file");
}

#[buck_test(inplace = false)]
async fn test_a_target_in_covers_is_an_error(buck: &Buck) {
    // The second target names a file of another package through an `export_file`: its output is
    // a source file, at the path of a file that the target covers in its own package.
    for (target, label) in [
        ("output_of_a_target", "root//lean_covers_dir:generated"),
        ("source_of_a_target", "root//lean_covers_other:a_txt"),
    ] {
        buck.build(&[&format!("root//lean_covers_dir:{target}")])
            .await
            .expect_failure()
            .assert_stderr_matches(&format!(
                "`covers` takes files of the package, and `{label}` is a target"
            ));
    }
}

#[buck_test(inplace = false)]
async fn test_build_fails_on_a_covered_file_that_does_not_exist(buck: &Buck) {
    // No action of the default output reads the file. Open source buck2 fails earlier, when it
    // reads the build file.
    buck.build(&["root//lean_covers_missing:missing_file"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"missing\.txt`.*does not exist");
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

#[buck_test(inplace = false, skip_for_os = ["windows", "darwin"])]
async fn test_archive_extraction_rollout(buck: &Buck) -> anyhow::Result<()> {
    // Switching the rollout key must select native extraction and then restore legacy
    // extraction, including their different treatment of literal brackets in excluded names.
    for enabled in [false, true, false] {
        let args = if enabled {
            &["-c", "buck2.extract_archives_in_daemon=true"][..]
        } else {
            &[]
        };
        let out = output(buck, "root//archive:unpacked", args).await?;
        let mut names = std::fs::read_dir(out.join("pages"))?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<anyhow::Result<Vec<_>>>()?;
        names.sort();
        let expected = if enabled {
            vec!["d.js", "i.js", "keep.js"]
        } else {
            vec!["[id].js", "keep.js"]
        };
        assert_eq!(names, expected);
    }
    Ok(())
}

async fn excluding_tar_directory_without_slash(buck: &Buck, native: bool) -> anyhow::Result<()> {
    // A tar directory follows its child and has no trailing slash. Either extraction mode
    // must exclude the directory and its earlier child while retaining an unrelated file.
    let mut archive = tar::Builder::new(Vec::new());
    for (name, contents, kind) in [
        ("top", &b""[..], tar::EntryType::Directory),
        ("top/bin/tool", &b"data"[..], tar::EntryType::Regular),
        ("top/bin", &b""[..], tar::EntryType::Directory),
        ("top/keep", &b"keep"[..], tar::EntryType::Regular),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        archive.append_data(&mut header, name, contents)?;
    }
    let data = archive.into_inner()?;
    assert_eq!(&data[1536..1544], b"top/bin\0");
    let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
    compressed.write_all(&data)?;
    std::fs::write(
        buck.cwd().join("archive/directory.tar.gz"),
        compressed.finish()?,
    )?;

    // The selected implementation must omit the excluded directory and its earlier child.
    let out = output(
        buck,
        "root//archive:directory_exclusion",
        &[
            "-c",
            &format!("buck2.extract_archives_in_daemon={native}"),
            "-c",
            "test.directory_archive=directory.tar.gz",
        ],
    )
    .await?;
    let names = std::fs::read_dir(out)?
        .map(|entry| Ok(entry?.file_name()))
        .collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(names, [std::ffi::OsString::from("keep")]);
    Ok(())
}

#[buck_test(inplace = false, skip_for_os = ["windows", "darwin"])]
async fn test_excluding_tar_directory_without_slash_legacy(buck: &Buck) -> anyhow::Result<()> {
    // Legacy extraction must exclude a directory whose child appears first in the archive.
    excluding_tar_directory_without_slash(buck, false).await
}

#[buck_test(inplace = false, skip_for_os = ["windows", "darwin"])]
async fn test_excluding_tar_directory_without_slash_native(buck: &Buck) -> anyhow::Result<()> {
    // Native extraction must exclude a directory whose child appears first in the archive.
    excluding_tar_directory_without_slash(buck, true).await
}
