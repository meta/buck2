/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// Eden's Thrift API does sometime want &Vec<...>.
#![allow(clippy::useless_vec)]

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::FileType;
use buck2_common::file_ops::metadata::RawDirEntry;
use buck2_common::file_ops::metadata::RawPathMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::io::IoProvider;
use buck2_common::io::ReadDirOutcome;
use buck2_common::io::fs::FsIoProvider;
use buck2_common::io::fs::ReadUncheckedOptions;
use buck2_common::io::report_invalid_file_names;
use buck2_core;
use buck2_core::buck2_env;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::io_counters::IoCounterKey;
use buck2_core::soft_error;
use buck2_core::warn_user;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_fs::paths::file_name::FileNameBuf;
use compact_str::CompactString;
use dupe::Dupe;
use edenfs::FileAttributes;
use edenfs::GetAttributesFromFilesParams;
use edenfs::GetFileContentRequest;
use edenfs::MountId;
use edenfs::ReaddirParams;
use edenfs::ScmBlobOrError;
use edenfs::SourceControlType;
use edenfs::SyncBehavior;
use edenfs::SynchronizeWorkingCopyParams;
use fbinit::FacebookInit;

use crate::connection::EdenConnectionManager;
use crate::error::EdenDataIntoResult;
use crate::error::EdenError;
use crate::semaphore::buck2_default;

#[derive(Allocative)]
pub struct EdenIoProvider {
    manager: EdenConnectionManager,
    fs: FsIoProvider,
    digest: Digest,
    use_eden_thrift_read: bool,
}

#[derive(Allocative, Copy, Clone, Dupe)]
enum Digest {
    Sha1,
    Blake3Keyed,
}

enum PathMetadataResult {
    Result(Option<RawPathMetadata<ProjectRelativePathBuf>>),
    Error(EdenError),
}

impl EdenIoProvider {
    pub async fn new(
        fb: FacebookInit,
        fs: &ProjectRoot,
        cas_digest_config: CasDigestConfig,
        use_eden_thrift_read: bool,
    ) -> buck2_error::Result<Option<Self>> {
        let (digest, min_eden_version) = if cas_digest_config.source_files_config().allows_sha1() {
            (Digest::Sha1, "20220905-214046")
        } else if cas_digest_config
            .source_files_config()
            .allows_blake3_keyed()
        {
            (Digest::Blake3Keyed, "20230612-191714")
        } else {
            tracing::debug!("Disabling Eden I/O: Digest config does not allow supported digests");
            return Ok(None);
        };

        let eden_semaphore = buck2_default()?;

        let manager = match EdenConnectionManager::new(fb, fs, Some(eden_semaphore))? {
            Some(manager) => manager,
            None => return Ok(None),
        };

        let eden_version = manager
            .get_eden_version()
            .await
            .buck_error_context("Error querying Eden version")?;

        if let Some(eden_version) = &eden_version {
            if eden_version.as_str() < min_eden_version {
                warn_user!(
                    "eden_io_version_too_old",
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Environment,
                        "Disabling Eden I/O: \
                    your Eden version ({}) is too old to support Eden I/O (minimum required: {}). \
                    Update Eden then restart Buck2 to use Eden I/O.",
                        eden_version,
                        min_eden_version
                    )
                );
                return Ok(None);
            }
        }

        manager.capture_identity().await;

