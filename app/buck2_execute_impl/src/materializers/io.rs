/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_error::ErrorTag;
use buck2_execute::directory::ActionDirectory;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionDirectoryRef;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::execute::blocking::IoRequest;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutMap;

pub struct MaterializeTreeStructure {
    pub path: ProjectRelativePathBuf,
    pub entry: ActionDirectoryEntry<ActionSharedDirectory>,
}

impl IoRequest for MaterializeTreeStructure {
    fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()> {
        materialize_dirs_and_syms(self.entry.as_ref(), project_fs.root().join(&self.path))?;

        Ok(())
    }
}

/// Materializes the entry at `dest`.
///
/// - `materialize_dirs_and_syms`: if `true`, materializes directories and
///   symlinks.
/// - `file_src`: takes the destination path of a file, and returns its
///   source path (where it should be copied from). If it returns [`None`],
///   the file is not materialized.
fn materialize<F, D>(
    entry: DirectoryEntry<&D, &ActionDirectoryMember>,
    dest: &AbsNormPath,
    materialize_dirs_and_syms: bool,
    mut file_src: F,
    executable_bit_override: Option<bool>,
) -> buck2_error::Result<()>
where
    F: FnMut(&AbsNormPath) -> Option<AbsNormPathBuf>,
    D: ActionDirectory,
{
    let mut dest = dest.to_owned();
    if materialize_dirs_and_syms {
        // create the directory where we'll materialize the entry
        if let Some(parent) = dest.parent() {
            fs_util::create_dir_all(parent)?;
        }
    }
    materialize_recursively(
        entry.map_dir(|d| Directory::as_ref(d)),
        &mut dest,
        materialize_dirs_and_syms,
        &mut file_src,
        executable_bit_override,
    )
}

/// Materializes the directories and symlinks of an entry at `dest`. Files
/// are not materialized.
pub(crate) fn materialize_dirs_and_syms<P, D>(
    entry: DirectoryEntry<&D, &ActionDirectoryMember>,
    dest: P,
) -> buck2_error::Result<()>
where
    P: AsRef<AbsNormPath>,
    D: ActionDirectory,
{
    materialize(entry, dest.as_ref(), true, |_: &AbsNormPath| None, None)
}

/// Materializes the files of an the entry rooted at `dest`.
///
/// Files are copied from `src`. In other words, if a file would be
/// materialized at `dest/p`, then it's copied from `src/p`.
pub(crate) fn materialize_files<P, D>(
    entry: DirectoryEntry<&D, &ActionDirectoryMember>,
    src: P,
    dest: P,
    executable_bit_override: Option<bool>,
    preserve_mtimes: bool,
) -> buck2_error::Result<()>
where
    P: AsRef<AbsNormPath>,
    D: ActionDirectory,
{
    let src = src.as_ref();
    let dest = dest.as_ref();
    let file_src = |d: &AbsNormPath| {
        // It's safe to unwrap because `materialize_impl` always gives us a
        // path inside `dest`.
        let subpath = d.strip_prefix(dest).unwrap();
        if subpath.as_str().is_empty() {
            // `dest` itself is a file
            Some(src.to_buf())
        } else {
            Some(src.join(subpath))
        }
    };
    materialize(
        entry.clone(),
        dest,
        false,
        file_src,
        executable_bit_override,
    )?;
    if preserve_mtimes {
        copy_mtimes(
            entry.map_dir(|d| Directory::as_ref(d)),
            &mut src.to_owned(),
            &mut dest.to_owned(),
        )?;
    }
    Ok(())
}

fn copy_mtimes<'a, D>(
    entry: DirectoryEntry<D, &ActionDirectoryMember>,
    src: &mut AbsNormPathBuf,
    dest: &mut AbsNormPathBuf,
) -> buck2_error::Result<()>
where
    D: ActionDirectoryRef<'a>,
{
    if let DirectoryEntry::Dir(d) = entry {
        for (name, entry) in d.entries() {
            src.push(name);
            dest.push(name);
            copy_mtimes(entry, src, dest)?;
            src.pop();
            dest.pop();
        }
    }
    // Creating directory entries changes the directory's mtime.
    fs_util::copy_mtime(&src, &dest).categorize_internal()
}

/// Materializes the files of an entry rooted at `dest`.
///
/// For a file at path `file_dest` in the entry, if `file_dest` exists in
/// `srcs` with value `file_src`, the file is copied from `file_src` to
/// `file_dest`. It's then removed from `srcs`.
fn _materialize_files_from_map<P, D>(
    entry: DirectoryEntry<&D, &ActionDirectoryMember>,
    srcs: &mut BuckMutMap<AbsNormPathBuf, AbsNormPathBuf>,
    dest: P,
) -> buck2_error::Result<()>
where
    P: AsRef<AbsNormPath>,
    D: ActionDirectory,
{
    let file_src = |d: &AbsNormPath| srcs.remove(d);
    materialize(entry, dest.as_ref(), false, file_src, None)
}

fn materialize_recursively<'a, F, D>(
    entry: DirectoryEntry<D, &ActionDirectoryMember>,
    dest: &mut AbsNormPathBuf,
    materialize_dirs_and_syms: bool,
    file_src: &mut F,
    executable_bit_override: Option<bool>,
) -> buck2_error::Result<()>
where
    F: FnMut(&AbsNormPath) -> Option<AbsNormPathBuf>,
    D: ActionDirectoryRef<'a>,
{
    match entry {
        DirectoryEntry::Dir(d) => {
            if materialize_dirs_and_syms {
                fs_util::create_dir_all(&dest)?;
            }
            for (name, entry) in d.entries() {
                dest.push(name);
                materialize_recursively(
                    entry,
                    dest,
                    materialize_dirs_and_syms,
                    file_src,
                    executable_bit_override,
                )?;
                dest.pop();
            }
            Ok(())
        }
        DirectoryEntry::Leaf(ActionDirectoryMember::File(_)) => {
            if let Some(src) = file_src(dest) {
                fs_util::copy(src, &dest)
                    .categorize_tagged(ErrorTag::MaterializeCopyMissingFile)?;
                if let Some(executable_bit_override) = executable_bit_override {
                    fs_util::set_executable(&dest, executable_bit_override)
                        .categorize_internal()?;
                }
            }
            Ok(())
        }
        DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(s)) => {
            if materialize_dirs_and_syms
                && fs_util::symlink_metadata(&dest)
                    .categorize_internal()
                    .is_err()
            {
                // A relative path normalizes `.` away, so a link to its own directory has an
                // empty target, which the filesystem refuses.
                let target = match s.target().as_str() {
                    "" => ".",
                    target => target,
                };
                fs_util::symlink(target, dest).categorize_internal()?;
            }
            Ok(())
        }
        DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(s)) => {
            if materialize_dirs_and_syms
                && fs_util::symlink_metadata(&dest)
                    .categorize_internal()
                    .is_err()
            {
                fs_util::symlink(s.target(), dest).categorize_internal()?;
            }
            Ok(())
        }
    }
}
