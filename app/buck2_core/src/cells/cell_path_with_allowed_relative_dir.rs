/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::relative_path::Component;
use buck2_fs::paths::relative_path::RelativePath;
use pagable::Pagable;

use crate::cells::cell_path::CellPath;
use crate::cells::paths::CellRelativePath;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum RelativeImportParseError {
    #[error("Relative import path `{0}` is not allowed at the current location.")]
    InvalidRelativeImport(String),
    #[error("Relative import path `{0}` with non-leading parent dir component is not allowed.")]
    NonLeadingParentDirRelativeImport(String),
    #[error("Relative import path `{0}` with current dir component `.` is not allowed.")]
    CurrentDirRelativeImport(String),
    #[error("Invalid path `{0}` for file relative import path. Should be a forward relative path.")]
    InvalidCurrentPathWhenFileRelativeImport(String),
    #[error(
        "Relative import path `{0}` must not contain empty path components (a leading, trailing or doubled `/`)"
    )]
    EmptyComponentRelativeImport(String),
}

#[derive(
    Debug, Hash, Eq, PartialEq, PartialOrd, Ord, Allocative, Clone, Pagable
)]
pub struct CellPathWithAllowedRelativeDir {
    current_dir: CellPath,
    allowed_relative_dir: Option<CellPath>,
}

impl CellPathWithAllowedRelativeDir {
    pub fn new(current_dir: CellPath, allowed_relative_dir: Option<CellPath>) -> Self {
        if let Some(allowed_relative_dir_value) = allowed_relative_dir {
            assert!(
                current_dir.starts_with(allowed_relative_dir_value),
                "current_dir: `{current_dir}` must be a subpath of allowed_relative_dir: `{allowed_relative_dir_value}`",
            );
        }

        Self {
            current_dir,
            allowed_relative_dir,
        }
    }

    pub fn backwards_relative_not_supported(current_dir: CellPath) -> Self {
        Self {
            current_dir,
            allowed_relative_dir: None,
        }
    }

    pub fn join_normalized(&self, path: &RelativePath) -> buck2_error::Result<CellPath> {
        let Some(allowed_relative_dir) = &self.allowed_relative_dir else {
            let rel_path = <&ForwardRelativePath>::try_from(path).map_err(|_e| {
                RelativeImportParseError::InvalidCurrentPathWhenFileRelativeImport(path.to_string())
            })?;
            return Ok(self.current_dir.join(rel_path));
        };

        // `components()` skips empty components, so check them here: the branch above rejects
        // them, and a load path must not depend on the directory it appears in.
        let raw = path.as_str();
        if raw.starts_with('/') || raw.ends_with('/') || raw.contains("//") {
            return Err(
                RelativeImportParseError::EmptyComponentRelativeImport(raw.to_owned()).into(),
            );
        }

        let mut resolved_path = self
            .current_dir
            .path()
            .as_forward_relative_path()
            .to_owned();
        let mut num_allowed_parents = self
            .current_dir
            .path()
            .strip_prefix(allowed_relative_dir.path())
            .expect("Checked at construction time")
            .iter()
            .count();
        let mut components = path.components();
        while let Some(c) = components.next() {
            match c {
                Component::CurDir => {
                    return Err(RelativeImportParseError::CurrentDirRelativeImport(
                        path.to_string(),
                    )
                    .into());
                }
                Component::ParentDir => {
                    if num_allowed_parents == 0 {
                        return Err(RelativeImportParseError::InvalidRelativeImport(
                            path.to_string(),
                        )
                        .into());
                    }
                    num_allowed_parents -= 1;
                    resolved_path.pop();
                }
                Component::Normal(file_name) => {
                    resolved_path.push(FileName::new(file_name)?);
                    let remaining = components.as_relative_path();
                    let Ok(remaining) = <&ForwardRelativePath>::try_from(remaining) else {
                        return Err(RelativeImportParseError::NonLeadingParentDirRelativeImport(
                            path.to_string(),
                        )
                        .into());
                    };
                    resolved_path.extend(remaining);
                    break;
                }
            }
        }
        Ok(CellPath::new(
            self.current_dir.cell(),
            CellRelativePath::new(&resolved_path),
        ))
    }

