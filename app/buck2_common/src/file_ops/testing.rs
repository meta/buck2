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
use std::collections::BTreeSet;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_util::arc_str::PackedStringSliceBuilder;
use dice::testing::DiceBuilder;
use dupe::Dupe;
use pagable::Pagable;

use crate::cas_digest::CasDigestConfig;
use crate::dice::data::SetIoProvider;
use crate::external_symlink::ExternalSymlink;
use crate::file_ops::metadata::FileMetadata;
use crate::file_ops::metadata::FileType;
use crate::file_ops::metadata::RawDirEntry;
use crate::file_ops::metadata::RawPathMetadata;
use crate::file_ops::metadata::RawSymlink;
use crate::file_ops::metadata::ReadDirOutput;
use crate::file_ops::metadata::TrackedFileDigest;
use crate::file_ops::trait_::FileOps;
use crate::ignores::file_ignores::FileIgnoreResult;
use crate::ignores::ignore_set::IgnoreSet;
use crate::ignores::ignore_set::SetIgnoreSet;
use crate::io::IoProvider;
use crate::io::ReadDirOutcome;

#[derive(Pagable)]
enum TestFileOpsEntry {
    File(String /*data*/, FileMetadata),
    ExternalSymlink(Arc<ExternalSymlink>),
    Directory(BTreeSet<(FileNameBuf, FileType)>),
}

#[derive(Allocative, Clone, Dupe, Pagable)]
pub struct TestFileOps {
    #[allocative(skip)]
    entries: Arc<BTreeMap<CellPath, TestFileOpsEntry>>,
}

impl TestFileOps {
    fn new(inputs: BTreeMap<CellPath, TestFileOpsEntry>) -> Self {
        let mut entries = BTreeMap::new();
        for (path, entry) in inputs {
            let mut file_type = match entry {
                TestFileOpsEntry::Directory(..) => FileType::Directory,
                TestFileOpsEntry::ExternalSymlink(..) => FileType::Symlink,
                TestFileOpsEntry::File(..) => FileType::File,
            };
            // make sure the test setup is correct and concise
            assert!(
                entries.insert(path, entry).is_none(),
                "Adding `{path}`, it already exists."
            );

            let mut path = path;

            // now add to / create the parent directories
            while let (Some(dir), Some(name)) = (path.parent(), path.path().file_name()) {
                let dir_entry = entries
                    .entry(dir)
                    .or_insert_with(|| TestFileOpsEntry::Directory(BTreeSet::new()));
                match dir_entry {
                    TestFileOpsEntry::Directory(listing) => {
                        listing.insert((name.to_owned(), file_type));
                        file_type = FileType::Directory;
                        path = dir;
                    }
                    _ => panic!("Adding `{path}`, but `{dir}` exists and is not a dir"),
                };
            }
        }
        TestFileOps {
            entries: Arc::new(entries),
        }
    }

    pub fn new_with_files(files: BTreeMap<CellPath, String>) -> Self {
        let cas_digest_config = CasDigestConfig::testing_default();

        Self::new(
            files
                .into_iter()
                .map(|(path, data)| {
                    (
                        path,
                        TestFileOpsEntry::File(
                            data.clone(),
                            FileMetadata {
                                digest: TrackedFileDigest::from_content(
                                    data.as_bytes(),
                                    cas_digest_config,
                                ),
                                is_executable: false,
                            },
                        ),
                    )
                })
                .collect::<BTreeMap<CellPath, TestFileOpsEntry>>(),
        )
    }

    pub fn new_with_files_metadata(files: BTreeMap<CellPath, FileMetadata>) -> Self {
        Self::new(
            files
                .into_iter()
                .map(|(path, m)| (path, TestFileOpsEntry::File("".to_owned(), m)))
                .collect::<BTreeMap<CellPath, TestFileOpsEntry>>(),
        )
    }

    pub fn new_with_symlinks(symlinks: BTreeMap<CellPath, Arc<ExternalSymlink>>) -> Self {
        Self::new(
            symlinks
                .into_iter()
                .map(|(path, s)| (path, TestFileOpsEntry::ExternalSymlink(s)))
                .collect::<BTreeMap<CellPath, TestFileOpsEntry>>(),
        )
    }

    /// Serves these files as the project's file system, with `cells` saying where in the project
    /// each cell is. Reads then go through the same DICE keys as in production; the transaction
    /// still needs a file system baseline, and a cell resolver if anything else wants one.
    pub fn mock_in_dice(&self, cells: &CellResolver, builder: DiceBuilder) -> DiceBuilder {
        let io = TestIoProvider {
            ops: self.dupe(),
            cells: cells.dupe(),
            root: ProjectRootTemp::new().unwrap(),
        };
        let ignores =
            IgnoreSet::testing_new_single_cell(cells.dupe(), cells.root_cell(), "").unwrap();
        builder.set_data(|data| {
            data.set_io_provider(Arc::new(io));
            data.set_ignore_set(Arc::new(ignores));
        })
    }
}

