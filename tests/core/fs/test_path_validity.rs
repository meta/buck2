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

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::is_open_source;

const SHOW_IGNORED_READS: (&str, &str) = ("BUCK2_SHOW_SOFT_ERRORS", "only=path_check_ignored_read");

const CP_DEFS: &str = concat!(
    "def _impl(ctx):\n",
    "    return [DefaultInfo(default_output = ctx.actions.copy_file(\"out\", ctx.attrs.src))]\n",
    "cp = rule(impl = _impl, attrs = {'src': attrs.source()})\n",
);

/// Creates the symlink `link` to `target`. On Windows it is a directory symlink when `target`,
/// relative to the link's directory, is a directory, as Python's `os.symlink` makes it.
fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
    let (target, link) = (target.as_ref(), link.as_ref());
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

// These tests assert that buck2 refuses to read source files when a path element
// along the way is "invalid" in some sense: wrong-case, a symlink, ignored, or
// crossing a cell boundary. Each test sets up a `TARGETS.fixture` reachable via
// such a path and asks buck to read it; the assertion is that buck rejects.

#[buck_test]
async fn test_target_pattern_wrong_case(buck: &Buck) {
    // `realdir/TARGETS.fixture` exists; querying with `Realdir` is rejected on
    // every platform, whatever the file system makes of the spelling.
    let res = buck.targets(&["//Realdir:", "-v0"]).await.expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        "golden/target_pattern_wrong_case.golden.stderr",
    );
}

#[buck_test]
async fn test_target_pattern_symlink(buck: &Buck) -> anyhow::Result<()> {
    symlink("realdir", buck.cwd().join("linkdir"))?;
    let res = buck.targets(&["//linkdir:", "-v0"]).await.expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        "golden/target_pattern_symlink.golden.stderr",
    );
    Ok(())
}

// A symlink out of the repo is no different from one into it.
#[buck_test]
async fn test_target_pattern_external_symlink(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let outside = tmp_path;
    fs::write(outside.join("TARGETS.fixture"), "stub(name = \"ext\")\n")?;
    symlink(outside, buck.cwd().join("extlink"))?;
    buck.targets(&["//extlink:"])
        .await
        .expect_failure()
        .assert_stderr_matches("path `root//extlink` is a symlink, which buck2 does not follow");
    Ok(())
}

// A symlink is a leaf of the package it is in: a file behind one belongs to that
// package, not to a build file that following the symlink would find.
#[buck_test]
async fn test_owner_behind_symlink(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let outside = tmp_path;
    fs::write(outside.join("f"), "")?;
    symlink(outside, buck.cwd().join("extlink"))?;
    symlink("realdir", buck.cwd().join("linkdir"))?;
    fs::write(buck.cwd().join("defs.bzl"), CP_DEFS)?;
    fs::write(
        buck.cwd().join("TARGETS.fixture"),
        concat!(
            "load(\":defs.bzl\", \"cp\")\n",
            "cp(name = \"in_repo\", src = \"linkdir/f\")\n",
            "cp(name = \"out_of_repo\", src = \"extlink/f\")\n",
        ),
    )?;
    // The directory behind `linkdir` is a package with a target of its own for the file.
    fs::write(buck.cwd().join("realdir").join("f"), "")?;
    fs::write(
        buck.cwd().join("realdir").join("TARGETS.fixture"),
        "load(\"//:defs.bzl\", \"cp\")\ncp(name = \"inner\", src = \"f\")\n",
    )?;

    let owners = async |path: &str| -> anyhow::Result<String> {
        Ok(buck.uquery(&["owner(%s)", path]).await?.stdout)
    };
    assert_eq!(owners("linkdir/f").await?, "root//:in_repo\n");
    assert_eq!(owners("extlink/f").await?, "root//:out_of_repo\n");
    assert_eq!(owners("realdir/f").await?, "root//realdir:inner\n");
    Ok(())
}

#[buck_test]
async fn test_target_pattern_ignored(buck: &Buck) {
    // `ignored*` is listed in `[project] ignore` (matching the dir but not its
    // contents); a `TARGETS.fixture` reachable only by traversing through an
    // ignored directory is rejected. NB: a bare `ignore = ignored` entry is
    // silently rewritten to also cover `ignored/**`, which would not exercise
    // the path-element check — hence the glob form.
    let res = buck
        .targets(&["//ignored/inside:", "-v0"])
        .await
        .expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        "golden/target_pattern_ignored.golden.stderr",
    );
}

