/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! Defines utilities to obtain the basic paths for buck2 client and the daemon.

use std::borrow::Cow;
use std::fs::File;
use std::fs::TryLockError;
use std::io::ErrorKind;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use allocative::Allocative;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
pub use buck2_wrapper_common::DEFAULT_ISOLATION_DIR;
use dupe::Dupe;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;

use crate::cancellation::CancellationScope;
use crate::cancellation::CancellationScopeGuard;
use crate::daemon_compatibility::DaemonCompatibilityKey;
use crate::daemon_dir::DaemonDir;
use crate::daemon_routing::DaemonRouting;
use crate::invocation_roots::InvocationRoots;

#[derive(Clone, Allocative)]
pub struct InvocationPaths {
    pub roots: InvocationRoots,
    #[allocative(skip)]
    daemon_compatibility: Arc<Mutex<Option<DaemonCompatibilityKey>>>,
    #[allocative(skip)]
    tenant_generation: Arc<Mutex<Option<u64>>>,

    /// The isolation dir is a relative path used to create unique directories for
    /// a tenant's on-disk state. This allows multiple isolated tenants to use the
    /// same project root.
    ///
    /// Under shared [`DaemonRouting`], the daemon serving the tenant lives in a namespace keyed by
    /// the client's compatibility key, and this directory holds the tenant's proxy. The tenant's
    /// buck-out remains `<projectroot>/buck-out/<isolationdir>/`.
    ///
    /// Any tenant-specific on-disk state from the daemon (including build outputs and similar)
    /// should only be written or read from directories that include this component.
    ///
    /// This form of isolation is currently supported primarily for two uses:
    ///
    /// 1. testing - it allows us to run isolated daemons on a project for tests. This is
    ///    particularly useful to allow a test in a project to recursively invoke buck, but also
    ///    useful to write tests against a project's macros and rules and using a project's real
    ///    configuration.
    ///
    /// 2. generally to support recursive buck invocations. while our ideal may be that these
    ///    eventually are not allowed, the most pragmatic approach currently is to support them
    ///    but push them into isolated, temporary daemons.
    pub isolation: FileNameBuf,

    /// How this invocation's client selects its daemon. Read from the root buckconfig before
    /// any daemon is contacted, so every path that depends on it agrees across clients. `None`
    /// for commands that never select a daemon, which then skip parsing the root buckconfig.
    routing: Option<DaemonRouting>,
}

/// Paths whose lifetime is tied to a tenant rather than an individual invocation.
#[derive(Clone, Allocative)]
pub struct TenantPaths {
    project_root: ProjectRoot,
    isolation: FileNameBuf,
}

impl TenantPaths {
    /// Creates tenant paths from their stable address components.
    pub fn new(project_root: ProjectRoot, isolation: FileNameBuf) -> Self {
        Self {
            project_root,
            isolation,
        }
    }

    /// A persistent lock file outside the tenant's disposable output and daemon directories.
    pub fn disk_ownership_lock_path(&self) -> buck2_error::Result<AbsNormPathBuf> {
        let paths = self.unshared_invocation_paths();
        Ok(paths
            .project_daemon_dir()?
            .path
            .join(FileName::unchecked_new("._buck2_tenant_locks"))
            .join(&self.isolation))
    }

    /// Waits until no process owns the tenant's directories, for up to `timeout`. A client starts
    /// an unshared daemon over them only once a previous owner has finished releasing them.
    pub async fn wait_until_disk_unowned(
        &self,
        timeout: Duration,
    ) -> buck2_error::Result<Result<(), TenantDiskStillOwned>> {
        Ok(TenantDiskLock::wait_for(self, timeout).await?.map(drop))
    }

    /// The daemon directory used by clients without shared daemon routing.
    pub fn unshared_daemon_dir(&self) -> buck2_error::Result<DaemonDir> {
        let paths = self.unshared_invocation_paths();
        Ok(DaemonDir {
            path: paths.project_daemon_dir()?.path.join(&self.isolation),
        })
    }

    /// Invocation paths for this tenant under unshared routing, for the paths that do not depend
    /// on routing.
    fn unshared_invocation_paths(&self) -> InvocationPaths {
        InvocationPaths::new(
            InvocationRoots {
                project_root: self.project_root.clone(),
                cwd: ProjectRelativePath::empty().to_buf(),
            },
            self.isolation.clone(),
            Some(DaemonRouting::Unshared),
        )
    }