        Ok(Some(Self {
            manager,
            fs: FsIoProvider::new(fs.dupe(), cas_digest_config, true),
            digest,
            use_eden_thrift_read,
        }))
    }

    async fn read_path_metadata_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<PathMetadataResult> {
        let _guard = IoCounterKey::StatEden.guard();

        let hash_attribute = match self.digest {
            Digest::Sha1 => FileAttributes::SHA1_HASH,
            // The only digests we get out of Eden are Blake3-keyed.
            Digest::Blake3Keyed => FileAttributes::BLAKE3_HASH,
        };

        let requested_attributes = i64::from(
            i32::from(hash_attribute)
                | i32::from(FileAttributes::FILE_SIZE)
                | i32::from(FileAttributes::SOURCE_CONTROL_TYPE),
        );

        let params = GetAttributesFromFilesParams {
            mountPoint: self.manager.get_mount_point(),
            paths: self
                .manager
                .project_path_list_as_eden_path_list([path.as_ref()]),
            requestedAttributes: requested_attributes,
            sync: no_sync(),
            ..Default::default()
        };

        let attrs = self
            .manager
            .with_eden(|eden| {
                tracing::trace!("getAttributesFromFilesV2({})", path);
                eden.getAttributesFromFilesV2(&params)
            })
            .await?;

        match attrs
            .res
            .into_iter()
            .next()
            .internal_error("Eden did not return file info")?
            .into_result()
        {
            Ok(data) => {
                let source_control_type = data
                    .sourceControlType
                    .internal_error("Eden did not return a type")?
                    .into_result()
                    .buck_error_context("Eden returned an error for sourceControlType")?;

                if source_control_type == SourceControlType::TREE {
                    return Ok(PathMetadataResult::Result(Some(RawPathMetadata::Directory)));
                };

                if source_control_type == SourceControlType::SYMLINK {
                    let options = if cfg!(windows) {
                        // On Windows, Eden will tell us something's a symlink but maybe expose it
                        // as a file on disk.
                        ReadUncheckedOptions::Anything
                    } else {
                        ReadUncheckedOptions::Symlink
                    };

                    let meta = self
                        .fs
                        .read_unchecked(path.to_buf(), options)
                        .await
                        .with_buck_error_context(|| {
                            format!(
                                "Eden returned that `{path}` was a symlink, but it was not.  \
                                This path may have changed during the build"
                            )
                        })?;

                    return Ok(PathMetadataResult::Result(Some(meta)));
                };

                let size = data
                    .size
                    .internal_error("Eden did not return a size")?
                    .into_result()
                    .buck_error_context("Eden returned an error for size")?
                    .try_into()
                    .buck_error_context("Eden returned an invalid size")?;

                tracing::debug!("getAttributesFromFilesV2({}): ok", path);
                let digest = match self.digest {
                    Digest::Sha1 => {
                        let sha1 = data
                            .sha1
                            .internal_error("Eden did not return a sha1")?
                            .into_result()
                            .buck_error_context("Eden returned an error for sha1")?
                            .try_into()
                            .ok()
                            .internal_error("Eden returned an invalid sha1")?;
                        FileDigest::new_sha1(sha1, size)
                    }
                    Digest::Blake3Keyed => {
                        let blake3 = data
                            .blake3
                            .internal_error("Eden did not return a blake3")?
                            .into_result()
                            .buck_error_context("Eden returned an error for blake3")?
                            .try_into()
                            .ok()
                            .internal_error("Eden returned an invalid blake3")?;
                        FileDigest::new_blake3_keyed(blake3, size)
                    }
                };

                let digest = TrackedFileDigest::new(
                    digest,
                    self.fs.cas_digest_config().source_files_config(),
                );

                let is_executable = source_control_type == SourceControlType::EXECUTABLE_FILE;

                let meta = FileMetadata {
                    digest,
                    is_executable,
                };

                Ok(PathMetadataResult::Result(Some(RawPathMetadata::File(
                    meta,
                ))))
            }
            Err(err) => Ok(PathMetadataResult::Error(err)),
        }
    }

    async fn read_dir_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<ReadDirOutcome> {
        let _guard = IoCounterKey::ReadDirEden.guard();

        let requested_attributes = i64::from(i32::from(FileAttributes::SOURCE_CONTROL_TYPE));

        let params = ReaddirParams {
            mountPoint: self.manager.get_mount_point(),
            directoryPaths: self
                .manager
                .project_path_list_as_eden_path_list([path.as_ref()]),
            requestedAttributes: requested_attributes,
            sync: no_sync(),
            ..Default::default()
        };

        let res = self
            .manager
            .with_eden(|eden| {
                tracing::trace!("readdir({})", path);
                eden.readdir(&params)
            })
            .await?
            .dirLists
            .into_iter()
            .next()
            .internal_error("Eden did not return a directory result")?;

        let data = match res {
            edenfs::DirListAttributeDataOrError::dirListAttributeData(data) => data,
            edenfs::DirListAttributeDataOrError::error(err) => match err.errorCode {
                Some(libc::ENOENT) => return Err(EdenError::from(err).into()),
                Some(libc::EACCES) => {
                    tracing::debug!(
                        "readdir({}): permission denied, treating as restricted directory",
                        path
                    );
                    return Ok(ReadDirOutcome::EdenPermissionDenied);
                }
                Some(libc::EINVAL) | Some(libc::ENOTDIR) => {
                    // A symlink at or above the path, which plain file IO follows. Goes with the
                    // diff that refuses reads through symlinks.
                    return self.fs.read_dir_impl(path).await;
                }
                _ => return Err(EdenError::from(err).into()),
            },
            edenfs::DirListAttributeDataOrError::UnknownField(code) => {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::IoEden,
                    "Eden ReadDir returned with unknown field code: {}",
                    code
                ));
            }
        };

        tracing::debug!("readdir({}): {} entries", path, data.len());

        let mut entries = Vec::with_capacity(data.len());
        let mut invalid_names = Vec::new();
        for (file_name, attrs) in data {
            let file_name =
                CompactString::from_utf8(file_name).buck_error_context("Filename is not UTF-8")?;
            let Ok(file_name) = FileNameBuf::try_from(file_name.clone()) else {
                invalid_names.push(file_name.into_string());
                continue;
            };

            let attr_data = match attrs.into_result() {
                Ok(data) => data,
                Err(EdenError::PosixError { code, .. }) if code == libc::EACCES => {
                    // EACCES on per-entry attributes means this is a tented directory --
                    // tenting is tree-level only in EdenFS, so if a file were tented its
                    // parent dir would have been restricted instead.
                    tracing::debug!(
                        "readdir({}): permission denied on entry `{}`, treating as restricted directory",
                        path,
                        file_name
                    );
                    entries.push(RawDirEntry {
                        file_name,
                        file_type: FileType::Directory,
                    });
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let source_control_type = attr_data
                .sourceControlType
                .internal_error("Missing sourceControlType")?
                .into_result()?;

            let file_type = match source_control_type {
                SourceControlType::TREE => FileType::Directory,
                SourceControlType::REGULAR_FILE | SourceControlType::EXECUTABLE_FILE => {
                    FileType::File
                }
                SourceControlType::SYMLINK => FileType::Symlink,
                _ => FileType::Unknown,
            };

            entries.push(RawDirEntry {
                file_name,
                file_type,
            });
        }
        report_invalid_file_names(&path, &invalid_names);

        Ok(ReadDirOutcome::Entries(entries))
    }

    async fn read_file_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<String>> {
        let _guard = IoCounterKey::Read.guard();
        let params = GetFileContentRequest {
            mount: MountId {
                mountPoint: self.manager.get_mount_point(),
                ..Default::default()
            },
            filePath: self.manager.project_path_as_eden_path(path.as_ref()),
            sync: no_sync(),
            ..Default::default()
        };

        let res = self
            .manager
            .with_eden(|eden| {
                tracing::trace!("getFileContent({})", path);
                eden.getFileContent(&params)
            })
            .await;

        match res {
            Ok(res) => match res.blob {
                ScmBlobOrError::blob(content) => {
                    let string_content = String::from_utf8_lossy(&content).into_owned();
                    Ok(Some(string_content))
                }
                ScmBlobOrError::error(err) => {
                    let eden_error = EdenError::from(err);
                    match eden_error {
                        EdenError::PosixError { code, .. } if code == libc::ENOENT => {
                            tracing::debug!("getFileContent({}): File Not Found", path);
                            Ok(None)
                        }
                        EdenError::PosixError { error, code } if code == libc::EFBIG => {
                            // TODO(minglunli): Look at data, if this doesn't happen in practice, enforce the limit for all IoProvider
                            soft_error!(
                                "eden_thrift_size_limit_exceeded",
                                buck2_error::buck2_error!(
                                    buck2_error::ErrorTag::Input,
                                    "File size exceeded Thrift message limit of 2GB, falling back to regular file I/O.
                                    Set env var `BUCK2_DISABLE_EDEN_THRIFT_READ=true` if this is constantly an issue: {:#}",
                                    error
                                ),
                            );

                            return self.fs.read_file_if_exists_impl(path).await;
                        }
                        EdenError::PosixError { code, .. }
                            if code == libc::EINVAL || code == libc::ENOTDIR =>
                        {
                            // A symlink at or above the path, which plain file IO follows. Goes
                            // with the diff that refuses reads through symlinks.
                            return self.fs.read_file_if_exists_impl(path).await;
                        }
                        _ => Err(eden_error.into()),
                    }
                }
                ScmBlobOrError::UnknownField(code) => Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::IoEdenUnknownField,
                    "Eden getFileContent thrift call failed with unknown field code: {}",
                    code
                )),
            },
            Err(e) => Err(e.into()),
        }
    }
}

