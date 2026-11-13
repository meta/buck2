/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::package::PackageLabel;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_core::package::package_relative_path::PackageRelativePathBuf;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_util::arc_str::PackedStringSliceBuilder;
use dice::DiceComputations;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::file_ops::dice::DiceFileComputations;
use crate::find_buildfile::find_buildfile;
use crate::ignores::file_ignores::FileIgnoreReason;
use crate::io::DirectoryDoesNotExistSuggestion;
use crate::io::ReadDirError;
use crate::package_listing::dice::enclosing_package_of_dir;
use crate::package_listing::listing::PackageListing;
use crate::package_listing::resolver::PackageListingResolver;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PackageListingError {
    #[error("Expected `{0}` to be within a package directory, but there was no buildfile in any parent directories. Expected one of `{}`", .1.join("`, `"))]
    NoContainingPackage(CellPath, Vec<FileNameBuf>),
}

#[async_trait]
impl PackageListingResolver for InterpreterPackageListingResolver<'_, '_> {
    async fn resolve(&mut self, package: PackageLabel) -> buck2_error::Result<PackageListing> {
        Ok(self.gather_package_listing(package.dupe()).await?)
    }

    async fn get_enclosing_package(&mut self, path: CellPath) -> buck2_error::Result<PackageLabel> {
        if let Some(dir) = path.parent() {
            if !DiceFileComputations::exists(self.ctx, dir).await? {
                // Only for the error, which says what is wrong and suggests
                // corrections: depending on the listing otherwise would rerun
                // this on any change to the directory.
                DiceFileComputations::read_dir(self.ctx, dir).await?;
            }
            if let Some(package) = enclosing_package_of_dir(self.ctx, dir).await? {
                return Ok(package);
            }
        }
        let buildfile_candidates = DiceFileComputations::buildfiles(self.ctx, path.cell()).await?;
        Err(PackageListingError::NoContainingPackage(path, buildfile_candidates.to_vec()).into())
    }

    async fn get_enclosing_packages(
        &mut self,
        path: CellPath,
        enclosing_path: CellPath,
    ) -> buck2_error::Result<Vec<PackageLabel>> {
        let Some(mut dir) = path.parent() else {
            let buildfile_candidates =
                DiceFileComputations::buildfiles(self.ctx, path.cell()).await?;
            return Err(PackageListingError::NoContainingPackage(
                path,
                buildfile_candidates.to_vec(),
            )
            .into());
        };
        // Hop from package to package rather than reading every directory, so
        // this only depends on where the buildfiles are.
        let mut packages = Vec::new();
        while let Some(package) = enclosing_package_of_dir(self.ctx, dir).await? {
            if !package.as_cell_path().starts_with(enclosing_path) {
                // stop when we are no longer within the enclosing path
                break;
            }
            packages.push(package);
            match package.as_cell_path().parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
        Ok(packages)
    }
}

pub struct InterpreterPackageListingResolver<'c, 'd> {
    ctx: &'c mut DiceComputations<'d>,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum GatherPackageListingError {
    #[buck2(input)]
    NoBuildFile {
        package: CellPath,
        candidates: Vec<FileNameBuf>,
    },
    #[buck2(input)]
    DirectoryDoesNotExist {
        package: CellPath,
        expected_path: CellPath,
        suggestion: DirectoryDoesNotExistSuggestion,
    },
    #[buck2(input)]
    DirectoryIsIgnored {
        package: CellPath,
        path: CellPath,
        ignore_reason: FileIgnoreReason,
    },
    #[buck2(input)]
    NotADirectory {
        package: CellPath,
        path: CellPath,
        node_type: String,
    },
    #[buck2(input)]
    Symlink { package: CellPath, at: CellPath },
    Error {
        package: CellPath,
        #[source]
        error: buck2_error::Error,
    },
}

impl GatherPackageListingError {
    fn error<E: Into<buck2_error::Error>>(
        package_path: CellPath,
        err: E,
    ) -> GatherPackageListingError {
        GatherPackageListingError::Error {
            package: package_path,
            error: err.into(),
        }
    }

    fn from_read_dir(package_path: CellPath, err: ReadDirError) -> GatherPackageListingError {
        match err {
            ReadDirError::DirectoryDoesNotExist { path, suggestion } => {
                GatherPackageListingError::DirectoryDoesNotExist {
                    package: package_path,
                    expected_path: path,
                    suggestion,
                }
            }
            ReadDirError::DirectoryIsIgnored(path, ignore_reason) => {
                GatherPackageListingError::DirectoryIsIgnored {
                    package: package_path,
                    path,
                    ignore_reason,
                }
            }
            ReadDirError::NotADirectory(path, node_type) => {
                GatherPackageListingError::NotADirectory {
                    package: package_path,
                    path,
                    node_type,
                }
            }
            ReadDirError::Symlink { at, .. } => GatherPackageListingError::Symlink {
                package: package_path,
                at,
            },
            ReadDirError::Error(e) => GatherPackageListingError::Error {
                package: package_path,
                error: e,
            },
        }
    }

