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
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use buck2_e2e::prelude::*;

/// The path `components` lead to under `root`.
fn join(root: &Path, components: &[&str]) -> PathBuf {
    components.iter().fold(root.to_owned(), |p, c| p.join(c))
}

/// `path` with every symlink resolved, like Python's `Path.resolve()`: on Windows, without the
/// `\\?\` prefix of `canonicalize`.
fn resolve(path: &Path) -> std::io::Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    let stripped = resolved
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .map(PathBuf::from);
    Ok(match stripped {
        Some(stripped) if cfg!(windows) => stripped,
        _ => resolved,
    })
}

#[cfg(unix)]
fn symlink(original: &str, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink(original: &str, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(original, link)
}

#[buck_test]
async fn test_unhashed_putputs(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//pack:trivial_build"]).await?;

    let p = join(
        buck.cwd(),
        &["buck-out", "v2", "gen", "root", "pack", "foo.txt"],
    );
    assert!(p.exists());
    assert!(p.is_symlink());
    Ok(())
}

#[buck_test]
async fn test_projected_output(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:projected_output"]).await?;

    let p = join(buck.cwd(), &["buck-out", "v2", "gen", "root", "dir"]);
    assert!(p.exists());
    assert!(p.is_symlink());
    assert!(p.join("file").is_file());
    Ok(())
}

#[buck_test]
async fn test_materializer_managed_unhashed_output(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//pack:trivial_build"]).await?;

    let p = join(
        buck.cwd(),
        &["buck-out", "v2", "gen", "root", "pack", "foo.txt"],
    );
    assert!(p.is_symlink());

    let materializer_state = buck.audit(&["deferred-materializer", "list"]).await?;
    assert!(
        materializer_state
            .stdout
            .contains("buck-out/v2/gen/root/pack/foo.txt")
    );

    let future_time = (SystemTime::now() + Duration::from_secs(7 * 7 * 24 * 60 * 60))
        .duration_since(UNIX_EPOCH)?
        .as_secs();
    buck.clean(&[&format!("--keep-since-time={future_time}")])
        .await?;
    assert!(p.is_symlink());
    assert!(p.is_file());
    let materializer_state = buck.audit(&["deferred-materializer", "list"]).await?;
    assert!(
        materializer_state
            .stdout
            .contains("buck-out/v2/gen/root/pack/foo.txt")
    );
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_materializer_managed_unhashed_output_without_materialization(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&["//pack:trivial_build", "--materializations=none"])
        .await?;
    let unhashed = join(
        buck.cwd(),
        &["buck-out", "v2", "gen", "root", "pack", "foo.txt"],
    );

    assert!(!unhashed.is_symlink());

    let materializer_state = buck.audit(&["deferred-materializer", "list"]).await?;
    assert!(
        materializer_state
            .stdout
            .contains("buck-out/v2/gen/root/pack/foo.txt")
    );
    Ok(())
}

#[buck_test]
async fn test_build_symlink_does_not_traverse_existing_symlinks(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//pack:trivial_build"]).await?;
    let symlink_folder = join(buck.cwd(), &["buck-out", "v2", "gen", "root", "pack"]);

    // Now, overwrite part of the symlink path with something we cannot traverse.
    let path = symlink_folder.parent().expect("a parent");
    std::fs::remove_dir_all(path)?;
    // On Windows this is just non existing path.
    symlink("/dev/null", path)?;

    // Can we still build? If we delete the symlink when walking up the path, we
    // can. If we traverse it, we can't.
    buck.build(&["//pack:trivial_build"]).await?;
    Ok(())
}

fn base_checks(
    symlink_path: &Path,
    content_based_path: &Path,
    subtarget_output: &Path,
    should_symlink_exist: bool,
) -> anyhow::Result<()> {
    if should_symlink_exist {
        assert!(symlink_path.is_symlink());
        assert!(resolve(symlink_path)?.is_file());
    } else {
        assert!(!symlink_path.exists());
    }

    assert!(content_based_path.is_dir());
    assert!(subtarget_output.is_symlink());
    let resolved = resolve(subtarget_output)?;
    assert!(resolved.is_file());
    assert!(!resolved.starts_with(symlink_path));
    assert!(resolved.starts_with(content_based_path));
    // Verify we can read the contents of the file
    std::fs::read_to_string(subtarget_output)?;
    Ok(())
}

#[buck_test]
async fn test_conflict_with_content_based_paths(buck: &Buck) -> anyhow::Result<()> {
    let symlink_path = join(
        buck.cwd(),
        &["buck-out", "v2", "gen", "root", "conflict", "shared_name"],
    );
    let content_based_path = join(
        buck.cwd(),
        &["buck-out", "v2", "art", "root", "conflict", "shared_name"],
    );
    // sanity check that we're starting from a clean state
    assert!(!symlink_path.exists());
    assert!(!content_based_path.exists());

    //
    // Build just the subtarget. Esnsure that the subtarget output exists and is
    // reacable, and that it lives in the place we expect.
    //
    let res = buck
        .build(&[
            "//conflict/shared_name:subtarget",
            "--config",
            "buck2.create_unhashed_links=false",
        ])
        .await?;
    let subtarget_output = res
        .build_report()?
        .output_for_target("root//conflict/shared_name:subtarget")?;
    base_checks(&symlink_path, &content_based_path, &subtarget_output, false)?;

    //
    // Build the conflicting target w/o unhashed links. This should leave the
    // subtarget_output alone, which should remain readable.
    //
    buck.build(&[
        "//conflict:shared_name",
        "--config",
        "buck2.create_unhashed_links=false",
    ])
    .await?;
    // TODO(jtbraun): this should instead ensure the symlink does NOT exist, and the content_based_path does and is a folder
    base_checks(&symlink_path, &content_based_path, &subtarget_output, false)?;

    //
    // Build the conflicting target with unhashed links. This will overwrite the
    // subtarget with a directory, and the symlink_path will now exist.
    //
    buck.build(&["//conflict:shared_name"]).await?;
    base_checks(&symlink_path, &content_based_path, &subtarget_output, true)?;
    Ok(())
}

#[buck_test]
async fn test_projected_symlink_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:projected_symlink_output"]).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:projected_symlink_output")?;

    assert!(output.parent().expect("a parent").is_symlink());
    assert!(output.is_symlink());
    assert!(resolve(&output)?.is_file());
    Ok(())
}
