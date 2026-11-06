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
use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::expect_exec_count;

/// Creates the symlink `link` to `target`. On Windows it is a directory symlink when `target`,
/// relative to the link's directory, is a directory, as Python's `os.symlink` makes it.
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        let resolved = link.parent().unwrap_or(Path::new(".")).join(target);
        if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
}

/// Removes whatever is at `path`, as Python's `shutil.rmtree` and `unlink` do between them: on
/// Windows a symlink to a directory is a directory entry, which `remove_file` refuses.
fn remove_path(path: &Path, file_type: fs::FileType) -> std::io::Result<()> {
    if file_type.is_dir() {
        return fs::remove_dir_all(path);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if file_type.is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }
    fs::remove_file(path)
}

fn setup_symlink(symlink_path: &Path, target: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(symlink_path.parent().expect("a symlink in a directory"))?;

    match symlink_path.symlink_metadata() {
        Ok(metadata) => remove_path(symlink_path, metadata.file_type())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    symlink(target, symlink_path)?;
    Ok(())
}

#[buck_test(extra_buck_config = [("buck2", "use_correct_source_symlink_reading", "true")])]
async fn test_symlink_target_tracked_for_rebuild(buck: &Buck) -> anyhow::Result<()> {
    setup_symlink(
        &buck.cwd().join("src").join("link"),
        &Path::new("..").join("dir"),
    )?;

    buck.build(&["//:cp"]).await?;
    expect_exec_count(buck, 1).await?;

    buck.build(&["//:cp"]).await?;
    expect_exec_count(buck, 0).await?;

    fs::write(buck.cwd().join("dir").join("file"), "GOODBYE\n")?;

    // This isn't really behavior  we want to guarantee and we'd rather users
    // don't use symlinks, but this is very observable (and it's not worse than
    // just reading the files then pretending they are never used!)
    buck.build(&["//:cp"]).await?;
    expect_exec_count(buck, 1).await?;
    Ok(())
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "use_correct_source_symlink_reading", "true")],
)]
async fn test_symlinks_redirection(buck: &Buck) -> anyhow::Result<()> {
    setup_symlink(
        &buck.cwd().join("src").join("link"),
        &Path::new("..").join("dir"),
    )?;

    buck.build(&["//:cp"]).await?;
    expect_exec_count(buck, 1).await?;

    buck.build(&["//:cp"]).await?;
    expect_exec_count(buck, 0).await?;

    // We change the symlink which should invalidate all files depending on it
    setup_symlink(
        &buck.cwd().join("src").join("link"),
        &Path::new("..").join("dir2"),
    )?;

    buck.build(&["//:cp"]).await?;
    expect_exec_count(buck, 1).await?;
    Ok(())
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "use_correct_source_symlink_reading", "true")],
)]
async fn test_symlinks_external(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let top_level = tmp_path;

    fs::create_dir(top_level.join("nested1"))?;
    fs::create_dir(top_level.join("nested2"))?;
    fs::write(top_level.join("nested1").join("file"), "HELLO")?;
    fs::write(top_level.join("nested2").join("file"), "GOODBYE")?;

    setup_symlink(
        &buck.cwd().join("ext").join("link"),
        &top_level.join("nested1"),
    )?;

    buck.build(&["//:ext"]).await?;
    expect_exec_count(buck, 1).await?;

    buck.build(&["//:ext"]).await?;
    expect_exec_count(buck, 0).await?;

    setup_symlink(
        &buck.cwd().join("ext").join("link"),
        &top_level.join("nested2"),
    )?;

    buck.build(&["//:ext"]).await?;
    expect_exec_count(buck, 1).await?;
    Ok(())
}

#[buck_test(extra_buck_config = [("buck2", "use_correct_source_symlink_reading", "true")])]
async fn test_no_read_through_symlinks(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build_without_report(&["//:stat_symlink", "--out", "-", "--remote-only"])
        .await?;
    // Just check that we don't always return `True`
    assert_eq!(res.stdout.trim(), "False");

    setup_symlink(
        &buck.cwd().join("src").join("link"),
        &Path::new("..").join("dir"),
    )?;

    let res = buck
        .build_without_report(&["//:stat_symlink", "--out", "-", "--remote-only"])
        .await?;
    assert_eq!(res.stdout.trim(), "True");

    let res = buck
        .build_without_report(&["//:stat_symlink_in_dir", "--out", "-", "--remote-only"])
        .await?;
    assert_eq!(res.stdout.trim(), "True");
    Ok(())
}

#[buck_test(extra_buck_config = [("buck2", "use_correct_source_symlink_reading", "true")])]
async fn test_no_read_through_source_symlinks_to_file(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build_without_report(&["//:stat_symlink", "--out", "-", "--remote-only"])
        .await?;
    // Just check that we don't always return `True`
    assert_eq!(res.stdout.trim(), "False");

    setup_symlink(
        &buck.cwd().join("src").join("link"),
        &Path::new("..").join("dir").join("file"),
    )?;

    let res = buck
        .build_without_report(&["//:stat_symlink", "--out", "-", "--remote-only"])
        .await?;
    assert_eq!(res.stdout.trim(), "True");
    Ok(())
}

#[buck_test(extra_buck_config = [("buck2", "use_correct_source_symlink_reading", "true")])]
async fn test_no_read_through_source_symlinks_to_in_symlink_target(
    buck: &Buck,
) -> anyhow::Result<()> {
    for s in [Path::new("dir").to_owned(), Path::new("dir2").join("dir")] {
        fs::create_dir_all(buck.cwd().join(&s))?;
        fs::write(
            buck.cwd().join(&s).join("file"),
            s.to_string_lossy().replace('\\', "/"),
        )?;
    }
    setup_symlink(
        &buck.cwd().join("redirectvia"),
        &Path::new("dir2").join("dir"),
    )?;

    setup_symlink(
        &buck.cwd().join("src").join("link"),
        &Path::new("..")
            .join("redirectvia")
            .join("..")
            .join("dir")
            .join("file"),
    )?;

    let res = buck
        .build_without_report(&["//:cp_src_link_via_builtin", "--out", "-"])
        .await?;
    // FIXME(JakobDegen): Should be `dir2/dir`. The fact that `redirectvia`, found in the symlink
    // target, is itself a symlink is completely ignored
    assert_eq!(res.stdout.trim(), "dir");
    Ok(())
}

#[buck_test(setup_eden = true, allow_soft_errors = true)]
async fn test_eden_io_read_symlink_dir_build_target(buck: &Buck) -> anyhow::Result<()> {
    setup_symlink(
        &buck.cwd().join("testlink"),
        &buck.cwd().join("symdir").join("dir"),
    )?;

    buck.build(&["//:symlink_dep"]).await?;
    Ok(())
}

#[buck_test(setup_eden = true, allow_soft_errors = true)]
async fn test_eden_io_read_symlink_dir_list_target(buck: &Buck) -> anyhow::Result<()> {
    setup_symlink(&buck.cwd().join("testlink"), &buck.cwd().join("symdir"))?;

    buck.targets(&["//testlink/dir:"]).await?;
    Ok(())
}
