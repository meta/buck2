/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Runtime selection of DICE engine ownership for repository tenants.

use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use allocative::Allocative;
use buck2_build_api::configure_dice::DiceEngineConfiguration;
use buck2_common::dice::cells::SetCellResolver;
use buck2_common::legacy_configs::dice::SetLegacyConfigs;
use buck2_execute::digest_config::DigestConfig;
use buck2_server_ctx::concurrency::ConcurrencyHandler;
use buck2_settings::DaemonBuckSettings;
use dice::BranchId;
use dice::Dice;
use dice::DiceData;
use dupe::Dupe;
use pagable::StorageState;
use tokio::sync::OnceCell;
use tokio::sync::watch;

use crate::paging::EvictionGate;

/// Engine ownership selected once when the daemon starts.
#[derive(Allocative)]
pub(crate) enum TenantDiceProvider {
    Separate,
    Shared {
        state_path: PathBuf,
        #[allocative(skip)]
        engine: Box<OnceCell<SharedDiceEngine>>,
    },
}

/// Reclaims one daemon's directory after the storage backend releases its context.
struct SharedDiceStorageLifetime {
    path: PathBuf,
    ownership: Option<File>,
}

impl StorageState for SharedDiceStorageLifetime {}

impl SharedDiceStorageLifetime {
    fn lock_directory(path: &Path) -> io::Result<File> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing storage parent"))?;
        std::fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(parent.join(".lifecycle-lock"))?;
        lock.lock()?;
        Ok(lock)
    }

    fn try_new(path: &Path) -> io::Result<Self> {
        let _directory_lock = Self::lock_directory(path)?;
        let parent = path
            .parent()
            .expect("directory lock requires a storage parent");
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            if entry.path() == path || !entry.file_type()?.is_dir() {
                continue;
            }
            // Only an existing, unlocked ownership file authorizes reclamation.
            if let Ok(ownership) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path().join(".ownership-lock"))
                && ownership.try_lock().is_ok()
            {
                if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                    tracing::warn!(
                        "Error reclaiming shared DICE storage {:?}: {error}",
                        entry.path()
                    );
                }
            }
        }
        std::fs::create_dir_all(path)?;
        let ownership = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join(".ownership-lock"))?;
        ownership
            .try_lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(Self {
            path: path.to_path_buf(),
            ownership: Some(ownership),
        })
    }
}

impl Drop for SharedDiceStorageLifetime {
    fn drop(&mut self) {
        let path = self.path.clone();
        let ownership = self.ownership.take();
        let cleanup = move || {
            let _ownership = ownership;
            let Ok(_directory_lock) = Self::lock_directory(&path) else {
                return;
            };
            if let Err(error) = std::fs::remove_dir_all(&path)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!("Error removing shared DICE storage {path:?}: {error}");
            }
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(cleanup);
        } else {
            cleanup();
        }
    }
}

pub(crate) struct SharedDiceEngine {
    configuration: DiceEngineConfiguration,
    computation_configuration: ComputationConfiguration,
    dice: Arc<Dice>,
    /// Memory-pressure evictions are engine-wide, so every tenant on the engine shares one gate.
    eviction_gate: Arc<EvictionGate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ComputationConfiguration {
    digest_config: Option<DigestConfig>,
    buck_settings: Option<DaemonBuckSettings>,
}

impl ComputationConfiguration {
    fn from_data(data: &DiceData) -> Self {
        Self {
            digest_config: data.get::<DigestConfig>().ok().copied(),
            buck_settings: data.get::<DaemonBuckSettings>().ok().cloned(),
        }
    }
}

impl TenantDiceProvider {
    pub(crate) fn new(share_dice: bool, state_path: PathBuf) -> Self {
        if share_dice {
            Self::Shared {
                state_path,
                engine: Box::new(OnceCell::new()),
            }
        } else {
            Self::Separate
        }
    }

    fn build_engine(
        configuration: &DiceEngineConfiguration,
        memory_pressure: Option<watch::Receiver<bool>>,
    ) -> buck2_error::Result<(Arc<Dice>, Arc<EvictionGate>)> {
        let dice = configuration.build(DiceData::new())?;
        let eviction_gate = Arc::new(EvictionGate::default());
        if let Some(pressure) = memory_pressure {
            crate::paging::spawn_evict_on_memory_pressure(
                Arc::downgrade(&dice),
                eviction_gate.dupe(),
                pressure,
            );
        }
        Ok((dice, eviction_gate))
    }

    pub(crate) fn state_path<'a>(&'a self, tenant_path: &'a Path) -> &'a Path {
        match self {
            Self::Separate => tenant_path,
            Self::Shared { state_path, .. } => state_path,
        }
    }

