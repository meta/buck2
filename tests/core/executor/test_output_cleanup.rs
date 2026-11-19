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
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::prelude::*;

/// The directories under `top`, `top` first, each with its subdirectories and its other entries,
/// as Python's `os.walk(top)` lists them: a symlink to a directory counts as a directory but is not
/// walked, and a directory that cannot be listed (or is not a directory) is left out.
fn walk(top: &Path) -> Vec<(PathBuf, Vec<PathBuf>, Vec<PathBuf>)> {
    let Ok(entries) = fs::read_dir(top) else {
        return Vec::new();
    };
    let (dirs, files): (Vec<_>, Vec<_>) = entries.flatten().partition(is_dir);
    let dirs: Vec<PathBuf> = dirs.iter().map(fs::DirEntry::path).collect();
    let files = files.iter().map(fs::DirEntry::path).collect();
    let subdirs: Vec<_> = dirs
        .iter()
        .filter(|dir| !dir.is_symlink())
        .flat_map(|dir| walk(dir))
        .collect();
    std::iter::once((top.to_owned(), dirs, files))
        .chain(subdirs)
        .collect()
}

/// `os.DirEntry.is_dir()`: it follows symlinks, and is false where that fails.
fn is_dir(entry: &fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(file_type) if file_type.is_symlink() => entry.path().is_dir(),
        Ok(file_type) => file_type.is_dir(),
        Err(_) => false,
    }
}

#[derive(Debug, PartialEq)]
enum Entry {
    File(String),
    Dir,
}

fn read_dir(d: &Path) -> anyhow::Result<BTreeMap<PathBuf, Entry>> {
    let mut out = BTreeMap::new();

    for (_, dirs, files) in walk(d) {
        for path in files {
            let contents = fs::read_to_string(&path)?;
            out.insert(path.strip_prefix(d)?.to_owned(), Entry::File(contents));
        }
        for path in dirs {
            out.entry(path.strip_prefix(d)?.to_owned())
                .or_insert(Entry::Dir);
        }
    }

    Ok(out)
}

async fn output_cleanup(
    buck: &Buck,
    tmp_path: &Path,
    first: &str,
    second: &str,
) -> anyhow::Result<()> {
    let rebuild = tmp_path.join("rebuild");
    let clean = tmp_path.join("clean");
    let first = format!("{first}-a");
    let second = format!("{second}-b");

    buck.build(&[":main", "-c", &format!("test.main={first}")])
        .await?;
    buck.build(&[
        ":main",
        "-c",
        &format!("test.main={second}"),
        "--out",
        &rebuild.to_string_lossy(),
    ])
    .await?;

    buck.clean(&[]).await?;
    buck.build(&[
        ":main",
        "-c",
        &format!("test.main={second}"),
        "--out",
        &clean.to_string_lossy(),
    ])
    .await?;

    assert_eq!(read_dir(&rebuild)?, read_dir(&clean)?);
    Ok(())
}