    /// Returns the tenant's project root.
    pub fn project_root(&self) -> &ProjectRoot {
        &self.project_root
    }

    /// Returns the tenant's isolation directory name.
    pub fn isolation(&self) -> &FileName {
        &self.isolation
    }

    /// Directory containing remote-execution logs for this tenant.
    pub fn re_logs_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("re_logs"))
    }

    /// Top-level directory name under the project root used for Buck outputs.
    pub fn buck_out_dir_prefix() -> &'static ProjectRelativePath {
        ProjectRelativePath::unchecked_new("buck-out")
    }

    /// Project-relative output directory for this tenant.
    pub fn buck_out_dir(&self) -> ProjectRelativePathBuf {
        Self::buck_out_dir_prefix().join(&self.isolation)
    }

    /// Absolute output directory for this tenant.
    pub fn buck_out_path(&self) -> AbsNormPathBuf {
        self.project_root.root().join(self.buck_out_dir())
    }

    /// Project-relative cache directory for this tenant.
    pub fn cache_dir(&self) -> ProjectRelativePathBuf {
        self.buck_out_dir()
            .join(ForwardRelativePath::unchecked_new("cache"))
    }

    /// Project-relative paranoid-download cache directory for this tenant.
    pub fn paranoid_cache_dir(&self) -> ProjectRelativePathBuf {
        self.buck_out_dir()
            .join(ForwardRelativePath::unchecked_new("paranoid"))
    }

    /// Absolute cache directory for this tenant.
    pub fn cache_dir_path(&self) -> AbsNormPathBuf {
        self.project_root.root().join(self.cache_dir())
    }

    /// Path containing persisted materializer state for this tenant.
    pub fn materializer_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path()
            .join(FileName::unchecked_new("materializer_state"))
    }

    /// Path containing content-based incremental state for this tenant.
    pub fn incremental_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path()
            .join(FileName::unchecked_new("incremental_state"))
    }

    /// Path containing persisted local dep-file state for this tenant.
    pub fn dep_file_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path()
            .join(FileName::unchecked_new("dep_file_state"))
    }

    /// Path containing paged-out DICE state for this tenant.
    pub fn dice_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path()
            .join(FileName::unchecked_new("dice_state"))
    }

    /// Cache subdirectories that persist across tenant initialization.
    pub fn valid_cache_dirs(&self) -> Vec<&FileName> {
        vec![
            FileName::unchecked_new("materializer_state"),
            FileName::unchecked_new("incremental_state"),
            FileName::unchecked_new("dep_file_state"),
        ]
    }
}

/// An exclusive lock a unidaemon holds for its whole lifetime, taken before it writes anything to
/// its namespace. The kernel releases it when the daemon exits however it exits, so whether it is
/// held tells a client whether a daemon of the namespace is running, whatever the namespace's
/// metadata says: a crash before `buckd.info` is written, damaged metadata, and a recorded pid
/// that another process has since taken all leave the answer right.
///
/// The lock file lives beside the namespace's lifecycle lock, outside the daemon directory that
/// cleanup clears. The descriptor is close-on-exec, so processes the daemon spawns do not keep it.
pub struct UnidaemonLifetimeLock {
    _file: File,
}