    fn no_build_file(
        package_path: CellPath,
        candidates: Vec<FileNameBuf>,
    ) -> GatherPackageListingError {
        GatherPackageListingError::NoBuildFile {
            package: package_path,
            candidates,
        }
    }
}

impl std::fmt::Display for GatherPackageListingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /*
         package `fbsource//foo/target/x/y/lmnop:` does not exist
                  ^--------------------^
             dir `fbsource//foo/target/x` does not exist

         package `fbsource//foo/target/x/y/lmnop:` does not exist
                  ^--------------------^
             dir `fbsource//foo/target/x` is ignored (config project.ignore contains `foo/target/ **`)

         package `fbsource//fbcode/target/x/y/lmnop:` does not exist
                  ^--------------^
             this package is using the wrong cell, use `fbcode//target/x/y/lmnop:` instead


         package `fbsource//foo/target/x/y/lmnop:` does not exist
                  ^--------------------^
            path `fbsource//foo/target/x` is a file, not a directory

         package `fbsource//foo/target/x/y/lmnop:` does not exist
             missing `TARGETS` file (also missing alternatives `TARGETS.v2`, `BUCK`, `BUCK.v2`)

         error loading package `fbsource//foo/target/x/y/lmnop:`
              ... # just display the buck2_error for now
        */

        let prefix = "package `";
        let underlined = |path_as_string: &str| {
            format!(
                "{}^{}^",
                " ".repeat(prefix.len()),
                "-".repeat(path_as_string.len().saturating_sub(2))
            )
        };

        let (package, submessage) = match self {
            GatherPackageListingError::Error { package, .. } => {
                // in this case we return the buck2_error as our source and we're just displayed as context
                write!(f, "gathering package listing for `{}`", package)?;
                return Ok(());
            }
            GatherPackageListingError::NoBuildFile {
                candidates,
                package,
            } => {
                // Prefer a name without the `.v2` extension; a configuration may consist of
                // `.v2` names only.
                let primary_candidate = candidates
                    .iter()
                    .find(|v| v.extension() != Some("v2"))
                    .or_else(|| candidates.first());
                let message = match primary_candidate {
                    Some(primary_candidate) => {
                        let alternatives: Vec<_> = candidates
                            .iter()
                            .filter(|v| *v != primary_candidate)
                            .map(|v| format!("`{v}`"))
                            .collect();

                        if alternatives.is_empty() {
                            format!("    missing `{}` file", primary_candidate)
                        } else {
                            format!(
                                "    missing `{}` file (also missing alternatives {})",
                                primary_candidate,
                                alternatives.join(", ")
                            )
                        }
                    }
                    None => "    no build file names are configured".to_owned(),
                };

                (package, message)
            }
            GatherPackageListingError::DirectoryDoesNotExist {
                package,
                expected_path,
                suggestion,
            } => {
                let path_as_str = expected_path.to_string();
                let suggestion_msg = match suggestion {
                    DirectoryDoesNotExistSuggestion::Cell(cell_suggestion) => {
                        format!("Did you mean one of [`{}`]?", cell_suggestion.join("`, `"))
                    }
                    DirectoryDoesNotExistSuggestion::Typo(suggestion) => {
                        let suggested_target = match expected_path.parent() {
                            Some(parent) => {
                                if parent.path().is_empty() {
                                    format!("{}//{}", parent.cell(), suggestion)
                                } else {
                                    format!("{}/{}", parent, suggestion)
                                }
                            }
                            None => {
                                format!("{}//{}", expected_path.cell(), suggestion)
                            }
                        };

                        format!("Did you mean `{}`?", suggested_target)
                    }
                    DirectoryDoesNotExistSuggestion::NoSuggestion => "".to_owned(),
                };

                (
                    package,
                    format!(
                        "{}\n    dir `{}` does not exist. {}",
                        underlined(&path_as_str),
                        path_as_str,
                        suggestion_msg
                    ),
                )
            }
            GatherPackageListingError::NotADirectory {
                package,
                path,
                node_type,
            } => {
                let path_as_str = path.to_string();
                (
                    package,
                    format!(
                        "{}\n   path `{}` is a {}, not a directory",
                        underlined(&path_as_str),
                        path_as_str,
                        node_type
                    ),
                )
            }
            GatherPackageListingError::Symlink { package, at } => {
                let path_as_str = at.to_string();
                (
                    package,
                    format!(
                        "{}\n    path `{}` is a symlink, which buck2 does not follow",
                        underlined(&path_as_str),
                        path_as_str,
                    ),
                )
            }
            GatherPackageListingError::DirectoryIsIgnored {
                package,
                path,
                ignore_reason: FileIgnoreReason::IgnoredByPattern { pattern, .. },
            } => {
                let path_as_str = path.to_string();
                (
                    package,
                    format!(
                        "{}\n    dir `{}` does not exist (project.ignore contains `{}`)",
                        underlined(&path_as_str),
                        path_as_str,
                        pattern
                    ),
                )
            }
            GatherPackageListingError::DirectoryIsIgnored {
                package,
                path,
                ignore_reason: FileIgnoreReason::IgnoredByCell { cell_name, .. },
            } => {
                let path_as_str = path.to_string();
                let corrected = {
                    match package.strip_prefix(*path) {
                        Ok(fixed) => {
                            CellPath::new(*cell_name, CellRelativePath::new(fixed)).to_string()
                        }
                        _ => format!("{cell_name}//"),
                    }
                };
                (
                    package,
                    format!(
                        "{}\n    this package is using the wrong cell, use `{}` instead",
                        underlined(&path_as_str),
                        corrected,
                    ),
                )
            }
        };