#[async_trait]
impl IoProvider for EdenIoProvider {
    async fn read_path_metadata_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<RawPathMetadata<ProjectRelativePathBuf>>> {
        match self.read_path_metadata_if_exists_impl(path).await {
            Ok(PathMetadataResult::Result(res)) => Ok(res),
            Ok(PathMetadataResult::Error(err)) => {
                match err {
                    EdenError::PosixError { code, .. } if code == libc::ENOENT => {
                        tracing::debug!("getAttributesFromFilesV2({}): ENOENT", path);
                        Ok(None)
                    }
                    EdenError::PosixError { code, .. } if code == libc::EINVAL => {
                        // Eden versions that do not report `SourceControlType::SYMLINK` say
                        // EINVAL for a symlink instead. The parents are plain directories by the
                        // `IoProvider` contract, so one look at the path itself is all there is
                        // to do, and `None` if it is gone by now.
                        tracing::debug!(
                            "getAttributesFromFilesV2({}): EINVAL, reading as symlink",
                            path
                        );
                        self.fs.read_path_metadata_if_exists_impl(path).await
                    }
                    _ => Err(err.into()),
                }
            }
            Err(err) => Err(err).tag(ErrorTag::IoEden),
        }
    }

    async fn read_file_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<String>> {
        if buck2_env!("BUCK2_ENABLE_EDEN_THRIFT_READ", bool)? || self.use_eden_thrift_read {
            self.read_file_if_exists_impl(path)
                .await
                .tag(ErrorTag::IoEden)
        } else {
            // Don't tag as IoEden because it uses regular file I/O.
            // TODO(minglunli): Can remove this arm if Eden Thrift API is better and stable
            self.fs.read_file_if_exists_impl(path).await
        }
    }