impl UnidaemonLifetimeLock {
    /// Takes the lock for the calling daemon, waiting out clients that are probing it. Fails if
    /// another daemon of the namespace holds it.
    pub fn acquire(
        paths: &InvocationPaths,
        key: &DaemonCompatibilityKey,
        timeout: Duration,
    ) -> buck2_error::Result<Self> {
        let path = paths.unidaemon_lifetime_lock_path(key)?;
        let file = Self::open(&path)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Another daemon of unidaemon namespace `{key}` is running (`{path}` is locked)"
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(
                        buck2_error::Error::from(error).context(format!("Locking `{path}`"))
                    );
                }
            }
        }
    }

    /// Whether a daemon of the namespace holds the lock. A probe takes the lock shared, so
    /// concurrent probes never mistake each other for a daemon, and does not create the lock
    /// file: a namespace whose daemon never created it has no daemon that holds it.
    pub async fn is_held(
        paths: &InvocationPaths,
        key: &DaemonCompatibilityKey,
    ) -> buck2_error::Result<bool> {
        let path = paths.unidaemon_lifetime_lock_path(key)?;
        tokio::task::spawn_blocking(move || Self::is_held_at(&path)).await?
    }

    fn is_held_at(path: &AbsNormPathBuf) -> buck2_error::Result<bool> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(buck2_error::Error::from(error).context(format!("Probing `{path}`")));
            }
        };
        match file.try_lock_shared() {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(error)) => {
                Err(buck2_error::Error::from(error).context(format!("Probing `{path}`")))
            }
        }
    }

    /// Whether the lock is free, waiting up to `timeout` for a daemon that holds it to exit. A
    /// daemon releases the lock only as its process ends, after it has removed its connection
    /// records, so a client that has seen the process go can still find the lock held for a
    /// moment.
    pub async fn wait_until_free(
        paths: &InvocationPaths,
        key: &DaemonCompatibilityKey,
        timeout: Duration,
    ) -> buck2_error::Result<bool> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if !Self::is_held(paths, key).await? {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn open(path: &AbsNormPathBuf) -> buck2_error::Result<File> {
        std::fs::create_dir_all(path.parent().internal_error("Lock file has a parent")?)?;
        Ok(std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?)
    }
}

/// The OS file lock behind a tenant's disk ownership, taken by a generation through
/// [`LeasedTenantPaths`] and by cleanup through [`TenantDiskForCleanup`].
#[derive(Allocative)]
struct TenantDiskLock {
    #[allocative(skip)]
    _file: File,
}

/// Another process still owned a tenant's disk state when [`TenantDiskLock::wait_for`] stopped
/// waiting.
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = BuckdLifecycleLock)]
#[error(
    "Timed out waiting for exclusive use of `{buck_out}`: another process still owns it, either a Buck2 daemon or a running `buck2 clean`. Wait for it to finish, or stop the daemon (for example with `buck2 killall`), then retry."
)]
pub struct TenantDiskStillOwned {
    pub buck_out: AbsNormPathBuf,
}

impl TenantDiskLock {
    /// Waits for disk ownership without interrupting the existing owner.
    async fn acquire(paths: &TenantPaths, timeout: Duration) -> buck2_error::Result<Self> {
        Ok(Self::wait_for(paths, timeout).await??)
    }

    /// Like [`acquire`](Self::acquire), but tells a wait that ran out, while another process still
    /// owned the disk state, from other failures.
    async fn wait_for(
        paths: &TenantPaths,
        timeout: Duration,
    ) -> buck2_error::Result<Result<Self, TenantDiskStillOwned>> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(lock) = Self::try_acquire(paths).await? {
                    break buck2_error::Ok(lock);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_or_else(
            |_elapsed| {
                Ok(Err(TenantDiskStillOwned {
                    buck_out: paths.buck_out_path(),
                }))
            },
            |lock| Ok(Ok(lock?)),
        )
    }

    async fn try_acquire(paths: &TenantPaths) -> buck2_error::Result<Option<Self>> {
        Ok(Self::try_lock_file(&paths.disk_ownership_lock_path()?)
            .await?
            .map(|file| Self { _file: file }))
    }