    #[cfg(test)]
    async fn create(
        &self,
        configuration: DiceEngineConfiguration,
        data: DiceData,
    ) -> buck2_error::Result<(Arc<ConcurrencyHandler>, BranchId)> {
        let (manager, root, _) = self
            .create_with_lifetime_guard(configuration, data, None, None)
            .await?;
        Ok((manager, root))
    }

    pub(crate) async fn create_with_lifetime_guard(
        &self,
        configuration: DiceEngineConfiguration,
        data: DiceData,
        lifetime_guard: Option<Arc<dyn Send + Sync>>,
        memory_pressure: Option<watch::Receiver<bool>>,
    ) -> buck2_error::Result<(Arc<ConcurrencyHandler>, BranchId, Arc<EvictionGate>)> {
        let computation_configuration = ComputationConfiguration::from_data(&data);
        let (dice, eviction_gate) = match self {
            Self::Separate => Self::build_engine(&configuration, memory_pressure)?,
            Self::Shared { engine, state_path } => {
                let engine = engine
                    .get_or_try_init(|| async {
                        let storage = if configuration
                            .dice_state_path()
                            .is_some_and(|path| path == state_path)
                        {
                            let state_path = state_path.clone();
                            Some(
                                tokio::task::spawn_blocking(move || {
                                    SharedDiceStorageLifetime::try_new(&state_path)
                                })
                                .await??,
                            )
                        } else {
                            None
                        };
                        let (dice, eviction_gate) =
                            Self::build_engine(&configuration, memory_pressure)?;
                        if let Some(context) = dice.pagable_storage_context()
                            && let Some(storage) = storage
                        {
                            context.get_or_init(|| storage);
                        }
                        buck2_error::Ok(SharedDiceEngine {
                            dice,
                            configuration: configuration.clone(),
                            computation_configuration: computation_configuration.clone(),
                            eviction_gate,
                        })
                    })
                    .await?;
                engine.configuration.ensure_compatible(&configuration)?;
                if engine.computation_configuration != computation_configuration {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "repository root requested incompatible untracked DICE settings: {computation_configuration:?}; shared engine uses {:?}",
                        engine.computation_configuration
                    ));
                }
                (engine.dice.dupe(), engine.eviction_gate.dupe())
            }
        };
        let root = dice.new_root_with_data(data).await;
        let manager =
            ConcurrencyHandler::new_owned_with_lifetime_guard(dice.dupe(), root, lifetime_guard);
        let mut updater = dice.updater(root);
        updater.set_none_cell_resolver()?;
        updater.set_none_legacy_config_external_data()?;
        updater.commit().await;
        Ok((manager, root, eviction_gate))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
    use buck2_build_api::configure_dice::DiceEngineConfigurationInputs;
    use buck2_build_api::context::SetBuildContextData;
    use buck2_common::cas_digest::CasDigestConfig;
    use buck2_common::cas_digest::DigestAlgorithm;
    use buck2_common::dice::data::HasIsolationDir;
    use buck2_common::dice::data::SetIoProvider;
    use buck2_common::dice::data::SetIsolationDir;
    use buck2_common::file_ops::baseline::FileSystemBaseline;
    use buck2_common::file_ops::baseline::SetFileSystemBaseline;
    use buck2_common::file_ops::dice::DiceFileComputations;
    use buck2_common::file_ops::io::FileChangeTracker;
    use buck2_common::ignores::ignore_set::IgnoreSet;
    use buck2_common::io::fs::FsIoProvider;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use derive_more::Display;
    use dice::DiceComputations;
    use dice::EqualityBehavior;
    use dice::InjectedKey;
    use dice::Key;
    use dice::NoValueSerialize;
    use dice::ValueSerialize;
    use dice_futures::cancellation::CancellationContext;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use super::*;

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct RootValue;

    #[async_trait::async_trait]
    impl Key for RootValue {
        type Value = usize;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancel: &CancellationContext,
        ) -> usize {
            *ctx.compute(&RootInput).await.unwrap()
        }

        fn equality_behavior() -> EqualityBehavior<usize> {
            EqualityBehavior::Compare(|a, b| a == b)
        }