        writeln!(f, "{prefix}{package}:` does not exist")?;
        f.write_str(&submessage)?;
        Ok(())
    }
}

impl<'c, 'd> InterpreterPackageListingResolver<'c, 'd> {
    pub fn new(ctx: &'c mut DiceComputations<'d>) -> Self {
        Self { ctx }
    }

    pub async fn gather_package_listing(
        &mut self,
        root: PackageLabel,
    ) -> Result<PackageListing, GatherPackageListingError> {
        gather_package_listing_impl(self.ctx, root).await
    }
}

struct Directory {
    path: PackageRelativePathBuf,
    files: Vec<PackageRelativePathBuf>,
    subdirs: Vec<Directory>,
    subpackages: Vec<PackageRelativePathBuf>,
    buildfile: Option<FileNameBuf>,

    recursive_files_count: usize,
    recursive_dirs_count: usize,
    recursive_subpackages_count: usize,
}

impl Directory {
    // Ok(None) indicates that the path is a subpackage
    async fn gather(
        ctx: &mut DiceComputations<'_>,
        buildfile_candidates: &[FileNameBuf],
        root: CellPath,
        path: &PackageRelativePath,
        is_root: bool,
    ) -> Result<Option<Directory>, GatherPackageListingError> {
        let cell_path = root.join(path.as_forward_rel_path());
        let entries = DiceFileComputations::read_dir_ext(ctx, cell_path)
            .await
            .map_err(|e| GatherPackageListingError::from_read_dir(cell_path, e))?
            .included;
        let buildfile = find_buildfile(buildfile_candidates, &entries);

        match (is_root, buildfile) {
            (true, None) => {
                return Err(GatherPackageListingError::no_build_file(
                    cell_path,
                    buildfile_candidates.to_vec(),
                ));
            }
            (false, Some(_)) => {
                return Ok(None);
            }
            _ => {}
        }

        let mut subdirs = Vec::new();
        let mut files = Vec::new();

        for (file_name, file_type) in entries.iter_with_tag() {
            let child_path = path.join(file_name);
            if file_type.is_dir() {
                subdirs.push(child_path);
            } else {
                files.push(child_path);
            }
        }

        let (subdirs, subpackages) =
            Self::gather_subdirs(ctx, buildfile_candidates, root, subdirs).await?;

        let mut recursive_files_count = files.len();
        let mut recursive_dirs_count = subdirs.len();
        let mut recursive_subpackages_count = subpackages.len();
        for d in &subdirs {
            recursive_files_count += d.recursive_files_count;
            recursive_dirs_count += d.recursive_dirs_count;
            recursive_subpackages_count += d.recursive_subpackages_count;
        }

        Ok(Some(Directory {
            path: path.to_owned(),
            files,
            subdirs,
            subpackages,
            buildfile: buildfile.map(|v| v.to_owned()),
            recursive_files_count,
            recursive_dirs_count,
            recursive_subpackages_count,
        }))
    }