    async fn try_lock_file(path: &AbsNormPathBuf) -> buck2_error::Result<Option<File>> {
        tokio::fs::create_dir_all(path.parent().expect("Lock file has a parent")).await?;
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .await?
            .into_std()
            .await;
        match file.try_lock() {
            Ok(()) => buck2_error::Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
        .buck_error_context(format!("Locking tenant disk state at `{path}`"))
    }
}

/// Exclusive ownership of a tenant's output and cache directories by a process that is not
/// serving the tenant, held while deleting them. Taking it waits for the owning generation's
/// disk work to drain, and no daemon can take the directories until it is dropped.
#[derive(Allocative)]
pub struct TenantDiskForCleanup {
    _lock: TenantDiskLock,
}

impl TenantDiskForCleanup {
    pub async fn take(paths: &TenantPaths, timeout: Duration) -> buck2_error::Result<Self> {
        Ok(Self {
            _lock: TenantDiskLock::acquire(paths, timeout).await?,
        })
    }
}

/// One generation's ownership of a tenant's output and cache directories.
///
/// The permit serializes generations of the tenant within this daemon, the file lock serializes
/// them across daemons, and the scope registration keeps the generation's eviction incomplete
/// while the lease exists. Held through [`LeasedTenantPaths`] and by the blocking executor that
/// runs the generation's I/O, so the lease outlives every reader of the paths and every request
/// in flight.
pub struct TenantDiskLease {
    _permit: OwnedSemaphorePermit,
    _lock: TenantDiskLock,
    _lifetime: Arc<CancellationScopeGuard>,
}

/// A tenant's paths, obtainable only with the tenant's disk lease.
///
/// Daemon code reaches a tenant's output and cache directories through these paths, so a
/// generation cannot touch them before it owns them, and a replacement generation cannot own them
/// until every holder of this value and every I/O its executor retains are gone. Cloning shares
/// the lease; [`Self::without_lease`] is for records that must not keep the generation's disk
/// state alive.
#[derive(Clone, Allocative)]
pub struct LeasedTenantPaths {
    paths: TenantPaths,
    #[allocative(skip)]
    lease: Arc<TenantDiskLease>,
}

impl LeasedTenantPaths {
    /// Takes the tenant's disk ownership for a generation of `scope`, waiting for the previous
    /// owner within this daemon and for up to `lock_timeout` for one in another process.
    pub async fn acquire(
        paths: TenantPaths,
        gate: Arc<Semaphore>,
        scope: &CancellationScope,
        lock_timeout: Duration,
    ) -> buck2_error::Result<Self> {
        let lifetime = scope.register()?;
        let permit = gate
            .acquire_owned()
            .await
            .internal_error("Tenant disk ownership gate should remain open")?;
        let lock = TenantDiskLock::acquire(&paths, lock_timeout).await?;
        Ok(Self {
            paths,
            lease: Arc::new(TenantDiskLease {
                _permit: permit,
                _lock: lock,
                _lifetime: lifetime,
            }),
        })
    }

    /// The lease itself, for an executor to retain while it runs the generation's I/O.
    pub fn lease(&self) -> Arc<TenantDiskLease> {
        self.lease.dupe()
    }

    /// The paths alone. The result keeps no disk ownership alive, so it suits status records and
    /// other bookkeeping that must not delay a replacement generation; it must not be used for I/O.
    pub fn without_lease(&self) -> TenantPaths {
        self.paths.clone()
    }
}

impl Deref for LeasedTenantPaths {
    type Target = TenantPaths;

    fn deref(&self) -> &TenantPaths {
        &self.paths
    }
}

impl InvocationPaths {
    pub fn new(
        roots: InvocationRoots,
        isolation: FileNameBuf,
        routing: Option<DaemonRouting>,
    ) -> Self {
        Self {
            roots,
            isolation,
            routing,
            daemon_compatibility: Arc::new(Mutex::new(None)),
            tenant_generation: Arc::new(Mutex::new(None)),
        }
    }

    /// The same paths under another routing, as a daemon applies the routing it was started with.
    pub fn with_routing(self, routing: DaemonRouting) -> Self {
        Self {
            routing: Some(routing),
            ..self
        }
    }

    pub fn routing(&self) -> buck2_error::Result<DaemonRouting> {
        self.routing
            .internal_error("Daemon routing is not resolved for this command")
    }

    /// Whether this command addresses a shared daemon. A command that never selects a daemon
    /// leaves routing unresolved and addresses none.
    pub fn shares_daemon(&self) -> bool {
        self.routing.is_some_and(|routing| routing.is_shared())
    }

    pub fn select_unidaemon(&self, key: DaemonCompatibilityKey) {
        *self
            .daemon_compatibility
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(key);
    }

    pub fn select_tenant_generation(&self, generation: u64) {
        *self
            .tenant_generation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(generation);
    }