        fn value_serialize() -> impl ValueSerialize<Value = usize> {
            NoValueSerialize::new()
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct RootInput;

    impl InjectedKey for RootInput {
        type Value = usize;

        fn equality_behavior() -> EqualityBehavior<usize> {
            EqualityBehavior::Compare(|a, b| a == b)
        }

        fn value_serialize() -> impl ValueSerialize<Value = usize> {
            NoValueSerialize::new()
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct RootFile;

    #[async_trait::async_trait]
    impl Key for RootFile {
        type Value = String;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancel: &CancellationContext,
        ) -> String {
            ctx.global_data()
                .get::<Arc<AtomicUsize>>()
                .unwrap()
                .fetch_add(1, Ordering::SeqCst);
            DiceFileComputations::read_file_if_exists(ctx, CellPath::testing_new("root//f"))
                .await
                .unwrap()
                .unwrap()
        }

        fn equality_behavior() -> EqualityBehavior<String> {
            EqualityBehavior::Compare(|a, b| a == b)
        }

        fn value_serialize() -> impl ValueSerialize<Value = String> {
            NoValueSerialize::new()
        }
    }

    fn file_data(fs: &ProjectRootTemp, counter: &Arc<AtomicUsize>) -> DiceData {
        let mut data = DiceData::new();
        data.set_io_provider(Arc::new(FsIoProvider::new(
            fs.path().dupe(),
            CasDigestConfig::testing_default(),
            false,
        )));
        data.set(Arc::new(IgnoreSet::testing_empty()));
        data.set(counter.dupe());
        data
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct ResolvedBuckOut;

    #[async_trait::async_trait]
    impl Key for ResolvedBuckOut {
        type Value = String;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancel: &CancellationContext,
        ) -> Self::Value {
            let fs = ctx.get_artifact_fs().await.unwrap();
            fs.fs()
                .resolve(fs.buck_out_path_resolver().root())
                .to_string()
        }

        fn equality_behavior() -> EqualityBehavior<String> {
            EqualityBehavior::Compare(|a, b| a == b)
        }

        fn value_serialize() -> impl ValueSerialize<Value = String> {
            NoValueSerialize::new()
        }
    }

    #[derive(Clone, Debug, Display, Hash, PartialEq, Eq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct ResolvedIsolationDir;

    #[async_trait::async_trait]
    impl Key for ResolvedIsolationDir {
        type Value = String;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancel: &CancellationContext,
        ) -> Self::Value {
            ctx.global_data()
                .get::<Arc<AtomicUsize>>()
                .unwrap()
                .fetch_add(1, Ordering::SeqCst);
            ctx.get_isolation_dir().await.unwrap().to_string()
        }

        fn equality_behavior() -> EqualityBehavior<String> {
            EqualityBehavior::Compare(|a, b| a == b)
        }

        fn value_serialize() -> impl ValueSerialize<Value = String> {
            NoValueSerialize::new()
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_artifact_paths_follow_tenant_roots() -> buck2_error::Result<()> {
        for share in [false, true] {
            let provider = TenantDiceProvider::new(share, PathBuf::from("shared-dice"));
            let first = ProjectRootTemp::new()?;
            let second = ProjectRootTemp::new()?;
            let mut tenants = Vec::new();
            for (fs, isolation, evaluations) in [
                (&first, "one", 1),
                (&first, "two", 1),
                (&second, "one", 1),
                (&first, "one", usize::from(!share)),
            ] {
                let count = Arc::new(AtomicUsize::new(0));
                let (manager, root) = provider
                    .create(configuration(), file_data(fs, &count))
                    .await?;
                let mut updater = manager.unsafe_dice().updater(root);
                updater.set_cell_resolver(CellResolver::testing_with_name_and_path(
                    CellName::testing_new("root"),
                    CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new(String::new())),
                ))?;
                updater
                    .set_file_system_baseline(FileSystemBaseline::Revision(Arc::from("base")))?;
                let out = ProjectRelativePathBuf::try_from(format!("buck-out/{isolation}"))?;
                updater.set_buck_out_path(Some(out.clone()))?;
                updater.set_isolation_dir(fs.path().resolve(&out))?;
                let expected = fs.path().resolve(&out).to_string();
                let tx = updater.commit().await;
                assert_eq!(tx.compute(&ResolvedIsolationDir).await?, &expected);
                assert_eq!(count.load(Ordering::SeqCst), evaluations);
                assert_eq!(tx.compute(&ResolvedBuckOut).await?, &expected);
                drop(tx);
                tenants.push((manager, root, expected));
            }
            for (manager, root, expected) in tenants.iter().rev().cycle().take(8) {
                let tx = manager.unsafe_dice().updater(*root).commit().await;
                assert_eq!(tx.compute(&ResolvedIsolationDir).await?, expected);
                assert_eq!(tx.compute(&ResolvedBuckOut).await?, expected);
            }
            for (manager, _, _) in tenants {
                manager.retire();
            }
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_modes_reuse_matching_file_baselines() -> buck2_error::Result<()> {
        for share in [false, true] {
            let provider = TenantDiceProvider::new(share, PathBuf::from("shared-dice"));
            let mut tenants = Vec::new();
            for (contents, baseline, evaluations) in [
                ("same", "base", 1),
                ("same", "base", usize::from(!share)),
                ("other", "different-base", 1),
            ] {
                let fs = ProjectRootTemp::new()?;
                fs.write_file("f", contents);
                let count = Arc::new(AtomicUsize::new(0));
                let (manager, root) = provider
                    .create(configuration(), file_data(&fs, &count))
                    .await?;
                let mut updater = manager.unsafe_dice().updater(root);
                updater.set_cell_resolver(CellResolver::testing_with_name_and_path(
                    CellName::testing_new("root"),
                    CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new(String::new())),
                ))?;
                updater
                    .set_file_system_baseline(FileSystemBaseline::Revision(Arc::from(baseline)))?;
                updater.set_buck_out_path(None)?;
                updater.set_isolation_dir(
                    fs.path()
                        .resolve(ProjectRelativePath::unchecked_new("buck-out/v2")),
                )?;
                let tx = updater.commit().await;
                assert_eq!(tx.ctx().get_artifact_fs().await?.fs(), fs.path());
                assert_eq!(tx.compute(&RootFile).await?, contents);
                assert_eq!(count.load(Ordering::SeqCst), evaluations);
                drop(tx);
                tenants.push((manager, root, fs));
            }

            let (second, root, fs) = &tenants[1];
            fs.write_file("f", "local-change");
            let mut updater = second.unsafe_dice().updater(*root);
            let mut changes = FileChangeTracker::new();
            changes.file_contents_changed(IProjectRelativePath::intern_directly(
                ProjectRelativePath::new("f")?,
            ));
            changes.write_to_dice(&mut updater)?;
            let tx = updater.commit().await;
            assert_eq!(tx.compute(&RootFile).await?, "local-change");
            drop(tx);
            let (first, first_root, _) = &tenants[0];
            let tx = first.unsafe_dice().updater(*first_root).commit().await;
            assert_eq!(tx.compute(&RootFile).await?, "same");
            drop(tx);
            for (manager, _, _) in tenants {
                manager.retire();
            }
        }
        Ok(())
    }

    fn configuration() -> DiceEngineConfiguration {
        DiceEngineConfiguration::resolve(DiceEngineConfigurationInputs {
            dice_state_path: None,
            pagable_storage_backend: Default::default(),
            page_out_policy: Default::default(),
            eviction_policy: Default::default(),
            defer_field_reads: false,
        })
        .unwrap()
    }

    fn data(value: usize) -> DiceData {
        let mut data = DiceData::new();
        data.set(value);
        data
    }

    async fn await_branch_count(dice: &Dice, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while dice.metrics().branch_count != expected {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("retired tenant roots should be released");
    }

    #[tokio::test]
    async fn memory_pressure_listener_is_owned_by_each_engine() -> buck2_error::Result<()> {
        for sharing in [false, true] {
            let provider = TenantDiceProvider::new(sharing, PathBuf::from("unused"));
            let (pressure, _) = watch::channel(false);
            let (first, _, _) = provider
                .create_with_lifetime_guard(
                    configuration(),
                    DiceData::new(),
                    None,
                    Some(pressure.subscribe()),
                )
                .await?;
            let (second, _, _) = provider
                .create_with_lifetime_guard(
                    configuration(),
                    DiceData::new(),
                    None,
                    Some(pressure.subscribe()),
                )
                .await?;
            assert_eq!(pressure.receiver_count(), if sharing { 1 } else { 2 });
            drop(first);
            drop(provider);
            pressure
                .send(true)
                .expect("remaining engine should retain its pressure subscription");
            drop(second);
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_modes_select_engine_ownership_and_isolate_data() -> buck2_error::Result<()> {
        for share in [false, true] {
            let provider = TenantDiceProvider::new(share, PathBuf::from("shared-dice"));
            let config = configuration();
            let (first, first_root) = provider.create(config.clone(), data(1)).await?;
            let (second, second_root) = provider.create(config, data(2)).await?;
            assert_eq!(
                share,
                Arc::ptr_eq(first.unsafe_dice(), second.unsafe_dice())
            );
            let mut first_updater = first.unsafe_dice().updater(first_root);
            first_updater.changed_to(vec![(RootInput, 1)])?;
            let first_tx = first_updater.commit().await;
            let mut second_updater = second.unsafe_dice().updater(second_root);
            second_updater.changed_to(vec![(RootInput, 2)])?;
            let second_tx = second_updater.commit().await;
            assert_eq!(1, *first_tx.global_data().get::<usize>().unwrap());
            assert_eq!(2, *second_tx.global_data().get::<usize>().unwrap());
            assert_eq!(1, *first_tx.compute(&RootValue).await?);
            assert_eq!(2, *second_tx.compute(&RootValue).await?);
            first.retire();
            tokio::task::yield_now().await;
            assert_eq!(1, *first_tx.compute(&RootValue).await?);
            drop(first_tx);
            await_branch_count(first.unsafe_dice(), usize::from(share)).await;
            assert_eq!(2, *second_tx.compute(&RootValue).await?);
            drop(second_tx);
            second.retire();
            await_branch_count(second.unsafe_dice(), 0).await;
            // Retirement releases roots even while compatibility handles retain the managers.
            assert!(first.unsafe_dice().is_idle().await);
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shared_engine_rejects_different_digest_configuration() -> buck2_error::Result<()> {
        let provider = TenantDiceProvider::new(true, PathBuf::from("shared-dice"));
        let mut first_data = data(1);
        first_data.set(DigestConfig::leak_new(vec![DigestAlgorithm::Sha1], None).unwrap());
        let (first, _) = provider.create(configuration(), first_data).await?;
        let mut second_data = data(2);
        second_data.set(DigestConfig::leak_new(vec![DigestAlgorithm::Sha256], None).unwrap());
        let rejected = provider.create(configuration(), second_data).await;
        assert!(rejected.is_err());
        assert_eq!(first.unsafe_dice().metrics().branch_count, 1);
        first.retire();
        await_branch_count(first.unsafe_dice(), 0).await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_tenant_initialization_creates_one_shared_engine() -> buck2_error::Result<()>
    {
        let provider = TenantDiceProvider::new(true, PathBuf::from("shared-dice"));
        let config = configuration();
        let (first, second) = tokio::join!(
            provider.create(config.clone(), data(1)),
            provider.create(config, data(2))
        );
        let (first, _) = first?;
        let (second, _) = second?;
        assert!(Arc::ptr_eq(first.unsafe_dice(), second.unsafe_dice()));
        let dice = first.unsafe_dice().dupe();
        drop(first);
        drop(second);
        await_branch_count(&dice, 0).await;
        Ok(())
    }

    #[test]
    fn shared_paging_uses_the_daemons_storage_path() {
        let shared_path = PathBuf::from("daemon-dice");
        let tenant_path = PathBuf::from("tenant-dice");
        let shared = TenantDiceProvider::new(true, shared_path.clone());
        let separate = TenantDiceProvider::new(false, shared_path.clone());
        assert_eq!(shared_path, shared.state_path(&tenant_path));
        assert_eq!(tenant_path, separate.state_path(&tenant_path));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shared_paging_storage_outlives_its_provider() -> buck2_error::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("storage");
        let provider = TenantDiceProvider::new(true, path.clone());
        let configuration = DiceEngineConfiguration::resolve(DiceEngineConfigurationInputs {
            dice_state_path: Some(&path),
            pagable_storage_backend: Default::default(),
            page_out_policy: Default::default(),
            eviction_policy: Default::default(),
            defer_field_reads: false,
        })?;
        let (manager, _) = provider.create(configuration, DiceData::new()).await?;
        assert!(path.is_dir());
        drop(provider);
        assert!(path.is_dir(), "a surviving engine still owns its storage");
        drop(manager);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("released storage should reclaim its directory");
        Ok(())
    }
    #[test]
    fn shared_paging_reclaims_only_released_storage() -> buck2_error::Result<()> {
        let directory = tempfile::tempdir()?;
        let live_path = directory.path().join("live");
        let live = SharedDiceStorageLifetime::try_new(&live_path)?;
        let stale_path = directory.path().join("stale");
        let mut stale = SharedDiceStorageLifetime::try_new(&stale_path)?;
        let unknown_path = directory.path().join("unknown");
        std::fs::create_dir(&unknown_path)?;
        std::fs::write(stale_path.join("paged-value"), "stale")?;
        // Simulate process exit releasing kernel ownership without running Drop.
        drop(stale.ownership.take());
        std::mem::forget(stale);
        let replacement_path = directory.path().join("replacement");
        let replacement = SharedDiceStorageLifetime::try_new(&replacement_path)?;
        assert!(!stale_path.exists());
        assert!(live_path.is_dir());
        assert!(
            unknown_path.is_dir(),
            "missing ownership evidence fails closed"
        );
        drop(replacement);
        drop(live);
        Ok(())
    }
}