#[buck_test(allow_soft_errors = true)]
async fn test_source_ignored(buck: &Buck) -> anyhow::Result<()> {
    // Ignored source files named explicitly are read, with a soft error saying
    // why the read will be refused.
    fs::write(buck.cwd().join("ignored_src"), "HELLO\n")?;
    fs::write(buck.cwd().join("defs.bzl"), CP_DEFS)?;
    fs::write(
        buck.cwd().join("TARGETS.fixture"),
        "load(\":defs.bzl\", \"cp\")\ncp(name = \"cp\", src = \"ignored_src\")\n",
    )?;
    let build = buck
        .build(&["//:cp"])
        .env(SHOW_IGNORED_READS.0, SHOW_IGNORED_READS.1);
    // Open source buck2 does not load a package with a source that is ignored.
    if is_open_source() {
        build.await.expect_failure().assert_stderr_matches(
            "Source file `ignored_src` does not exist as a member of package `root//`",
        );
        return Ok(());
    }
    let res = build.await?;
    assert!(
        res.stderr
            .contains("`ignored_src` matches the `project.ignore` pattern `ignored*`"),
        "{}",
        res.stderr
    );
    assert!(
        res.stderr
            .contains("reading ignored files will become an error"),
        "{}",
        res.stderr
    );
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_source_ignored_behind_symlink(buck: &Buck) -> anyhow::Result<()> {
    // One report, naming the symlink, for an ignored source reached through it.
    fs::create_dir(buck.cwd().join("real"))?;
    fs::write(buck.cwd().join("real").join("f"), "HELLO\n")?;
    fs::create_dir(buck.cwd().join("ignored_dir"))?;
    symlink(
        Path::new("..").join("real"),
        buck.cwd().join("ignored_dir").join("link"),
    )?;
    fs::write(buck.cwd().join("defs.bzl"), CP_DEFS)?;
    fs::write(
        buck.cwd().join("TARGETS.fixture"),
        "load(\":defs.bzl\", \"cp\")\ncp(name = \"cp\", src = \"ignored_dir/link/f\")\n",
    )?;
    let build = buck
        .build(&["//:cp"])
        .env(SHOW_IGNORED_READS.0, SHOW_IGNORED_READS.1);
    if is_open_source() {
        build.await.expect_failure().assert_stderr_matches(
            "Source file `ignored_dir/link/f` does not exist as a member of package `root//`",
        );
        return Ok(());
    }
    let res = build.await?;
    assert_eq!(
        res.stderr
            .matches("matches the `project.ignore` pattern")
            .count(),
        1,
        "{}",
        res.stderr
    );
    assert!(
        res.stderr
            .contains("`ignored_dir/link` is under `ignored_dir`"),
        "{}",
        res.stderr
    );
    Ok(())
}

#[buck_test]
async fn test_target_pattern_cell_traverse(buck: &Buck) {
    // `subcell/` is its own cell. A `root//` target pattern must not be
    // able to reach into it.
    let res = buck
        .targets(&["root//subcell/subdir:", "-v0"])
        .await
        .expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        "golden/target_pattern_cell_traverse.golden.stderr",
    );
}

#[buck_test]
async fn test_load_cell_traverse(buck: &Buck) -> anyhow::Result<()> {
    // The analogue of `test_target_pattern_cell_traverse` for `load()`.
    fs::create_dir(buck.cwd().join("loader"))?;
    fs::write(
        buck.cwd().join("loader").join("TARGETS.fixture"),
        "load(\"//subcell/subdir:lib.bzl\", \"value\")\nstub(name = \"loader_\" + value)\n",
    )?;
    let res = buck.targets(&["//loader:", "-v0"]).await.expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        "golden/load_cell_traverse.golden.stderr",
    );
    Ok(())
}

#[buck_test]
async fn test_load_symlink(buck: &Buck) -> anyhow::Result<()> {
    symlink("realdir", buck.cwd().join("linkdir"))?;
    fs::create_dir(buck.cwd().join("loader"))?;
    fs::write(
        buck.cwd().join("loader").join("TARGETS.fixture"),
        "load(\"//linkdir:lib.bzl\", \"value\")\nstub(name = \"loader_\" + value)\n",
    )?;
    let res = buck.targets(&["//loader:", "-v0"]).await.expect_failure();
    golden(
        &sanitize_stderr(&res.stderr),
        "golden/load_symlink.golden.stderr",
    );
    Ok(())
}