    pub fn tenant_generation(&self) -> Option<u64> {
        *self
            .tenant_generation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub fn unidaemon_key(&self) -> Option<DaemonCompatibilityKey> {
        self.daemon_compatibility
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Returns the stable tenant paths, excluding the invocation cwd.
    pub fn tenant_paths(&self) -> TenantPaths {
        TenantPaths::new(self.project_root().clone(), self.isolation.clone())
    }

    fn project_daemon_dir(&self) -> buck2_error::Result<DaemonDir> {
        #[cfg(windows)]
        let root_relative: Cow<ForwardRelativePath> = {
            use buck2_fs::paths::forward_rel_path::ForwardRelativePathNormalizer;

            // Get drive letter, network share name, etc.
            // Network share contains '\' therefore it needs to be normalized.
            let prefix = self.roots.project_root.root().windows_prefix()?;
            let stripped_path = ForwardRelativePathNormalizer::normalize_path(
                self.roots.project_root.root().strip_windows_prefix()?,
            )?;
            Cow::Owned(ForwardRelativePathNormalizer::normalize_path(&prefix)?.join(stripped_path))
        };
        #[cfg(not(windows))]
        let root_relative: Cow<ForwardRelativePath> = self
            .roots
            .project_root
            .root()
            .strip_prefix(buck2_fs::paths::abs_norm_path::AbsNormPath::new("/")?)?;

        let path = self.roots.common_buckd_dir()?.join(root_relative.as_ref());
        Ok(DaemonDir { path })
    }

    /// See [`UnidaemonLifetimeLock`].
    pub fn unidaemon_lifetime_lock_path(
        &self,
        key: &DaemonCompatibilityKey,
    ) -> buck2_error::Result<AbsNormPathBuf> {
        Ok(self
            .roots
            .common_buckd_dir()?
            .join(FileName::unchecked_new(UNIDAEMON_LOCKS_DIR))
            .join(FileName::new(&format!("{key}.alive"))?))
    }

    pub fn unidaemon_dir(&self, key: &DaemonCompatibilityKey) -> buck2_error::Result<DaemonDir> {
        Ok(DaemonDir {
            path: self
                .roots
                .common_buckd_dir()?
                .join(FileName::unchecked_new(UNIDAEMON_NAMESPACES_DIR))
                .join(key.file_name())
                .join(FileName::unchecked_new(DEFAULT_ISOLATION_DIR)),
        })
    }

    pub fn daemon_dir(&self) -> buck2_error::Result<DaemonDir> {
        if let Some(key) = self.unidaemon_key() {
            return self.unidaemon_dir(&key);
        }
        Ok(DaemonDir {
            path: self.project_daemon_dir()?.path.join(&self.isolation),
        })
    }

    pub fn project_root(&self) -> &ProjectRoot {
        &self.roots.project_root
    }

    pub fn log_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("log"))
    }