    async fn read_dir_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<ReadDirOutcome> {
        self.read_dir_impl(path)
            .await
            .map_err(|e| e.tag([ErrorTag::IoEden]))
    }

    async fn settle(&self) -> buck2_error::Result<()> {
        let _guard = IoCounterKey::EdenSettle.guard();

        let root = self.manager.get_mount_point();

        self.manager
            .with_eden(move |eden| {
                eden.synchronizeWorkingCopy(
                    &root,
                    &SynchronizeWorkingCopyParams {
                        sync: SyncBehavior {
                            syncTimeoutSeconds: None,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
            })
            .await
            .buck_error_context("Error synchronizing Eden working copy")
            .tag(ErrorTag::IoEden)
    }

    fn name(&self) -> &'static str {
        "eden"
    }

    async fn eden_version(&self) -> buck2_error::Result<Option<String>> {
        Ok(self.manager.get_eden_version().await?)
    }

    async fn verify_eden_identity(&self) -> buck2_error::Result<()> {
        self.manager.verify_identity().await
    }

    fn project_root(&self) -> &ProjectRoot {
        self.fs.project_root()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_eden_repo(&self) -> bool {
        true
    }
}

/// We don't request sync on individual calls because we make a dedicated call for this before we
/// start an operation.
fn no_sync() -> SyncBehavior {
    // NOTE/ we'd rather make this a `const` if we could, but we can't, because
    // `Default::default()` is not a const-fn.
    SyncBehavior {
        syncTimeoutSeconds: Some(0),
        ..Default::default()
    }
}
