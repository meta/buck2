/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_error::internal_error;
use dice::DiceComputations;
use dupe::Dupe;

use crate::dice::cells::HasCellResolver;
use crate::file_ops::dice::DiceFileComputations;
use crate::file_ops::dice::ReadDirRefusal;
use crate::file_ops::metadata::FileType;
use crate::file_ops::path_check::PathCheck;
use crate::file_ops::path_check::PathCheckError;
use crate::ignores::file_ignores::FileIgnoreReason;
use crate::ignores::ignore_set::HasIgnoreSet;
use crate::io::DirectoryDoesNotExistSuggestion;
use crate::io::ReadDirError;

#[derive(Debug, buck2_error::Error)]
pub(crate) enum FileOpsError {
    #[error("File not found: {0}")]
    // File not found errors are not inherently always user errors; however, we only use these
    // methods with source files, and in that case this is correct
    #[buck2(input)]
    #[buck2(tag = IoNotFound)]
    FileNotFound(String),
    #[error("{0}")]
    #[buck2(input)]
    NoFileToRead(String),
}

pub enum FileReadError {
    /// The cell's delegate found nothing at the path.
    NotFound(String),
    /// The check found no file to read at the path, and says what it found instead.
    Check(PathCheckError),
    Buck(buck2_error::Error),
}

impl FileReadError {
    pub fn with_package_context_information(self, package_path: String) -> buck2_error::Error {
        match self {
            FileReadError::NotFound(path) => FileOpsError::FileNotFound(format!(
                "`{path}`.\n     Included in `{package_path}` but does not exist"
            ))
            .into(),
            FileReadError::Check(err) => {
                FileOpsError::NoFileToRead(format!("{err}.\n     Included in `{package_path}`"))
                    .into()
            }
            FileReadError::Buck(err) => err.dupe(),
        }
    }

    pub fn without_package_context_information(self) -> buck2_error::Error {
        match self {
            FileReadError::NotFound(path) => FileOpsError::FileNotFound(path).into(),
            FileReadError::Check(err) => err.into(),
            FileReadError::Buck(err) => err.dupe(),
        }
    }
}

pub trait FileReadErrorContext<T> {
    fn with_package_context_information(self, package_path: String) -> buck2_error::Result<T>;
    fn without_package_context_information(self) -> buck2_error::Result<T>;
}

impl<T> FileReadErrorContext<T> for std::result::Result<T, FileReadError> {
    fn with_package_context_information(self, package_path: String) -> buck2_error::Result<T> {
        self.map_err(|e| e.with_package_context_information(package_path))
    }

    fn without_package_context_information(self) -> buck2_error::Result<T> {
        self.map_err(|e| e.without_package_context_information())
    }
}

fn did_you_mean<'a>(value: &str, variants: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    if value.is_empty() {
        return None;
    }

    const MAX_LEVENSHTEIN_DISTANCE: usize = 2;

    variants
        .into_iter()
        .map(|v| (v, strsim::levenshtein(value, v)))
        .filter(|(_, dist)| *dist <= MAX_LEVENSHTEIN_DISTANCE)
        .min_by_key(|(_v, sim)| *sim)
        .map(|(v, _)| v)
}

/// The error for a directory listing refused for `refusal`, with the suggestions the package
/// listing errors show. The suggestions cost reads, which is fine on the error path.
pub(super) async fn read_dir_error(
    ctx: &mut DiceComputations<'_>,
    path: CellPath,
    refusal: ReadDirRefusal,
) -> ReadDirError {
    let cells = match ctx.get_cell_resolver().await {
        Ok(cells) => cells,
        Err(e) => return ReadDirError::Error(e),
    };
    let check = match refusal {
        ReadDirRefusal::NestedCell { at, cell } => {
            return ReadDirError::DirectoryIsIgnored(
                at,
                FileIgnoreReason::IgnoredByCell {
                    path: at.path().as_str().to_owned(),
                    cell_name: cell,
                },
            );
        }
        ReadDirRefusal::Check { check, .. } => check,
    };
    match check {
        PathCheck::Present(FileType::Directory) => ReadDirError::Error(internal_error!(
            "`{path}` was refused although it is a directory"
        )),
        PathCheck::Present(FileType::Symlink) => ReadDirError::Symlink { path, at: path },
        PathCheck::Present(file_type) => {
            ReadDirError::NotADirectory(path, file_type.describe().to_owned())
        }
        PathCheck::Missing { at } => {
            let at_cell = cells.get_cell_path(&at);
            let suggestion = suggestion_for_missing(ctx, cells, at, at_cell).await;
            ReadDirError::DirectoryDoesNotExist {
                path: at_cell,
                suggestion,
            }
        }
        PathCheck::Ignored { at } => {
            let pattern = ctx
                .global_data()
                .get_ignore_set()
                .matches_candidate(&at)
                .unwrap_or("?")
                .to_owned();
            let at = cells.get_cell_path(&at);
            ReadDirError::DirectoryIsIgnored(
                at,
                FileIgnoreReason::IgnoredByPattern {
                    path: at.path().as_str().to_owned(),
                    pattern,
                },
            )
        }
        PathCheck::ParentSymlink { at } => ReadDirError::Symlink {
            path,
            at: cells.get_cell_path(&at),
        },
        PathCheck::ParentNotDirectory { at, file_type } => {
            ReadDirError::NotADirectory(cells.get_cell_path(&at), file_type.describe().to_owned())
        }
    }
}

/// What the user might have meant by the missing `at`: the same path in another cell, or a
/// sibling with a similar name.
async fn suggestion_for_missing(
    ctx: &mut DiceComputations<'_>,
    cells: &CellResolver,
    at: IProjectRelativePath,
    at_cell: CellPath,
) -> DirectoryDoesNotExistSuggestion {
    let mut in_other_cells = Vec::new();
    for (cell, _) in cells.cells() {
        if cell == at_cell.cell() {
            continue;
        }
        let candidate = CellPath::new(cell, at_cell.path());
        if DiceFileComputations::read_path_metadata_if_exists(ctx, candidate)
            .await
            .is_ok_and(|meta| meta.is_some())
        {
            in_other_cells.push(candidate.to_string());
        }
    }
    if !in_other_cells.is_empty() {
        return DirectoryDoesNotExistSuggestion::Cell(in_other_cells);
    }

    if let Some((parent, name)) = at.split_last() {
        if let Ok(siblings) =
            DiceFileComputations::read_dir(ctx, cells.get_cell_path(&parent)).await
        {
            if let Some(suggestion) =
                did_you_mean(name.as_str(), siblings.included.iter().map(|n| n.as_str()))
            {
                return DirectoryDoesNotExistSuggestion::Typo(suggestion.to_owned());
            }
        }
    }
    DirectoryDoesNotExistSuggestion::NoSuggestion
}