#[async_trait]
impl FileOps for TestFileOps {
    async fn read_file_if_exists(&self, path: CellPath) -> buck2_error::Result<Option<String>> {
        Ok(self.entries.get(&path).and_then(|e| match e {
            TestFileOpsEntry::File(data, ..) => Some(data.clone()),
            _ => None,
        }))
    }

    async fn read_dir(&self, path: CellPath) -> buck2_error::Result<ReadDirOutput> {
        let listing = self
            .entries
            .get(&path)
            .and_then(|e| match e {
                TestFileOpsEntry::Directory(listing) => Some(listing),
                _ => None,
            })
            .ok_or_else(|| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "couldn't find dir {:?}",
                    path
                )
            })?;
        let mut builder: PackedStringSliceBuilder<FileName, FileType> =
            PackedStringSliceBuilder::with_capacity(listing.len());
        // BTreeSet iterates in sorted order — by FileNameBuf, then FileType.
        // The sort key for the packed slice is the file name only.
        for (file_name, file_type) in listing {
            builder.push(file_name, *file_type);
        }
        Ok(ReadDirOutput {
            included: builder.build_sorted(),
        })
    }

    async fn read_path_metadata_if_exists(
        &self,
        path: CellPath,
    ) -> buck2_error::Result<Option<RawPathMetadata>> {
        self.entries.get(&path).map_or(Ok(None), |e| {
            match e {
                TestFileOpsEntry::File(_data, metadata) => {
                    Ok(RawPathMetadata::File(metadata.to_owned()))
                }
                TestFileOpsEntry::ExternalSymlink(sym) => Ok(RawPathMetadata::Symlink {
                    at: path,
                    to: RawSymlink::External(sym.dupe()),
                }),
                TestFileOpsEntry::Directory(..) => Ok(RawPathMetadata::Directory),
            }
            .map(Some)
        })
    }

    async fn is_ignored(&self, _path: CellPath) -> buck2_error::Result<FileIgnoreResult> {
        Ok(FileIgnoreResult::Ok)
    }

    async fn buildfiles<'a>(&self, _cell: CellName) -> buck2_error::Result<Arc<[FileNameBuf]>> {
        Ok(Arc::from_iter([FileNameBuf::unchecked_new("BUCK")]))
    }
}

/// `TestFileOps` as an `IoProvider`. The project root is an empty temporary directory that
/// nothing reads; it only exists to be pointed at.
#[derive(Allocative)]
struct TestIoProvider {
    ops: TestFileOps,
    cells: CellResolver,
    #[allocative(skip)]
    root: ProjectRootTemp,
}

impl TestIoProvider {
    fn cell_path(&self, path: IProjectRelativePath) -> CellPath {
        self.cells.get_cell_path(path.as_project_relative_path())
    }
}

#[async_trait]
impl IoProvider for TestIoProvider {
    async fn read_file_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<String>> {
        FileOps::read_file_if_exists(&self.ops, self.cell_path(path)).await
    }

    /// The files are given per cell; the directories leading from the project root to the cell
    /// roots exist by construction.
    async fn read_dir_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<ReadDirOutcome> {
        let cell_roots = || {
            self.cells
                .cells()
                .map(|(_, instance)| instance.path().as_project_relative_path())
        };
        let mut entries = match FileOps::read_dir(&self.ops, self.cell_path(path)).await {
            Ok(listing) => listing
                .included
                .iter_with_tag()
                .map(|(file_name, file_type)| RawDirEntry {
                    file_name: file_name.to_owned(),
                    file_type,
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                if cell_roots().any(|root| root.starts_with(path.as_project_relative_path())) {
                    Vec::new()
                } else {
                    return Err(e);
                }
            }
        };
        for root in cell_roots() {
            if let Some(first) = root
                .strip_prefix_opt(path.as_project_relative_path())
                .and_then(|rest| rest.iter().next())
            {
                entries.push(RawDirEntry {
                    file_name: first.to_owned(),
                    file_type: FileType::Directory,
                });
            }
        }
        entries.sort();
        entries.dedup();
        Ok(ReadDirOutcome::Entries(entries))
    }

    async fn read_path_metadata_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<RawPathMetadata<ProjectRelativePathBuf>>> {
        let Some(meta) =
            FileOps::read_path_metadata_if_exists(&self.ops, self.cell_path(path)).await?
        else {
            return Ok(None);
        };
        Ok(Some(meta.try_map(|p| self.cells.resolve_path(p))?))
    }

    async fn settle(&self) -> buck2_error::Result<()> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "test"
    }

    async fn eden_version(&self) -> buck2_error::Result<Option<String>> {
        Ok(None)
    }

    fn project_root(&self) -> &ProjectRoot {
        self.root.path()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_eden_repo(&self) -> bool {
        false
    }
}