    fn gather_subdirs<'a>(
        ctx: &'a mut DiceComputations<'_>,
        buildfile_candidates: &'a [FileNameBuf],
        root: CellPath,
        subdirs: Vec<PackageRelativePathBuf>,
    ) -> BoxFuture<
        'a,
        Result<(Vec<Directory>, Vec<PackageRelativePathBuf>), GatherPackageListingError>,
    > {
        async move {
            let mut new_subdirs = Vec::new();
            let mut subpackages = Vec::new();

            for res in ctx
                .compute_join(subdirs, async |ctx: &mut DiceComputations, path| {
                    let res =
                        Directory::gather(ctx, buildfile_candidates, root, &path, false).await?;
                    Ok((path, res))
                })
                .await
            {
                let (path, res) = res?;
                match res {
                    Some(v) => new_subdirs.push(v),
                    None => subpackages.push(path),
                }
            }
            Ok((new_subdirs, subpackages))
        }
        .boxed()
    }

    fn collect_into(
        self,
        files: &mut PackedStringSliceBuilder<PackageRelativePath>,
        dirs: &mut PackedStringSliceBuilder<PackageRelativePath>,
        pkgs: &mut PackedStringSliceBuilder<PackageRelativePath>,
    ) {
        for f in &self.files {
            files.push(f, ());
        }
        for s in &self.subpackages {
            pkgs.push(s, ());
        }
        if !self.path.is_empty() {
            dirs.push(&self.path, ());
        }
        for d in self.subdirs {
            d.collect_into(files, dirs, pkgs)
        }
    }

    fn flatten(mut self) -> PackageListing {
        let buildfile = self.buildfile.take().unwrap();
        let mut files: PackedStringSliceBuilder<PackageRelativePath> =
            PackedStringSliceBuilder::with_capacity(self.recursive_files_count);
        let mut dirs: PackedStringSliceBuilder<PackageRelativePath> =
            PackedStringSliceBuilder::with_capacity(self.recursive_dirs_count);
        let mut subpackages: PackedStringSliceBuilder<PackageRelativePath> =
            PackedStringSliceBuilder::with_capacity(self.recursive_subpackages_count);

        self.collect_into(&mut files, &mut dirs, &mut subpackages);

        // The files are discovered in a deterministic order but not necessarily sorted.
        PackageListing::new(
            files.sort_and_build(),
            dirs.sort_and_build(),
            subpackages.sort_and_build(),
            buildfile,
        )
    }
}

async fn gather_package_listing_impl(
    ctx: &mut DiceComputations<'_>,
    root: PackageLabel,
) -> Result<PackageListing, GatherPackageListingError> {
    let cell_path = root.as_cell_path();
    let buildfile_candidates = DiceFileComputations::buildfiles(ctx, root.cell_name())
        .await
        .map_err(|e| GatherPackageListingError::error(cell_path, e))?;
    Ok(Directory::gather(
        ctx,
        buildfile_candidates,
        cell_path,
        PackageRelativePath::empty(),
        true,
    )
    .await?
    .unwrap()
    .flatten())
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::cell_path::CellPath;
    use buck2_fs::paths::file_name::FileNameBuf;

    use crate::buildfiles::parse_buildfile_name;
    use crate::legacy_configs::configs::testing::parse;
    use crate::package_listing::interpreter::GatherPackageListingError;

    fn candidates_from_config(buildfile_section: &str) -> Vec<FileNameBuf> {
        let text = format!("[buildfile]\n{buildfile_section}\n");
        let config = parse(&[("config", text.as_str())], "config").unwrap();
        parse_buildfile_name(&config).unwrap()
    }

    fn no_build_file_message(candidates: Vec<FileNameBuf>) -> String {
        GatherPackageListingError::NoBuildFile {
            package: CellPath::testing_new("root//foo"),
            candidates,
        }
        .to_string()
    }

    #[test]
    fn test_no_build_file_message_default_candidates() {
        let msg = no_build_file_message(candidates_from_config("includes = x"));
        assert!(
            msg.contains("missing `BUCK` file (also missing alternatives `BUCK.v2`)"),
            "{msg}"
        );
    }

    /// A configuration made only of `.v2` names still names the missing file.
    #[test]
    fn test_no_build_file_message_only_v2_candidates() {
        for (section, expected) in [
            ("name_v2 = BUCK.v2", "missing `BUCK.v2` file"),
            (
                "name_v2 = TARGETS.v2,BUCK.v2",
                "missing `TARGETS.v2` file (also missing alternatives `BUCK.v2`)",
            ),
            (
                "name = BUCK.v2",
                "missing `BUCK.v2.v2` file (also missing alternatives `BUCK.v2`)",
            ),
        ] {
            let msg = no_build_file_message(candidates_from_config(section));
            assert!(msg.contains(expected), "config `{section}`: {msg}");
        }
    }
}