    pub fn join<P: AsRef<ForwardRelativePath>>(&self, path: P) -> CellPath {
        self.current_dir.join(path)
    }

    pub fn current_dir(&self) -> &CellPath {
        &self.current_dir
    }

    pub fn has_allowed_relative_dir(&self) -> bool {
        self.allowed_relative_dir.is_some()
    }
}

#[cfg(test)]
mod tests {
    use buck2_fs::paths::file_name::FileName;
    use buck2_fs::paths::relative_path::RelativePath;

    use crate::cells::cell_path::CellPath;
    use crate::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
    use crate::cells::cell_path_with_allowed_relative_dir::RelativeImportParseError;
    use crate::cells::name::CellName;
    use crate::cells::paths::CellRelativePath;

    fn path(cell: &str, dir: &str, filename: &str) -> CellPath {
        let path = CellRelativePath::unchecked_new(dir).join(FileName::unchecked_new(filename));
        CellPath::new(CellName::testing_new(cell), &path)
    }

    #[test]
    fn parent_directory_traversal() -> buck2_error::Result<()> {
        let import = "../sibling.bzl";
        let cell_path_with_allowed_relative_dir = CellPathWithAllowedRelativeDir {
            current_dir: CellPath::testing_new("cell1//package/path"),
            allowed_relative_dir: Some(CellPath::testing_new("cell1//package")),
        };
        assert_eq!(
            path("cell1", "package", "sibling.bzl"),
            cell_path_with_allowed_relative_dir
                .join_normalized(RelativePath::unchecked_new(import))?
        );
        Ok(())
    }

    #[test]
    fn multiple_parent_directory_traversal() -> buck2_error::Result<()> {
        let import = "../../foo.bzl";
        let cell_path_with_allowed_relative_dir = CellPathWithAllowedRelativeDir {
            current_dir: CellPath::testing_new("cell1//root/a/b"),
            allowed_relative_dir: Some(CellPath::testing_new("cell1//root")),
        };
        assert_eq!(
            path("cell1", "root", "foo.bzl"),
            cell_path_with_allowed_relative_dir
                .join_normalized(RelativePath::unchecked_new(import))?
        );
        Ok(())
    }

    #[test]
    fn parent_directory_with_subpath() -> buck2_error::Result<()> {
        let import = "../bar/zoo.bzl";
        let cell_path_with_allowed_relative_dir = CellPathWithAllowedRelativeDir {
            current_dir: CellPath::testing_new("cell1//package/foo/baz"),
            allowed_relative_dir: Some(CellPath::testing_new("cell1//package")),
        };
        assert_eq!(
            path("cell1", "package/foo/bar", "zoo.bzl"),
            cell_path_with_allowed_relative_dir
                .join_normalized(RelativePath::unchecked_new(import))?
        );
        Ok(())
    }

    #[test]
    fn traversal_out_of_allowed_dir() -> buck2_error::Result<()> {
        let import = "../../package1/too_far_up.bzl";
        let cell_path_with_allowed_relative_dir = CellPathWithAllowedRelativeDir {
            current_dir: CellPath::testing_new("cell1//package1/path"),
            allowed_relative_dir: Some(CellPath::testing_new("cell1//package1")),
        };
        assert_eq!(
            format!(
                "{:#}",
                cell_path_with_allowed_relative_dir
                    .join_normalized(RelativePath::unchecked_new(import))
                    .unwrap_err()
            ),
            RelativeImportParseError::InvalidRelativeImport(import.to_owned()).to_string()
        );
        Ok(())
    }

    #[test]
    fn nonleading_parent_dir_component() -> buck2_error::Result<()> {
        let import = "some_dir/../../package2/wrong_package.bzl";
        let cell_path_with_allowed_relative_dir = CellPathWithAllowedRelativeDir {
            current_dir: CellPath::testing_new("cell1//package1/path"),
            allowed_relative_dir: Some(CellPath::testing_new("cell1//package1")),
        };
        assert_eq!(
            format!(
                "{:#}",
                cell_path_with_allowed_relative_dir
                    .join_normalized(RelativePath::unchecked_new(import))
                    .unwrap_err()
            ),
            RelativeImportParseError::NonLeadingParentDirRelativeImport(import.to_owned())
                .to_string()
        );
        Ok(())
    }
}