    pub fn tmp_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("tmp"))
    }

    pub fn re_logs_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("re_logs"))
    }

    pub fn build_count_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("build_count"))
    }

    pub fn dice_dump_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("dice_dump"))
    }

    pub fn buck_out_dir_prefix() -> &'static ProjectRelativePath {
        TenantPaths::buck_out_dir_prefix()
    }

    pub fn buck_out_dir(&self) -> ProjectRelativePathBuf {
        Self::buck_out_dir_prefix().join(&self.isolation)
    }

    pub fn buck_out_path(&self) -> AbsNormPathBuf {
        self.roots.project_root.root().join(self.buck_out_dir())
    }

    /// Directory containing on-disk cache
    pub fn cache_dir(&self) -> ProjectRelativePathBuf {
        self.buck_out_dir()
            .join(ForwardRelativePath::unchecked_new("cache"))
    }

    /// Temporary directory for paranoid downloads.
    pub fn paranoid_cache_dir(&self) -> ProjectRelativePathBuf {
        self.buck_out_dir()
            .join(ForwardRelativePath::unchecked_new("paranoid"))
    }

    pub fn cache_dir_path(&self) -> AbsNormPathBuf {
        self.roots.project_root.root().join(self.cache_dir())
    }

    /// Subdirectory of `cache_dir` responsible for storing materializer state
    pub fn materializer_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path()
            .join(self.materializer_state_dir_name())
    }

    /// Subdirectory of `cache_dir` responsible for storing content-based incremental path state
    pub fn incremental_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path()
            .join(self.incremental_state_dir_name())
    }

    /// Subdirectory of `cache_dir` responsible for storing the persisted local dep-file
    /// (local action) cache.
    pub fn dep_file_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path().join(self.dep_file_state_dir_name())
    }

    /// Subdirectory of `cache_dir` holding event logs downloaded from remote storage
    /// by `buck2 log` commands, and their in-flight `.tmp` staging files. Kept apart
    /// from `log_dir` so downloads never appear in local log listings, and apart from
    /// the action scratch dirs, which are cleaned by liveness.
    ///
    /// Deliberately absent from [`Self::valid_cache_dirs`], so it is wiped on daemon
    /// startup: it is purely a cache of remote content, and the wipe bounds it
    /// without a dedicated reaper.
    pub fn log_download_cache_dir(&self) -> AbsNormPathBuf {
        self.cache_dir_path().join(FileName::unchecked_new("logs"))
    }

    /// Subdirectory of `cache_dir` responsible for storing paged-out DICE node
    /// values (see `hydration.enable_paging`).
    ///
    /// Deliberately absent from [`Self::valid_cache_dirs`], so it is wiped on
    /// daemon startup: the `DiceKey`->`DataKey` mapping needed to read it back
    /// lives only in the in-memory DICE graph, so a paged-out DB cannot outlive
    /// the daemon that wrote it.
    pub fn dice_state_path(&self) -> AbsNormPathBuf {
        self.cache_dir_path().join(self.dice_state_dir_name())
    }

    /// Daemon-scoped state outside every tenant's disposable output directory.
    pub fn daemon_state_dir(&self) -> AbsNormPathBuf {
        self.project_root()
            .root()
            .join(Self::buck_out_dir_prefix())
            .join(FileName::unchecked_new(RESERVED_BUCK_OUT_PREFIX))
    }

    /// The daemon's working directory. A shared daemon must outlive `buck2 clean` of any one
    /// tenant, so it lives outside the tenant output directories; an unshared daemon runs in
    /// the tenant's buck-out.
    pub fn daemon_working_dir(&self) -> buck2_error::Result<AbsNormPathBuf> {
        Ok(if self.routing()?.is_shared() {
            self.daemon_state_dir()
        } else {
            self.buck_out_path()
        })
    }

    /// Daemon-scoped paged DICE storage, outside every disposable tenant tree.
    pub fn shared_dice_state_dir(&self) -> AbsNormPathBuf {
        self.daemon_state_dir()
            .join(FileName::unchecked_new("dice_state"))
    }

    /// Per-boot shared paging storage, scoped by daemon id. Independent of
    /// tenant generation and output isolation.
    pub fn shared_dice_state_path(&self, daemon_id: &str) -> buck2_error::Result<AbsNormPathBuf> {
        Ok(self.shared_dice_state_dir().join(FileName::new(daemon_id)?))
    }

    /// Private executable state for the daemon's forkserver, under execution-allowlisted
    /// buck-out. A shared daemon keeps it beside its working directory, so that `buck2 clean` of
    /// a tenant leaves it intact, under its namespace. As with an unshared daemon's state, which
    /// daemons under different homes started over one checkout and isolation directory share, a
    /// daemon only ever touches its own directory.
    pub fn forkserver_state_dir(&self) -> buck2_error::Result<AbsNormPathBuf> {
        if !self.routing()?.is_shared() {
            return Ok(self
                .buck_out_path()
                .join(FileName::unchecked_new("forkserver")));
        }
        let key = self
            .unidaemon_key()
            .internal_error("A shared daemon serves a namespace")?;
        Ok(self.shared_forkserver_state_root().join(key.file_name()))
    }

    /// Forkserver state of the shared daemons started from this checkout, one entry per
    /// namespace.
    pub fn shared_forkserver_state_root(&self) -> AbsNormPathBuf {
        self.daemon_state_dir()
            .join(FileName::unchecked_new("forkserver"))
    }

    fn materializer_state_dir_name(&self) -> &FileName {
        FileName::unchecked_new("materializer_state")
    }

    fn incremental_state_dir_name(&self) -> &FileName {
        FileName::unchecked_new("incremental_state")
    }

    fn dep_file_state_dir_name(&self) -> &FileName {
        FileName::unchecked_new("dep_file_state")
    }

    fn dice_state_dir_name(&self) -> &FileName {
        FileName::unchecked_new("dice_state")
    }

    pub fn valid_cache_dirs(&self) -> Vec<&FileName> {
        // `dice_state` is intentionally omitted so it is wiped on startup; see
        // `dice_state_path`.
        vec![
            self.materializer_state_dir_name(),
            self.incremental_state_dir_name(),
            self.dep_file_state_dir_name(),
        ]
    }

    /// This is used by the health check server and client to preserve states across runs, and for temporary files.
    pub fn health_check_state_dir(&self) -> AbsNormPathBuf {
        self.buck_out_path()
            .join(ForwardRelativePath::unchecked_new("health_check"))
    }

    /// Trash directory for background clean operations.
    /// Files moved here can be deleted asynchronously without blocking the main clean operation.
    /// This points to buck-out/._buck2/trash which is used as the trash directory.
    pub fn trash_dir(&self) -> AbsNormPathBuf {
        self.roots
            .project_root
            .root()
            .join(Self::buck_out_dir_prefix())
            .join(ForwardRelativePath::unchecked_new(RESERVED_BUCK_OUT_PREFIX))
            .join(ForwardRelativePath::unchecked_new("trash"))
    }
}

