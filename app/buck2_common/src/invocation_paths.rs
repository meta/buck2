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
use std::time::Duration;

use allocative::Allocative;
use buck2_core::buck2_env;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
pub use buck2_wrapper_common::DEFAULT_ISOLATION_DIR;
use buck2_wrapper_common::kill::process_exists;
use buck2_wrapper_common::pid::Pid;

use crate::daemon_dir::DaemonDir;
use crate::invocation_roots::InvocationRoots;

#[derive(Clone, Allocative)]
pub struct InvocationPaths {
    pub roots: InvocationRoots,

    /// The isolation dir is a relative path used to create unique directories for
    /// a tenant's on-disk state. This allows multiple isolated tenants to use the
    /// same project root.
    ///
    /// With `BUCK2_SHARE_DAEMON=true`, daemon metadata is shared across isolation directories for
    /// the same project. `BUCK2_DAEMON_GROUP` also shares metadata across project roots in that
    /// group. The tenant's buck-out remains `<projectroot>/buck-out/<isolationdir>/`.
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
        let paths = InvocationPaths {
            roots: InvocationRoots {
                project_root: self.project_root.clone(),
                cwd: ProjectRelativePath::empty().to_buf(),
            },
            isolation: self.isolation.clone(),
        };
        Ok(paths
            .project_daemon_dir()?
            .path
            .join(FileName::unchecked_new("._buck2_tenant_locks"))
            .join(&self.isolation))
    }

    /// The daemon directory used by clients without shared daemon routing.
    pub fn unshared_daemon_dir(&self) -> buck2_error::Result<DaemonDir> {
        let paths = InvocationPaths {
            roots: InvocationRoots {
                project_root: self.project_root.clone(),
                cwd: ProjectRelativePath::empty().to_buf(),
            },
            isolation: self.isolation.clone(),
        };
        Ok(DaemonDir {
            path: paths.project_daemon_dir()?.path.join(&self.isolation),
        })
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

/// Exclusive disk ownership shared by tenant generations and cleanup clients.
#[derive(Allocative)]
pub struct TenantDiskLock {
    #[allocative(skip)]
    _file: File,
    #[allocative(skip)]
    _legacy_lifecycle: Option<File>,
}

impl TenantDiskLock {
    /// Waits for disk ownership without interrupting the existing owner.
    pub async fn acquire(paths: &TenantPaths, timeout: Duration) -> buck2_error::Result<Self> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(mut lock) = Self::try_acquire(paths).await? {
                    if buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
                        let legacy_dir = paths.unshared_daemon_dir()?;
                        let Some(file) = Self::try_lock_file(&legacy_dir.path.join(FileName::unchecked_new("buckd.lifecycle"))).await? else {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            continue;
                        };
                        lock._legacy_lifecycle = Some(file);
                        match tokio::fs::read_to_string(legacy_dir.buckd_pid()).await {
                            Ok(pid) => {
                                let trimmed = pid.trim();
                                let raw: i64 = trimmed.parse().buck_error_context(format!(
                                    "Cannot verify legacy daemon ownership at `{legacy_dir}`; refusing shared initialization. Garbage `buckd.pid` does not prove ownership ended. Stop the legacy daemon and restore a consistent `buckd.pid` before sharing."
                                ))?;
                                let legacy_pid = Pid::from_i64(raw).buck_error_context(format!(
                                    "Cannot verify legacy daemon ownership at `{legacy_dir}`; refusing shared initialization. Invalid PID `{trimmed}` does not prove ownership ended. Stop the legacy daemon and restore a consistent `buckd.pid` before sharing."
                                ))?;
                                if process_exists(legacy_pid)? {
                                    return Err(buck2_error::buck2_error!(
                                        buck2_error::ErrorTag::Input,
                                        "Tenant disk state is already owned by a legacy daemon at `{legacy_dir}`. Stop it before enabling daemon sharing."
                                    ));
                                }
                            }
                            Err(error) if error.kind() == ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    break buck2_error::Ok(lock);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_elapsed| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::BuckdLifecycleLock,
                "Timed out waiting for exclusive use of `{}`: another process still owns it, either a Buck2 daemon or a running `buck2 clean`. Wait for it to finish, or stop the daemon (for example with `buck2 killall`), then retry.",
                paths.buck_out_path()
            )
        })?
    }

    async fn try_acquire(paths: &TenantPaths) -> buck2_error::Result<Option<Self>> {
        Ok(Self::try_lock_file(&paths.disk_ownership_lock_path()?)
            .await?
            .map(|file| Self {
                _file: file,
                _legacy_lifecycle: None,
            }))
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

impl InvocationPaths {
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

    pub fn daemon_dir(&self) -> buck2_error::Result<DaemonDir> {
        if let Some(group) = self.shared_daemon_group()? {
            return Ok(DaemonDir {
                path: self
                    .roots
                    .common_buckd_dir()?
                    .join(FileName::unchecked_new(".groups"))
                    .join(group)
                    .join(FileName::unchecked_new(DEFAULT_ISOLATION_DIR)),
            });
        }
        let isolation = if buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
            FileName::unchecked_new("._buck2_shared")
        } else {
            &self.isolation
        };
        Ok(DaemonDir {
            path: self.project_daemon_dir()?.path.join(isolation),
        })
    }

    pub fn shared_daemon_group(&self) -> buck2_error::Result<Option<FileNameBuf>> {
        // Groups only exist among shared daemons; without sharing the variable is ignored.
        if !buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
            return Ok(None);
        }
        let Some(group) = buck2_env!("BUCK2_DAEMON_GROUP")? else {
            return Ok(None);
        };
        Ok(Some(FileName::new(group)?.to_owned()))
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
        Ok(
            if buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
                self.daemon_state_dir()
            } else {
                self.buck_out_path()
            },
        )
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

    /// Private executable state for one forkserver, under execution-allowlisted buck-out.
    /// It is a sibling of the daemon's working directory so that `buck2 clean` of a shared
    /// tenant leaves the daemon-wide forkserver intact.
    pub fn forkserver_state_dir(&self, daemon_id: &str) -> buck2_error::Result<AbsNormPathBuf> {
        Ok(self
            .daemon_working_dir()?
            .join(FileName::unchecked_new("forkserver"))
            .join(FileName::new(daemon_id)?))
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

    use crate::invocation_paths::InvocationPaths;
    use crate::invocation_roots::InvocationRoots;

    #[test]
    fn test_paths() {
        let project_root = if cfg!(windows) {
            "C:\\my\\project"
        } else {
            "/my/project"
        };
        let paths = InvocationPaths {
            roots: InvocationRoots {
                project_root: ProjectRoot::new_unchecked(
                    AbsNormPathBuf::try_from(project_root.to_owned()).unwrap(),
                ),
                cwd: ProjectRelativePath::empty().to_buf(),
            },
            isolation: FileNameBuf::unchecked_new("isolation"),
        };

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

        let other_paths = InvocationPaths {
            roots: paths.roots.clone(),
            isolation: FileNameBuf::unchecked_new("other-isolation"),
        };
        assert_ne!(
            paths.daemon_dir().unwrap().path,
            other_paths.daemon_dir().unwrap().path,
        );
        assert_ne!(paths.buck_out_path(), other_paths.buck_out_path());

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
