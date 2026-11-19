/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_data::ParsedTargetPatterns;
use buck2_error::BuckErrorContext;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_hash::BuckMutMap;
use serde::Deserialize;
use serde::Serialize;

use crate::client_utils;

// Version for serialized BuildCount on disk.
// Update if changing BuildCount to allow building with deployed and compiled buck on the same rev.
pub const BUILD_COUNT_VERSION: u64 = 2;

#[derive(
    Default,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Debug
)]
pub struct BuildCount {
    pub successful_build_count: u64,
    pub attempted_build_count: u64,
}

impl BuildCount {
    pub fn new(successful_build_count: u64, attempted_build_count: u64) -> Self {
        Self {
            successful_build_count,
            attempted_build_count,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct BuildCountMap {
    merge_base: String,
    counts: BuckMutMap<String, BuildCount>,
}

impl BuildCountMap {
    fn new(merge_base: String) -> Self {
        Self {
            merge_base,
            counts: BuckMutMap::default(),
        }
    }

    #[cfg(test)]
    fn testing_new(counts: BuckMutMap<String, BuildCount>) -> Self {
        Self {
            merge_base: "testing".to_owned(),
            counts,
        }
    }

    fn increment(&mut self, patterns: &ParsedTargetPatterns, is_success: bool) {
        for target in patterns.target_patterns.iter() {
            match self.counts.get_mut(&target.value) {
                Some(count) => {
                    count.attempted_build_count += 1;
                    if is_success {
                        count.successful_build_count += 1;
                    }
                }
                None => {
                    let count = BuildCount::new(if is_success { 1 } else { 0 }, 1);
                    self.counts.insert(target.value.clone(), count);
                }
            }
        }
    }

    fn min_count(&self, patterns: &ParsedTargetPatterns) -> BuildCount {
        if patterns.target_patterns.is_empty() {
            return Default::default();
        }

        // If the target has never been successfully built it won't be in the map, in that case its count is 0.
        patterns
            .target_patterns
            .iter()
            .map(|v| {
                self.counts
                    .get(&v.value)
                    .copied()
                    .unwrap_or(Default::default())
            })
            .min()
            .unwrap() // target_patterns is non-empty, so min() should return Some
    }
}

/// BuildCountManager keeps track of how many times each target has been successfully built since rebase.
/// This helps understand how much the performance differs between first and incremental builds.
pub struct BuildCountManager {
    base_dir: AbsNormPathBuf,
    file_path: AbsNormPathBuf,
    lock_file_path: AbsNormPathBuf,
}

impl BuildCountManager {
    const LOCK_FILE_NAME: &'static str = "build_count.lock";
    const LOCK_TIMEOUT: Duration = Duration::from_millis(2000);

    pub fn new(base_dir: AbsNormPathBuf) -> buck2_error::Result<Self> {
        let file_path = base_dir.join(FileName::new(&BUILD_COUNT_VERSION.to_string())?);
        let lock_file_path = base_dir.join(FileName::new(Self::LOCK_FILE_NAME)?);
        Ok(Self {
            base_dir,
            file_path,
            lock_file_path,
        })
    }

    async fn ensure_dir(&self) -> buck2_error::Result<()> {
        async_fs_util::create_dir_all(&self.base_dir).await
    }

    async fn read(&self) -> buck2_error::Result<Option<BuildCountMap>> {
        let Some(buffer) = async_fs_util::read_to_string_if_exists(&self.file_path).await? else {
            return Ok(None);
        };
        let build_count_map: BuildCountMap = serde_json::from_str(&buffer)
            .with_buck_error_context(|| {
                format!("Parsing JSON from {}", self.file_path.display())
            })?;
        Ok(Some(build_count_map))
    }

    async fn read_mergebase(&self, merge_base: &str) -> buck2_error::Result<BuildCountMap> {
        if let Some(build_count_map) = self.read().await? {
            if build_count_map.merge_base == merge_base {
                return Ok(build_count_map);
            }
        }
        // If merge base does not match or if there is no existing build count map (due to clean, rebase, etc),
        // create a new build count map
        Ok(BuildCountMap::new(merge_base.to_owned()))
    }

    async fn write(&self, build_count: &BuildCountMap) -> buck2_error::Result<()> {
        async_fs_util::write(&self.file_path, &serde_json::to_vec(build_count)?)
            .await
            .categorize_internal()
    }

    async fn lock_with_timeout(&self) -> buck2_error::Result<FileLockGuard> {
        self.ensure_dir().await?;
        let file = std::fs::File::create(&self.lock_file_path)?;
        let fileref = &file;
        client_utils::retrying(
            Duration::from_millis(5),
            Duration::from_millis(100),
            Self::LOCK_TIMEOUT,
            // Contention (`WouldBlock`) must surface as an error here so the
            // retry loop keeps polling until the lock is actually held.
            || async { buck2_error::Ok(fileref.try_lock().map_err(std::io::Error::from)?) },
        )
        .await?;
        Ok(FileLockGuard { file })
    }

    /// Updates the build counts for set of targets (on success) and returns the min.
    pub async fn increment(
        &self,
        merge_base: &str,
        target_patterns: &ParsedTargetPatterns,
        is_success: bool,
    ) -> buck2_error::Result<BuildCount> {
        let _guard = self.lock_with_timeout().await?;
        let mut build_count_map = self.read_mergebase(merge_base).await?;
        build_count_map.increment(target_patterns, is_success);
        self.write(&build_count_map).await?;
        Ok(build_count_map.min_count(target_patterns))
    }

    /// Returns the existing min build count for the set of targets.
    #[cfg(test)]
    async fn min_count(
        &self,
        target_patterns: &ParsedTargetPatterns,
    ) -> buck2_error::Result<BuildCount> {
        match self.read().await? {
            Some(build_count_map) => Ok(build_count_map.min_count(target_patterns)),
            None => Ok(Default::default()),
        }
    }
}

#[must_use]
struct FileLockGuard {
    file: std::fs::File,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        self.file
            .unlock()
            .expect("Unexpected failure to release a lock file for build count");
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn make_patterns(targets: Vec<&'static str>) -> ParsedTargetPatterns {
        ParsedTargetPatterns {
            target_patterns: targets
                .into_iter()
                .map(|v| buck2_data::TargetPattern {
                    value: v.to_owned(),
                })
                .collect::<Vec<_>>(),
        }
    }

    #[test]
    fn test_update_normal_input() -> buck2_error::Result<()> {
        let mut before = BuckMutMap::default();
        before.insert("//some:target".to_owned(), BuildCount::new(1, 1));
        before.insert("//some/other:target".to_owned(), BuildCount::new(2, 2));
        let mut bc = BuildCountMap::testing_new(before);
        let target_patterns = make_patterns(vec!["//some/other:target", "//yet/another:target"]);
        bc.increment(&target_patterns, true);
        let mut expected = BuckMutMap::default();
        expected.insert("//some:target".to_owned(), BuildCount::new(1, 1));
        expected.insert("//some/other:target".to_owned(), BuildCount::new(3, 3));
        expected.insert("//yet/another:target".to_owned(), BuildCount::new(1, 1));
        assert_eq!(bc.counts, expected);

        Ok(())
    }

    #[test]
    fn test_update_empty_input() -> buck2_error::Result<()> {
        let mut before = BuckMutMap::default();
        before.insert("//some:target".to_owned(), BuildCount::new(1, 1));
        let expected = before.clone();
        let mut bc = BuildCountMap::testing_new(before);
        let target_patterns = make_patterns(vec![]);
        bc.increment(&target_patterns, true);
        assert_eq!(bc.counts, expected);

        Ok(())
    }

    #[test]
    fn test_min_count_some_value() -> buck2_error::Result<()> {
        let mut data = BuckMutMap::default();
        data.insert("//some:target1".to_owned(), BuildCount::new(3, 3));
        data.insert("//some:target2".to_owned(), BuildCount::new(4, 4));
        data.insert("//some:target3".to_owned(), BuildCount::new(5, 5));
        let bc = BuildCountMap::testing_new(data);
        let target_patterns = make_patterns(vec!["//some:target1", "//some:target2"]);
        assert_eq!(bc.min_count(&target_patterns), BuildCount::new(3, 3));

        Ok(())
    }

    #[test]
    fn test_min_count_ignores_others() -> buck2_error::Result<()> {
        let mut data = BuckMutMap::default();
        data.insert("//some:target1".to_owned(), BuildCount::new(3, 3));
        data.insert("//some:target2".to_owned(), BuildCount::new(4, 4));
        data.insert("//some:target3".to_owned(), BuildCount::new(5, 5));
        let bc = BuildCountMap::testing_new(data);
        let target_patterns = make_patterns(vec!["//some:target2"]);
        assert_eq!(bc.min_count(&target_patterns), BuildCount::new(4, 4));

        Ok(())
    }

    #[test]
    fn test_min_count_empty_data() -> buck2_error::Result<()> {
        let data = BuckMutMap::default();
        let bc = BuildCountMap::testing_new(data);
        assert_eq!(bc.min_count(&make_patterns(vec![])), BuildCount::new(0, 0));

        Ok(())
    }

    #[tokio::test]
    async fn test_read_no_such_file() -> buck2_error::Result<()> {
        let no_such_dir = if cfg!(windows) {
            "C:\\no\\such\\dir"
        } else {
            "/no/such/dir"
        };
        let bcm = BuildCountManager::new(AbsNormPathBuf::from(no_such_dir.to_owned())?)?;
        let bc = bcm.read().await?;
        assert_eq!(bc, None);

        Ok(())
    }

    #[tokio::test]
    async fn test_read_normal_file() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        tokio::fs::write(
            &bcm.file_path,
            "{\"merge_base\":\"testing\",\"counts\":{\"//some:target\":[1,1]}}",
        )
        .await?;
        let bc = bcm.read_mergebase("testing").await?;
        let mut expected = BuckMutMap::default();
        expected.insert("//some:target".to_owned(), BuildCount::new(1, 1));
        assert_eq!(bc.counts, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_read_illegal_file_contents() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        tokio::fs::write(&bcm.file_path, "aaa").await?;
        assert!(bcm.read_mergebase("testing").await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_write_normal_input() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        let mut data = BuckMutMap::default();
        data.insert("//some:target".to_owned(), BuildCount::new(1, 1));
        bcm.write(&BuildCountMap::testing_new(data)).await?;
        assert_eq!(
            std::str::from_utf8(&tokio::fs::read(bcm.file_path).await?)?,
            "{\"merge_base\":\"testing\",\"counts\":{\"//some:target\":{\"successful_build_count\":1,\"attempted_build_count\":1}}}",
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_write_empty_input() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        let data = BuckMutMap::default();
        bcm.write(&BuildCountMap::testing_new(data)).await?;
        assert_eq!(
            std::str::from_utf8(&tokio::fs::read(bcm.file_path).await?)?,
            "{\"merge_base\":\"testing\",\"counts\":{}}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_increment_normal_input() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let file_name = "some_file";
        tokio::fs::write(temp_dir.path().join(file_name), "{\"//some:target\":[1,1]}").await?;
        let target_patterns = make_patterns(vec!["//some:target", "//some/other:target"]);
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        assert_eq!(
            bcm.increment(file_name, &target_patterns, true).await?,
            BuildCount::new(1, 1),
        );
        assert_eq!(
            bcm.increment(file_name, &target_patterns, true).await?,
            BuildCount::new(2, 2),
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_increment_on_failure() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let file_name = "some_file";
        tokio::fs::write(temp_dir.path().join(file_name), "{\"//some:target\":[1,1]}").await?;
        let target_patterns = make_patterns(vec!["//some:target", "//some/other:target"]);
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        assert_eq!(
            bcm.increment(file_name, &target_patterns, true).await?,
            BuildCount::new(1, 1),
        );
        assert_eq!(
            bcm.increment(file_name, &target_patterns, false).await?,
            BuildCount::new(1, 2),
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_increment_empty_input() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let file_name = "some_file";
        tokio::fs::write(temp_dir.path().join(file_name), "{}").await?;
        let target_patterns = make_patterns(vec![]);
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        assert_eq!(
            bcm.increment(file_name, &target_patterns, true).await?,
            BuildCount::new(0, 0),
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_min_count_no_increment() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let file_name = "some_file";
        let target_patterns = make_patterns(vec!["//some:target", "//some/other:target"]);
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        let _ = bcm
            .increment(file_name, &make_patterns(vec!["//some:target"]), true)
            .await?;
        let _ = bcm.increment(file_name, &target_patterns, true).await?;
        assert_eq!(
            bcm.min_count(&target_patterns).await?,
            BuildCount::new(1, 1),
        );
        assert_eq!(
            bcm.min_count(&target_patterns).await?,
            BuildCount::new(1, 1),
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_rebase() -> buck2_error::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let merge_base1 = "merge_base1";
        let merge_base2 = "merge_base2";
        let target_patterns = make_patterns(vec!["//some:target"]);
        let bcm = BuildCountManager::new(temp_dir.path().to_path_buf().try_into()?)?;
        let _ = bcm.increment(merge_base1, &target_patterns, true).await?;
        assert_eq!(
            bcm.min_count(&target_patterns).await?,
            BuildCount::new(1, 1),
        );
        let _ = bcm.increment(merge_base2, &target_patterns, true).await?;
        assert_eq!(
            bcm.min_count(&target_patterns).await?,
            BuildCount::new(1, 1),
        );
        let _ = bcm.increment(merge_base1, &target_patterns, true).await?;
        assert_eq!(
            bcm.min_count(&target_patterns).await?,
            BuildCount::new(1, 1),
        );

        Ok(())
    }
}