/// One test per case: the attribute, the test name, then the arguments of `output_cleanup` after
/// `buck` and `tmp_path`.
macro_rules! output_cleanup_cases {
    ($(#[$meta:meta] async fn $name:ident => ($first:literal, $second:literal);)*) => {
        $(
            #[$meta]
            async fn $name(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
                output_cleanup(buck, tmp_path, $first, $second).await
            }
        )*
    };
}

// The cases are named `test_output_cleanup_<first>_<second>`, as the Python harness named them.
output_cleanup_cases! {
    #[buck_test]
    async fn test_output_cleanup_local_action_local_action =>
        ("local_action", "local_action");
    #[buck_test]
    async fn test_output_cleanup_local_action_remote_action =>
        ("local_action", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_local_action_symlinked_dir =>
        ("local_action", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_local_action_write =>
        ("local_action", "write");
    #[buck_test]
    async fn test_output_cleanup_local_action_copy =>
        ("local_action", "copy");
    #[buck_test]
    async fn test_output_cleanup_local_action_local_readonly_file =>
        ("local_action", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_action_local_readonly_dir =>
        ("local_action", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_action_local_nonexec_dir =>
        ("local_action", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_action_remote_readonly_file =>
        ("local_action", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_action_remote_readonly_dir =>
        ("local_action", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_action_remote_nonexec_dir =>
        ("local_action", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_action_local_action =>
        ("remote_action", "local_action");
    #[buck_test]
    async fn test_output_cleanup_remote_action_remote_action =>
        ("remote_action", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_remote_action_symlinked_dir =>
        ("remote_action", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_action_write =>
        ("remote_action", "write");
    #[buck_test]
    async fn test_output_cleanup_remote_action_copy =>
        ("remote_action", "copy");
    #[buck_test]
    async fn test_output_cleanup_remote_action_local_readonly_file =>
        ("remote_action", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_action_local_readonly_dir =>
        ("remote_action", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_action_local_nonexec_dir =>
        ("remote_action", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_action_remote_readonly_file =>
        ("remote_action", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_action_remote_readonly_dir =>
        ("remote_action", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_action_remote_nonexec_dir =>
        ("remote_action", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_local_action =>
        ("symlinked_dir", "local_action");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_remote_action =>
        ("symlinked_dir", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_symlinked_dir =>
        ("symlinked_dir", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_write =>
        ("symlinked_dir", "write");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_copy =>
        ("symlinked_dir", "copy");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_local_readonly_file =>
        ("symlinked_dir", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_local_readonly_dir =>
        ("symlinked_dir", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_local_nonexec_dir =>
        ("symlinked_dir", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_remote_readonly_file =>
        ("symlinked_dir", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_remote_readonly_dir =>
        ("symlinked_dir", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_symlinked_dir_remote_nonexec_dir =>
        ("symlinked_dir", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_write_local_action =>
        ("write", "local_action");
    #[buck_test]
    async fn test_output_cleanup_write_remote_action =>
        ("write", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_write_symlinked_dir =>
        ("write", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_write_write =>
        ("write", "write");
    #[buck_test]
    async fn test_output_cleanup_write_copy =>
        ("write", "copy");
    #[buck_test]
    async fn test_output_cleanup_write_local_readonly_file =>
        ("write", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_write_local_readonly_dir =>
        ("write", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_write_local_nonexec_dir =>
        ("write", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_write_remote_readonly_file =>
        ("write", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_write_remote_readonly_dir =>
        ("write", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_write_remote_nonexec_dir =>
        ("write", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_copy_local_action =>
        ("copy", "local_action");
    #[buck_test]
    async fn test_output_cleanup_copy_remote_action =>
        ("copy", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_copy_symlinked_dir =>
        ("copy", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_copy_write =>
        ("copy", "write");
    #[buck_test]
    async fn test_output_cleanup_copy_copy =>
        ("copy", "copy");
    #[buck_test]
    async fn test_output_cleanup_copy_local_readonly_file =>
        ("copy", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_copy_local_readonly_dir =>
        ("copy", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_copy_local_nonexec_dir =>
        ("copy", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_copy_remote_readonly_file =>
        ("copy", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_copy_remote_readonly_dir =>
        ("copy", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_copy_remote_nonexec_dir =>
        ("copy", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_local_action =>
        ("local_readonly_file", "local_action");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_remote_action =>
        ("local_readonly_file", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_symlinked_dir =>
        ("local_readonly_file", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_write =>
        ("local_readonly_file", "write");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_copy =>
        ("local_readonly_file", "copy");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_local_readonly_file =>
        ("local_readonly_file", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_local_readonly_dir =>
        ("local_readonly_file", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_local_nonexec_dir =>
        ("local_readonly_file", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_remote_readonly_file =>
        ("local_readonly_file", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_remote_readonly_dir =>
        ("local_readonly_file", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_file_remote_nonexec_dir =>
        ("local_readonly_file", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_local_action =>
        ("local_readonly_dir", "local_action");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_remote_action =>
        ("local_readonly_dir", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_symlinked_dir =>
        ("local_readonly_dir", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_write =>
        ("local_readonly_dir", "write");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_copy =>
        ("local_readonly_dir", "copy");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_local_readonly_file =>
        ("local_readonly_dir", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_local_readonly_dir =>
        ("local_readonly_dir", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_local_nonexec_dir =>
        ("local_readonly_dir", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_remote_readonly_file =>
        ("local_readonly_dir", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_remote_readonly_dir =>
        ("local_readonly_dir", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_readonly_dir_remote_nonexec_dir =>
        ("local_readonly_dir", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_local_action =>
        ("local_nonexec_dir", "local_action");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_remote_action =>
        ("local_nonexec_dir", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_symlinked_dir =>
        ("local_nonexec_dir", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_write =>
        ("local_nonexec_dir", "write");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_copy =>
        ("local_nonexec_dir", "copy");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_local_readonly_file =>
        ("local_nonexec_dir", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_local_readonly_dir =>
        ("local_nonexec_dir", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_local_nonexec_dir =>
        ("local_nonexec_dir", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_remote_readonly_file =>
        ("local_nonexec_dir", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_remote_readonly_dir =>
        ("local_nonexec_dir", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_local_nonexec_dir_remote_nonexec_dir =>
        ("local_nonexec_dir", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_local_action =>
        ("remote_readonly_file", "local_action");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_remote_action =>
        ("remote_readonly_file", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_symlinked_dir =>
        ("remote_readonly_file", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_write =>
        ("remote_readonly_file", "write");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_copy =>
        ("remote_readonly_file", "copy");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_local_readonly_file =>
        ("remote_readonly_file", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_local_readonly_dir =>
        ("remote_readonly_file", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_local_nonexec_dir =>
        ("remote_readonly_file", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_remote_readonly_file =>
        ("remote_readonly_file", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_remote_readonly_dir =>
        ("remote_readonly_file", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_file_remote_nonexec_dir =>
        ("remote_readonly_file", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_local_action =>
        ("remote_readonly_dir", "local_action");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_remote_action =>
        ("remote_readonly_dir", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_symlinked_dir =>
        ("remote_readonly_dir", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_write =>
        ("remote_readonly_dir", "write");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_copy =>
        ("remote_readonly_dir", "copy");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_local_readonly_file =>
        ("remote_readonly_dir", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_local_readonly_dir =>
        ("remote_readonly_dir", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_local_nonexec_dir =>
        ("remote_readonly_dir", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_remote_readonly_file =>
        ("remote_readonly_dir", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_remote_readonly_dir =>
        ("remote_readonly_dir", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_readonly_dir_remote_nonexec_dir =>
        ("remote_readonly_dir", "remote_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_local_action =>
        ("remote_nonexec_dir", "local_action");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_remote_action =>
        ("remote_nonexec_dir", "remote_action");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_symlinked_dir =>
        ("remote_nonexec_dir", "symlinked_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_write =>
        ("remote_nonexec_dir", "write");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_copy =>
        ("remote_nonexec_dir", "copy");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_local_readonly_file =>
        ("remote_nonexec_dir", "local_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_local_readonly_dir =>
        ("remote_nonexec_dir", "local_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_local_nonexec_dir =>
        ("remote_nonexec_dir", "local_nonexec_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_remote_readonly_file =>
        ("remote_nonexec_dir", "remote_readonly_file");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_remote_readonly_dir =>
        ("remote_nonexec_dir", "remote_readonly_dir");
    #[buck_test]
    async fn test_output_cleanup_remote_nonexec_dir_remote_nonexec_dir =>
        ("remote_nonexec_dir", "remote_nonexec_dir");
}

/// A Python `st_mode`.
#[cfg(unix)]
fn st_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode()
}

/// A Python `st_mode`, which Windows file attributes make up: a directory is executable, a
/// read-only entry is not writable, and a symlink keeps its target's permission bits.
#[cfg(windows)]
fn st_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    let attributes = metadata.file_attributes();
    let kind = if metadata.file_type().is_symlink() {
        0o120000
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        0o040000
    } else {
        0o100000
    };
    let executable = if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        0o111
    } else {
        0
    };
    let permissions = if attributes & FILE_ATTRIBUTE_READONLY != 0 {
        0o444
    } else {
        0o666
    };
    kind | executable | permissions
}

type Entries = BTreeMap<String, (u32, Option<String>)>;

fn add_entry(
    ret: &mut Entries,
    root: &Path,
    path: &Path,
    with_file_contents: bool,
) -> anyhow::Result<()> {
    let key = path.strip_prefix(root)?;
    let s = fs::symlink_metadata(path)?;
    let contents = if path.is_file() && with_file_contents {
        Some(fs::read_to_string(path)?)
    } else {
        None
    };
    // `Path("x").relative_to("x")` is `.` in Python.
    let key = if key.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        key.to_string_lossy().into_owned()
    };
    ret.insert(key, (st_mode(&s), contents));
    Ok(())
}

fn get_entries(root: &Path, with_file_contents: bool) -> anyhow::Result<Entries> {
    let mut ret = Entries::new();
    if root.is_file() {
        add_entry(&mut ret, root, root, with_file_contents)?;
    } else {
        for (dirname, _, filenames) in walk(root) {
            add_entry(&mut ret, root, &dirname, with_file_contents)?;
            for filename in filenames {
                add_entry(&mut ret, root, &filename, with_file_contents)?;
            }
        }
    }
    Ok(ret)
}

async fn permissions_match_local_remote(buck: &Buck, kind: &str) -> anyhow::Result<()> {
    let target = "root//:main";
    let lhs = format!("local_{kind}-a");
    let rhs = format!("remote_{kind}-a");

    let lhs_res = buck
        .build(&[":main", "-c", &format!("test.main={lhs}")])
        .await?
        .build_report()?
        .output_for_target(target)?;
    let lhs_entries = get_entries(&lhs_res, false)?;
    let lhs_entries_and_contents = get_entries(&lhs_res, true)?;

    buck.clean(&[]).await?;
    let rhs_res = buck
        .build(&[":main", "-c", &format!("test.main={rhs}")])
        .await?
        .build_report()?
        .output_for_target(target)?;
    let rhs_entries = get_entries(&rhs_res, false)?;
    let rhs_entries_and_contents = get_entries(&rhs_res, true)?;

    assert_eq!(
        lhs_entries, rhs_entries,
        "Permissions mismatch between local and remote: lhs_entries != rhs_entries"
    );
    assert_ne!(
        lhs_entries_and_contents, rhs_entries_and_contents,
        "Test error, contents of files weren't different"
    );
    Ok(())
}

#[buck_test]
async fn test_permissions_match_local_remote_readonly_file(buck: &Buck) -> anyhow::Result<()> {
    permissions_match_local_remote(buck, "readonly_file").await
}

#[buck_test]
async fn test_permissions_match_local_remote_readonly_dir(buck: &Buck) -> anyhow::Result<()> {
    permissions_match_local_remote(buck, "readonly_dir").await
}

#[buck_test]
async fn test_permissions_match_local_remote_nonexec_dir(buck: &Buck) -> anyhow::Result<()> {
    permissions_match_local_remote(buck, "nonexec_dir").await
}