/// Top-level names under `buck-out` share a namespace with isolation dirs. Names starting with
/// this prefix are reserved for buck2's own bookkeeping (e.g. `trash_dir`) and are rejected as
/// `--isolation-dir` values so the two can never collide.
///
/// One entry is set aside for tools other than buck2: `buck-out/._buck2/tmp` is scratch space
/// for tooling (e.g. compiler wrappers invoked outside of a buck2 action) that needs a temp
/// location under buck-out. Buck2 never stores its own state there, and `clean --all` deletes
/// it like any other reserved entry, so contents must be disposable. Shared daemons keep their
/// working directory and forkserver state under sibling entries of this prefix; those are
/// recreated on daemon start.
pub const RESERVED_BUCK_OUT_PREFIX: &str = "._buck2";

/// Under the common daemon directory, the directory holding one shared-daemon namespace per
/// compatibility key.
pub const UNIDAEMON_NAMESPACES_DIR: &str = ".unidaemons";
/// Beside [`UNIDAEMON_NAMESPACES_DIR`]: the lifecycle locks of the namespaces, named by their key.
pub const UNIDAEMON_LOCKS_DIR: &str = ".unidaemon_locks";

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use buck2_core::fs::project::ProjectRoot;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_fs::paths::abs_norm_path::AbsNormPath;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::file_name::FileNameBuf;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;

    use crate::daemon_compatibility::DaemonCompatibilityKey;
    use crate::daemon_routing::DaemonRouting;
    use crate::init::DaemonStartupConfig;
    use crate::invocation_paths::InvocationPaths;
    use crate::invocation_roots::InvocationRoots;

    #[test]
    fn test_paths() {
        let project_root = if cfg!(windows) {
            "C:\\my\\project"
        } else {
            "/my/project"
        };
        let paths = InvocationPaths::new(
            InvocationRoots {
                project_root: ProjectRoot::new_unchecked(
                    AbsNormPathBuf::try_from(project_root.to_owned()).unwrap(),
                ),
                cwd: ProjectRelativePath::empty().to_buf(),
            },
            FileNameBuf::unchecked_new("isolation"),
            Some(DaemonRouting::Unshared),
        );

        let expected_path = if cfg!(windows) {
            ".buck\\buckd\\C\\my\\project\\isolation"
        } else {
            ".buck/buckd/my/project/isolation"
        };
        assert_eq!(
            paths.daemon_dir().unwrap().path.as_os_str(),
            AbsNormPathBuf::try_from(
                dirs::home_dir().expect("Expected a HOME directory to be available")
            )
            .expect("Expected an absolute HOME directory")
            .join(ForwardRelativePath::unchecked_new(expected_path))
            .as_os_str()
        );

        let other_paths = InvocationPaths::new(
            paths.roots.clone(),
            FileNameBuf::unchecked_new("other-isolation"),
            Some(DaemonRouting::Unshared),
        );
        assert_ne!(
            paths.daemon_dir().unwrap().path,
            other_paths.daemon_dir().unwrap().path,
        );
        assert_ne!(paths.buck_out_path(), other_paths.buck_out_path());

        // A shared daemon outlives any one tenant's buck-out, so it works outside them; until a
        // namespace is selected, the tenant's own directory holds its proxy.
        let shared = InvocationPaths::new(
            paths.roots.clone(),
            FileNameBuf::unchecked_new("isolation"),
            Some(DaemonRouting::Shared),
        );
        assert_eq!(
            shared.daemon_dir().unwrap().path,
            paths.daemon_dir().unwrap().path
        );
        assert_eq!(
            shared.daemon_working_dir().unwrap(),
            shared.daemon_state_dir()
        );
        assert_eq!(paths.daemon_working_dir().unwrap(), paths.buck_out_path());

        // Commands that never select a daemon leave routing unresolved; anything that depends on
        // it reports that instead of guessing.
        let unrouted = InvocationPaths::new(
            paths.roots.clone(),
            FileNameBuf::unchecked_new("isolation"),
            None,
        );
        assert!(unrouted.routing().is_err());
        assert!(unrouted.daemon_working_dir().is_err());
        assert_eq!(unrouted.buck_out_path(), paths.buck_out_path());

        // A unidaemon directory depends on neither the checkout nor the isolation directory.
        let key = DaemonCompatibilityKey::new(
            "release-a",
            None,
            &DaemonStartupConfig::testing_empty().serialize().unwrap(),
            false,
        );
        let mut other_checkout = other_paths;
        let other_root = if cfg!(windows) {
            "C:\\other\\project"
        } else {
            "/other/project"
        };
        other_checkout.roots.project_root =
            ProjectRoot::new_unchecked(AbsNormPathBuf::try_from(other_root.to_owned()).unwrap());
        assert_eq!(
            paths.unidaemon_dir(&key).unwrap().path,
            other_checkout.unidaemon_dir(&key).unwrap().path,
        );

        let expected_path = if cfg!(windows) {
            "C:\\my\\project"
        } else {
            "/my/project"
        };
        assert_eq!(
            paths.project_root().root().as_os_str(),
            AbsNormPath::new(expected_path).unwrap().as_os_str()
        );

        assert_eq!(
            paths.buck_out_dir(),
            ProjectRelativePathBuf::unchecked_new("buck-out/isolation".to_owned())
        );
        let expected_path = if cfg!(windows) {
            "C:\\my\\project\\buck-out\\isolation"
        } else {
            "/my/project/buck-out/isolation"
        };
        assert_eq!(paths.buck_out_path().as_os_str(), OsStr::new(expected_path));

        let expected_path = if cfg!(windows) {
            "C:\\my\\project\\buck-out\\isolation\\log"
        } else {
            "/my/project/buck-out/isolation/log"
        };
        assert_eq!(paths.log_dir().as_os_str(), OsStr::new(expected_path));
        let expected_path = if cfg!(windows) {
            "C:\\my\\project\\buck-out\\isolation\\dice_dump"
        } else {
            "/my/project/buck-out/isolation/dice_dump"
        };
        assert_eq!(paths.dice_dump_dir().as_os_str(), OsStr::new(expected_path));

        assert_eq!(
            paths.cache_dir(),
            ProjectRelativePathBuf::unchecked_new("buck-out/isolation/cache".to_owned())
        );

        let expected_path = if cfg!(windows) {
            "C:\\my\\project\\buck-out\\isolation\\cache\\materializer_state"
        } else {
            "/my/project/buck-out/isolation/cache/materializer_state"
        };
        assert_eq!(
            paths.materializer_state_path().as_os_str(),
            OsStr::new(expected_path),
        );

        let expected_path = if cfg!(windows) {
            "C:\\my\\project\\buck-out\\isolation\\health_check"
        } else {
            "/my/project/buck-out/isolation/health_check"
        };
        assert_eq!(
            paths.health_check_state_dir().as_os_str(),
            OsStr::new(expected_path),
        );

        let tenant_paths = paths.tenant_paths();
        assert_eq!(tenant_paths.project_root(), paths.project_root());
        assert_eq!(tenant_paths.isolation().as_str(), paths.isolation.as_str());
        assert_eq!(tenant_paths.re_logs_dir(), paths.re_logs_dir());
        assert_eq!(tenant_paths.buck_out_dir(), paths.buck_out_dir());
        assert_eq!(tenant_paths.buck_out_path(), paths.buck_out_path());
        assert_eq!(tenant_paths.cache_dir(), paths.cache_dir());
        assert_eq!(
            tenant_paths.paranoid_cache_dir(),
            paths.paranoid_cache_dir()
        );
        assert_eq!(tenant_paths.cache_dir_path(), paths.cache_dir_path());
        assert_eq!(
            tenant_paths.materializer_state_path(),
            paths.materializer_state_path()
        );
        assert_eq!(
            tenant_paths.incremental_state_path(),
            paths.incremental_state_path()
        );
        assert_eq!(
            tenant_paths.dep_file_state_path(),
            paths.dep_file_state_path()
        );
        assert_eq!(tenant_paths.dice_state_path(), paths.dice_state_path());
        assert_eq!(tenant_paths.valid_cache_dirs(), paths.valid_cache_dirs());
    }
}
