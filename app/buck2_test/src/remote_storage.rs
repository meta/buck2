/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Mutex;
use std::time::Duration;

use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileDigestConfig;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_error::Ok;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::re::manager::UnconfiguredRemoteExecutionClient;
use buck2_fs::async_fs_util::spawn_blocking;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_hash::BuckMutMap;
use buck2_test_api::data::RemoteStorageConfig;
use dupe::Dupe;
use remote_execution::NamedDigest;
use remote_execution::TDigest;

type CacheKey = (TDigest, RemoteExecutorUseCase);

pub struct ReClientWithCache {
    client: UnconfiguredRemoteExecutionClient,
    /// The longest TTL granted so far for each digest and use case. A digest is recorded only
    /// once its extension succeeded.
    cache: Mutex<BuckMutMap<CacheKey, Duration>>,
}

impl ReClientWithCache {
    pub fn new(client: UnconfiguredRemoteExecutionClient) -> Self {
        Self {
            client,
            cache: Mutex::new(BuckMutMap::default()),
        }
    }

    pub async fn apply_config(
        &self,
        artifact: &ArtifactValue,
        config: &RemoteStorageConfig,
    ) -> buck2_error::Result<()> {
        match &config.ttl_config {
            Some(ttl_config) => {
                // Note that deps represent artifacts that symlinks depend on. Currently, test artifact trees
                // that contain symlinks cannot be converted into remote objects. Therefore, we do not extend
                // the TTL of symlinks. Additionally, it is rare for test outputs to include symlinks, but if they do,
                // we are materializing them on disk.
                let digests = collect_digests(artifact.entry());

                // Skip digests already granted at least this TTL under this use case.
                let digests_to_extend: Vec<TDigest> = {
                    let cache = self.cache.lock().unwrap();
                    digests
                        .into_iter()
                        .filter(|digest| {
                            cache
                                .get(&(digest.clone(), ttl_config.use_case))
                                .is_none_or(|granted| *granted < ttl_config.ttl)
                        })
                        .collect()
                };

                if digests_to_extend.is_empty() {
                    return Ok(());
                }

                let info = CasDownloadInfo::new_test_artifact(ttl_config.use_case);
                self.client
                    .clone()
                    .with_use_case(info.re_use_case)
                    .extend_digest_ttl(digests_to_extend.clone(), ttl_config.ttl, &info)
                    .await?;

                let mut cache = self.cache.lock().unwrap();
                for digest in digests_to_extend {
                    cache
                        .entry((digest, ttl_config.use_case))
                        .and_modify(|granted| *granted = std::cmp::max(*granted, ttl_config.ttl))
                        .or_insert(ttl_config.ttl);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Upload a raw local file to CAS by its absolute path.
    ///
    /// Computes the file's digest, uploads it to CAS, and returns the RE digest.
    /// Called by the `upload_to_cas` RPC handler when tpx requests CAS upload
    /// of a local test artifact file.
    pub async fn upload_local_file(
        &self,
        local_path: &str,
        digest_config: DigestConfig,
        ttl_seconds: i64,
        use_case: &str,
    ) -> buck2_error::Result<TDigest> {
        let file_config = FileDigestConfig::build(digest_config.cas_digest_config());
        let file_path = AbsPathBuf::new(local_path)?;
        let tracked_digest =
            spawn_blocking(move || FileDigest::from_file(&file_path, file_config)).await??;
        let re_digest = tracked_digest.to_re();

        let re_use_case = RemoteExecutorUseCase::new(use_case.to_owned());

        let info = CasDownloadInfo::new_uploaded(re_use_case);
        let managed_client = self.client.dupe().with_use_case(info.re_use_case);

        managed_client
            .upload_files_and_directories(
                vec![NamedDigest {
                    name: local_path.to_owned(),
                    digest: re_digest.clone(),
                    ..Default::default()
                }],
                vec![],
                vec![],
            )
            .await?;

        managed_client
            .extend_digest_ttl(
                vec![re_digest.clone()],
                Duration::from_secs(ttl_seconds as u64),
                &info,
            )
            .await?;

        Ok(re_digest)
    }
}

fn collect_digests(directory_entry: &ActionDirectoryEntry<ActionSharedDirectory>) -> Vec<TDigest> {
    match directory_entry {
        ActionDirectoryEntry::Leaf(ActionDirectoryMember::File(f)) => vec![f.digest.to_re()],
        ActionDirectoryEntry::Dir(dir) => {
            let mut digests: Vec<_> = dir
                .entries()
                .into_iter()
                .flat_map(|(_, entry)| collect_digests(entry))
                .collect();
            digests.push(dir.fingerprint().to_re());
            digests
        }
        _ => vec![], // Symlink or ExternalSymlink
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_execute::digest_config::DigestConfig;
    use buck2_execute::directory::ActionDirectoryBuilder;
    use buck2_execute::directory::extract_artifact_value;
    use buck2_execute::directory::insert_file;
    use buck2_test_api::data::TtlConfig;

    use super::*;

    fn ttl_config(secs: u64, use_case: &str) -> RemoteStorageConfig {
        RemoteStorageConfig {
            supports_remote: true,
            ttl_config: Some(TtlConfig {
                ttl: Duration::from_secs(secs),
                use_case: RemoteExecutorUseCase::new(use_case.to_owned()),
            }),
        }
    }

    /// A digest is remembered only once its extension succeeded. The dummy client cannot
    /// connect, so every call reaches RE and fails.
    #[tokio::test]
    async fn test_failed_ttl_extension_is_retried() {
        let client = ReClientWithCache::new(UnconfiguredRemoteExecutionClient::testing_new_dummy());
        let artifact = ArtifactValue::file(DigestConfig::testing_default().empty_file());
        for _ in 0..2 {
            assert!(
                client
                    .apply_config(&artifact, &ttl_config(3600, "tpx-default"))
                    .await
                    .is_err()
            );
        }
        assert!(
            client
                .apply_config(&artifact, &ttl_config(30 * 24 * 3600, "other-use-case"))
                .await
                .is_err()
        );
    }

    #[test]
    fn test_collect_digests_dir() {
        let digest_config = DigestConfig::testing_default();
        let mut expected: Vec<TDigest> = vec![];
        let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();

        // construct a directory with 3 files, f1, f2, and f3
        for file in &["d1/f1", "d1/f2", "d1/f3"] {
            let empty_file = FileMetadata {
                digest: TrackedFileDigest::from_content(
                    file.as_bytes(),
                    digest_config.cas_digest_config(),
                ),
                is_executable: false,
            };
            expected.push(empty_file.digest.to_re());
            let _unused = insert_file(
                &mut builder,
                ProjectRelativePath::new(file).unwrap().to_buf(),
                empty_file,
            );
        }

        builder.mark_uniformly_exhaustive();

        let value = extract_artifact_value(
            &builder,
            ProjectRelativePath::new("d1").unwrap(),
            digest_config,
        )
        .expect("Failed to build dir")
        .unwrap();

        let digests = match value.entry() {
            ActionDirectoryEntry::Dir(dir) => {
                expected.push(dir.fingerprint().to_re());
                collect_digests(value.entry())
            }
            _ => vec![],
        };
        assert_eq!(expected, digests);
    }

    #[test]
    fn test_collect_digests_leaf() {
        let digest_config = DigestConfig::testing_default();
        let empty_file = digest_config.empty_file();
        let expected = vec![empty_file.digest.to_re()];

        let leaf_entry: ActionDirectoryEntry<ActionSharedDirectory> =
            ActionDirectoryEntry::Leaf(ActionDirectoryMember::File(empty_file));
        let digests = collect_digests(&leaf_entry);
        assert_eq!(expected, digests);
    }
}
