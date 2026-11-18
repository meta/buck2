/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::cells::nested::NestedCells;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_util::arc_str::PackedStringSlice;
use buck2_util::arc_str::PackedStringSliceBuilder;
use dice::DiceComputations;
use dupe::Dupe;

use crate::buildfiles::HasBuildfiles;
use crate::dice::cells::HasCellResolver;
use crate::file_ops::delegate::external_cell_delegate;
use crate::file_ops::error::FileReadError;
use crate::file_ops::error::read_dir_error;
use crate::file_ops::io;
use crate::file_ops::io::DirListing;
use crate::file_ops::metadata::FileType;
use crate::file_ops::metadata::RawPathMetadata;
use crate::file_ops::metadata::ReadDirOutput;
use crate::file_ops::path_check::PathCheck;
use crate::file_ops::path_check::PathCheckError;
use crate::file_ops::path_check::ReadKind;
use crate::file_ops::path_check::check_path;
use crate::ignores::file_ignores::CellFileIgnores;
use crate::ignores::file_ignores::FileIgnoreReason;
use crate::ignores::file_ignores::FileIgnoreResult;
use crate::ignores::ignore_set::HasIgnoreSet;
use crate::io::ReadDirError;

pub struct DiceFileComputations;

/// Functions for accessing files with keys on the dice graph.
impl DiceFileComputations {
    /// Lists a directory of the cell. Entries that are the roots of nested cells are left out,
    /// and a path inside a nested cell is an error; `project.ignore` applies as it does to every
    /// read.
    pub async fn read_dir(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> buck2_error::Result<ReadDirOutput> {
        match read_dir_impl(ctx, path).await? {
            Ok(listing) => Ok(listing),
            Err(refusal) => Err(refusal.into_error(ctx, path)),
        }
    }

    /// Lists a directory like `read_dir`, except that a path that is a symlink or behind one is
    /// `None` rather than an error.
    ///
    /// Nothing is found through a symlink, so a caller that looks for something along a path,
    /// such as the package a file belongs to, finds nothing there and looks on above it.
    pub async fn read_dir_unless_behind_symlink(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> buck2_error::Result<Option<ReadDirOutput>> {
        let cells = ctx.get_cell_resolver().await?;
        if external_cell_delegate(ctx, cells, path.cell())
            .await?
            .is_none()
        {
            // Asking the check rather than matching on the listing's refusal: where reads through
            // symlinks are tolerated, the listing goes through.
            if let PathCheck::Present(FileType::Symlink) | PathCheck::ParentSymlink { .. } =
                check_path(ctx, cells.resolve_ipath(path)?).await?
            {
                return Ok(None);
            }
        }
        Self::read_dir(ctx, path).await.map(Some)
    }

    /// Whether a file, directory or symlink exists at the path, spelled exactly like that in
    /// every component, with no symlink above it, nothing ignored on the way, and inside this
    /// cell rather than a cell nested in it.
    pub async fn exists(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> buck2_error::Result<bool> {
        let cells = ctx.get_cell_resolver().await?;
        if cells
            .get(path.cell())?
            .nested_cells()
            .matches(path.path())
            .is_some()
        {
            return Ok(false);
        }
        match external_cell_delegate(ctx, cells, path.cell()).await? {
            Some(delegate) => delegate.exists(ctx, path.ipath()).await,
            None => io::exists(ctx, cells.resolve_ipath(path)?).await,
        }
    }

    /// `read_dir` with an error that says what is wrong and suggests corrections. The
    /// suggestions cost extra reads, which add dependencies to the caller.
    pub async fn read_dir_ext(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> Result<ReadDirOutput, ReadDirError> {
        match read_dir_impl(ctx, path).await? {
            Ok(listing) => Ok(listing),
            Err(refusal) => Err(read_dir_error(ctx, path, refusal).await),
        }
    }

    /// The contents of the file, or `None` if there is no file there: nothing at all, or
    /// something ignored. A symlink at the path or above it is an error, as is a directory.
    pub async fn read_file_if_exists(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> buck2_error::Result<Option<String>> {
        match read_file_impl(ctx, path).await? {
            FileRead::Contents(contents) => Ok(Some(contents)),
            FileRead::Absent => Ok(None),
            FileRead::Refused {
                check:
                    PathCheck::Missing { .. }
                    | PathCheck::Ignored { .. }
                    | PathCheck::ParentNotDirectory { .. },
                ..
            } => Ok(None),
            FileRead::Refused {
                project_path,
                check,
            } => Err(PathCheckError::new(ctx, project_path, check, ReadKind::File).into()),
        }
    }

    /// Like `read_file_if_exists`, with no file to read as an error that says why.
    pub async fn read_file(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> Result<String, FileReadError> {
        match read_file_impl(ctx, path).await {
            Ok(FileRead::Contents(contents)) => Ok(contents),
            Ok(FileRead::Absent) => Err(FileReadError::NotFound(path.to_string())),
            Ok(FileRead::Refused {
                project_path,
                check,
            }) => Err(FileReadError::Check(PathCheckError::new(
                ctx,
                project_path,
                check,
                ReadKind::File,
            ))),
            Err(e) => Err(FileReadError::Buck(e)),
        }
    }

    /// TODO(jakobdegen): once the repo is clean, undo D128645602, which reads ignored paths anyway.
    pub async fn read_path_metadata_if_exists(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> buck2_error::Result<Option<RawPathMetadata>> {
        let cells = ctx.get_cell_resolver().await?;
        match external_cell_delegate(ctx, cells, path.cell()).await? {
            Some(delegate) => {
                delegate
                    .read_path_metadata_if_exists(ctx, path.ipath())
                    .await
            }
            None => Ok(
                io::read_path_metadata_if_exists(ctx, cells.resolve_ipath(path)?)
                    .await?
                    .map(|meta| meta.map(|p| cells.get_cell_path(&p))),
            ),
        }
    }

    /// TODO(jakobdegen): once the repo is clean, undo D128645602, which reads ignored paths anyway.
    pub async fn read_path_metadata(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> Result<RawPathMetadata, FileReadError> {
        match Self::read_path_metadata_if_exists(ctx, path).await {
            Ok(result) => result.ok_or_else(|| FileReadError::NotFound(path.to_string())),
            Err(e) => Err(FileReadError::Buck(e)),
        }
    }

    pub async fn is_ignored(
        ctx: &mut DiceComputations<'_>,
        path: CellPath,
    ) -> buck2_error::Result<FileIgnoreResult> {
        let cells = ctx.get_cell_resolver().await?;
        Ok(cell_ignores(ctx, cells, path.cell())?.check(path.path()))
    }

    pub async fn buildfiles<'d>(
        ctx: &mut DiceComputations<'d>,
        cell: CellName,
    ) -> buck2_error::Result<&'d Arc<[FileNameBuf]>> {
        ctx.get_buildfiles(cell).await
    }
}

fn cell_ignores(
    ctx: &DiceComputations<'_>,
    cells: &CellResolver,
    cell: CellName,
) -> buck2_error::Result<CellFileIgnores> {
    cells.get(cell)?;
    Ok(CellFileIgnores::new(
        cells.dupe(),
        cell,
        ctx.global_data().get_ignore_set(),
    ))
}

/// `entries`, listed at `path`, minus the roots of cells nested in `path`'s cell.
fn without_nested_cells(
    path: CellPath,
    entries: PackedStringSlice<FileName, FileType>,
    nested_cells: &NestedCells,
) -> PackedStringSlice<FileName, FileType> {
    if nested_cells.is_empty() {
        return entries;
    }
    let path = path.path();
    let mut buf = String::with_capacity(path.as_str().len() + 1 + 32);
    let mut is_nested_cell = |file_name: &FileName| {
        buf.clear();
        if !path.is_empty() {
            buf.push_str(path.as_str());
            buf.push('/');
        }
        buf.push_str(file_name.as_str());
        nested_cells
            .matches(CellRelativePath::unchecked_new(&buf))
            .is_some()
    };

    // Input is sorted; we only remove entries, never reorder, so the output stays sorted. Defer
    // allocating a builder until we encounter the first removed entry, so that the common case
    // can reuse the existing alloc.
    let mut builder: Option<PackedStringSliceBuilder<FileName, FileType>> = None;
    for (idx, (file_name, file_type)) in entries.iter_with_tag().enumerate() {
        if is_nested_cell(file_name) {
            builder.get_or_insert_with(|| {
                let mut b = PackedStringSliceBuilder::with_capacity(entries.len() - 1);
                for (fname, ftype) in entries.iter_with_tag().take(idx) {
                    b.push(fname, ftype);
                }
                b
            });
            continue;
        }
        if let Some(b) = builder.as_mut() {
            b.push(file_name, file_type);
        }
    }
    match builder {
        Some(b) => b.build_sorted(),
        None => entries,
    }
}

/// Why a directory could not be listed as asked.
pub(super) enum ReadDirRefusal {
    /// What the check found at the directory's project path instead of a present directory.
    Check {
        project_path: IProjectRelativePath,
        check: PathCheck,
    },
    /// The directory is inside a cell nested in the one it was addressed through; `at` is that
    /// cell's root.
    NestedCell { at: CellPath, cell: CellName },
}

impl ReadDirRefusal {
    fn into_error(self, ctx: &DiceComputations<'_>, path: CellPath) -> buck2_error::Error {
        match self {
            ReadDirRefusal::Check {
                project_path,
                check,
            } => PathCheckError::new(ctx, project_path, check, ReadKind::Directory).into(),
            ReadDirRefusal::NestedCell { cell, .. } => FileIgnoreReason::IgnoredByCell {
                path: path.path().as_str().to_owned(),
                cell_name: cell,
            }
            .into_read_error(),
        }
    }
}

async fn read_dir_impl(
    ctx: &mut DiceComputations<'_>,
    path: CellPath,
) -> buck2_error::Result<Result<ReadDirOutput, ReadDirRefusal>> {
    let cells = ctx.get_cell_resolver().await?;
    let nested_cells = cells.get(path.cell())?.nested_cells();
    if let Some((root, cell, _)) = nested_cells.matches(path.path()) {
        return Ok(Err(ReadDirRefusal::NestedCell {
            at: CellPath::new(path.cell(), root),
            cell,
        }));
    }

    let entries = match external_cell_delegate(ctx, cells, path.cell()).await? {
        Some(delegate) => delegate.read_dir(ctx, path.ipath()).await?,
        None => {
            let project_path = cells.resolve_ipath(path)?;
            match io::read_dir(ctx, project_path).await? {
                DirListing::Entries(entries) => entries,
                DirListing::Refused(check) => {
                    return Ok(Err(ReadDirRefusal::Check {
                        project_path,
                        check,
                    }));
                }
            }
        }
    };
    Ok(Ok(ReadDirOutput {
        included: without_nested_cells(path, entries, nested_cells),
    }))
}

/// What looking for a file found.
enum FileRead {
    Contents(String),
    /// Nothing there, according to the cell's delegate.
    Absent,
    /// What the check found at the file's project path instead of a present file.
    Refused {
        project_path: IProjectRelativePath,
        check: PathCheck,
    },
}

async fn read_file_impl(
    ctx: &mut DiceComputations<'_>,
    path: CellPath,
) -> buck2_error::Result<FileRead> {
    let cells = ctx.get_cell_resolver().await?;
    match external_cell_delegate(ctx, cells, path.cell()).await? {
        Some(delegate) => Ok(
            match delegate.read_file_if_exists(ctx, path.ipath()).await? {
                Some(contents) => FileRead::Contents(contents),
                None => FileRead::Absent,
            },
        ),
        None => {
            let project_path = cells.resolve_ipath(path)?;
            Ok(match io::read_file(ctx, project_path).await? {
                Ok(contents) => FileRead::Contents(contents),
                Err(check) => FileRead::Refused {
                    project_path,
                    check,
                },
            })
        }
    }
}
