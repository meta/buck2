/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
#[cfg(test)]
use std::process::Child;
use std::process::Command;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use dupe::Dupe;
use nix::fcntl::OFlag;
use nix::sys::stat::Mode;

use crate::OrphanProcessInfo;
use crate::cgroup_files::CgroupFile;
use crate::cgroup_files::CgroupFileMode;
use crate::cgroup_files::MemoryStat;
use crate::cgroup_files::ResourcePressure;
use crate::path::CgroupPath;
use crate::path::CgroupPathBuf;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Environment)]
enum CgroupError {
    #[error("{msg} IO error: {io_err}")]
    Io { msg: String, io_err: std::io::Error },
}

fn is_process_gone_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(nix::libc::ESRCH)
}

/// Resource constraints inherited from ancestor cgroups in the hierarchy.
///
/// For the various resource constraints that can be imposed on a cgroup, this struct represents the
/// effective limit that a particular cgroup sees from the combination of its parents in aggregate.
///
/// Note that in containerized environments, we may not be able to see the limits imposed on us.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EffectiveResourceConstraints {
    pub memory_max: Option<u64>,
    pub memory_high: Option<u64>,
    pub memory_swap_max: Option<u64>,
    pub memory_swap_high: Option<u64>,
}

/// A list of enabled controllers.
///
/// Each one is formatted as `+controller`, as that's a bit easier to use in practice.
#[derive(Dupe, Clone)]
pub(crate) struct EnabledControllers(Arc<[String]>);

impl EnabledControllers {
    pub(crate) fn contains(&self, controller: &str) -> bool {
        self.0.iter().any(|c| &c[1..] == controller)
    }
}

pub trait MemoryMonitoring {}

pub struct NoMemoryMonitoring;

impl MemoryMonitoring for NoMemoryMonitoring {}

pub struct WithMemoryMonitoring {
    memory_stat: CgroupFile,
    memory_current: CgroupFile,
    swap_current: CgroupFile,
    memory_pressure: CgroupFile,
}

impl MemoryMonitoring for WithMemoryMonitoring {}

/// The kind of node in the cgroup tree this cgroup is.
///
/// Cgroups come in two flavors: Internal cgroups, which are those that can have child groups but
/// cannot contain processes, and leaf cgroups, for which the opposite is the case.
pub trait CgroupKind {}

pub struct CgroupKindInternal {
    controllers: EnabledControllers,
}

impl CgroupKind for CgroupKindInternal {}

pub struct CgroupKindLeaf {
    // `cgroup.procs`
    procs: Arc<CgroupFile>,
}

impl CgroupKind for CgroupKindLeaf {}

/// Just after creating a cgroup we may not yet know what kind it is.
pub struct CgroupKindUndecided;

impl CgroupKind for CgroupKindUndecided {}

/// Handle to a cgroup
///
/// Generic over whether we are monitoring the memory use of the cgroup; having memory monitoring on
/// is relatively cheap but does require holding open a couple extra FDs.
pub struct Cgroup<M: MemoryMonitoring, K: CgroupKind> {
    /// Store the dirfd, not the more standard `DIR*`, because this one is thread safe
    dir: Arc<OwnedFd>,
    path: CgroupPathBuf,
    memory: M,
    kind: K,
}

pub type CgroupMinimal = Cgroup<NoMemoryMonitoring, CgroupKindUndecided>;

pub type CgroupLeaf = Cgroup<WithMemoryMonitoring, CgroupKindLeaf>;

pub type CgroupInternal = Cgroup<WithMemoryMonitoring, CgroupKindInternal>;

impl<M: MemoryMonitoring, K: CgroupKind> Cgroup<M, K> {
    pub fn path(&self) -> &CgroupPath {
        &self.path
    }

    pub(crate) fn dir_fd(&self) -> &OwnedFd {
        &self.dir
    }

    pub(crate) async fn read_enabled_controllers(&self) -> buck2_error::Result<EnabledControllers> {
        let controllers_file = CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.controllers"),
            CgroupFileMode::ReadOnly,
        )
        .await?;
        let controllers = controllers_file.read_to_string().await?;
        Ok(EnabledControllers(
            controllers
                .split_whitespace()
                .map(|s| format!("+{}", s))
                .collect(),
        ))
    }

    /// Set the memory.high limit for this cgroup
    pub async fn set_memory_high(&self, memory_high: &str) -> buck2_error::Result<()> {
        CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("memory.high"),
            CgroupFileMode::ReadWrite,
        )
        .await?
        .write(memory_high.to_owned())
        .await
    }

    /// Set the memory.max limit for this cgroup
    pub async fn set_memory_max(&self, memory_max: &str) -> buck2_error::Result<()> {
        CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("memory.max"),
            CgroupFileMode::ReadWrite,
        )
        .await?
        .write(memory_max.to_owned())
        .await
    }

    /// Set the memory.swap.max limit for this cgroup
    pub async fn set_memory_swap_max(&self, memory_swap_max: &str) -> buck2_error::Result<()> {
        CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("memory.swap.max"),
            CgroupFileMode::ReadWrite,
        )
        .await?
        .write(memory_swap_max.to_owned())
        .await
    }

    /// Set the cpuset.cpus value for this cgroup, restricting which CPU cores
    /// processes in this cgroup can run on.
    ///
    /// `value` is a comma/range list of CPU IDs (e.g., `"0-3"` or `"0,1,2,3"`).
    /// An empty string clears the value and the leaf inherits from its parent.
    pub async fn set_cpuset_cpus(&self, value: &str) -> buck2_error::Result<()> {
        CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cpuset.cpus"),
            CgroupFileMode::ReadWrite,
        )
        .await?
        .write(value.to_owned())
        .await
    }

    /// Setting this means that various OOM killer implementations will always kill the entire
    /// cgroup and all its children together, instead of just subgroups.
    ///
    /// In buck2 we want this because there's a risk that actions will not correctly report failures
    /// if sub-processes are getting killed.
    pub(crate) async fn set_memory_oom_group(&self) -> buck2_error::Result<()> {
        CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("memory.oom.group"),
            CgroupFileMode::ReadWrite,
        )
        .await?
        .write("1")
        .await
    }

    async fn read_resource_constraints(&self) -> buck2_error::Result<EffectiveResourceConstraints> {
        let read = |f| async move {
            let f = CgroupFile::open(
                self.dir.dupe(),
                FileNameBuf::unchecked_new(f),
                CgroupFileMode::ReadOnly,
            )
            .await?;
            buck2_error::Ok(f.read_max_or_int().await?)
        };
        let (memory_high, memory_max, memory_swap_high, memory_swap_max) = tokio::try_join!(
            read("memory.high"),
            read("memory.max"),
            read("memory.swap.high"),
            read("memory.swap.max"),
        )?;
        Ok(EffectiveResourceConstraints {
            memory_max,
            memory_high,
            memory_swap_max,
            memory_swap_high,
        })
    }

    pub(crate) async fn read_effective_resouce_constraints(
        &self,
    ) -> buck2_error::Result<EffectiveResourceConstraints> {
        let Some(parent_path) = self.path.parent() else {
            // The root cgroup doesn't have memory restrictions (the files don't even exist)
            return Ok(EffectiveResourceConstraints::default());
        };
        let (parent, me) = tokio::try_join!(
            async {
                Box::pin(
                    CgroupMinimal::try_from_path(parent_path.to_buf())
                        .await?
                        .read_effective_resouce_constraints(),
                )
                .await
            },
            self.read_resource_constraints()
        )?;

        fn min_options(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            a.into_iter().chain(b).reduce(std::cmp::min)
        }

        Ok(EffectiveResourceConstraints {
            memory_max: min_options(parent.memory_max, me.memory_max),
            memory_high: min_options(parent.memory_high, me.memory_high),
            memory_swap_max: min_options(parent.memory_swap_max, me.memory_swap_max),
            memory_swap_high: min_options(parent.memory_swap_high, me.memory_swap_high),
        })
    }

    /// For now, only available in tests.
    ///
    /// Before using outside of tests:
    ///  1. Write tests
    ///  2. Ensure that the pid controller is available
    ///  3. Consider holding the FD open
    #[cfg(test)]
    pub(crate) async fn read_pid_count(&self) -> buck2_error::Result<u64> {
        CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("pids.current"),
            CgroupFileMode::ReadOnly,
        )
        .await?
        .read_int()
        .await
    }

    /// Kill all remaining processes in the cgroup and return information about what was killed.
    ///
    /// The kill behavior is "race free," ie no risk of racing against forks or whatever. However, the output reporting is not race free.
    pub async fn kill_remaining_pids(&self) -> buck2_error::Result<Vec<OrphanProcessInfo>> {
        let procs = CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.procs"),
            CgroupFileMode::ReadOnly,
        )
        .await?;
        let procs_content = procs.read_to_string().await?;
        let pids: Vec<u32> = procs_content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.parse().ok())
            .collect();

        let orphans: Vec<OrphanProcessInfo> = pids
            .into_iter()
            .map(|pid| {
                let comm =
                    fs_util::read_to_string(AbsPath::new(&format!("/proc/{}/comm", pid)).unwrap())
                        .map(|s| s.trim().to_owned())
                        .unwrap_or_default();
                OrphanProcessInfo { pid, comm }
            })
            .collect();

        let f = CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.kill"),
            CgroupFileMode::WriteOnly,
        )
        .await?;
        // TODO: Is there something we ought to be doing to ensure this is
        // "completed" before reusing the cgroup?
        f.write("1").await?;

        Ok(orphans)
    }
}

impl Cgroup<NoMemoryMonitoring, CgroupKindUndecided> {
    pub(crate) fn sync_try_from_path(path: CgroupPathBuf) -> buck2_error::Result<Self> {
        let dir = nix::fcntl::open(
            path.as_path(),
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY,
            Mode::empty(),
        )
        .map_err(|e| CgroupError::Io {
            msg: format!("Failed to open cgroup directory: {path:?}"),
            io_err: e.into(),
        })?;

        let cgroup = Self {
            path,
            dir: Arc::new(dir),
            memory: NoMemoryMonitoring,
            kind: CgroupKindUndecided,
        };

        Ok(cgroup)
    }

    pub async fn try_from_path(path: CgroupPathBuf) -> buck2_error::Result<Self> {
        tokio::task::spawn_blocking(|| Self::sync_try_from_path(path)).await?
    }

    /// Treat this cgroup as a leaf cgroup
    pub async fn into_leaf(
        self,
    ) -> buck2_error::Result<Cgroup<NoMemoryMonitoring, CgroupKindLeaf>> {
        Ok(Cgroup {
            kind: CgroupKindLeaf {
                procs: Arc::new(
                    CgroupFile::open(
                        self.dir.dupe(),
                        FileNameBuf::unchecked_new("cgroup.procs"),
                        CgroupFileMode::ReadWrite,
                    )
                    .await?,
                ),
            },
            dir: self.dir,
            path: self.path,
            memory: self.memory,
        })
    }

    /// Move all processes in this cgroup (other than the current process) into `dest`, which may be
    /// any cgroup, not necessarily a direct child of this one.
    ///
    /// This is needed before enabling subtree control, because cgroupv2 enforces a
    /// no-internal-process constraint: a cgroup cannot have both processes in cgroup.procs
    /// AND controllers enabled in cgroup.subtree_control.
    ///
    /// Processes may end up in this cgroup if they were spawned before prep_current_process()
    /// moved the daemon into its own cgroup
    pub(crate) async fn drain_to(
        &self,
        dest: &CgroupMinimal,
    ) -> buck2_error::Result<Vec<OrphanProcessInfo>> {
        let procs = CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.procs"),
            CgroupFileMode::ReadOnly,
        )
        .await?;
        let procs_content = procs.read_to_string().await?;
        let my_pid = std::process::id();
        let pids: Vec<u32> = procs_content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.parse().ok())
            .filter(|pid| *pid != my_pid)
            .collect();

        if pids.is_empty() {
            return Ok(Vec::new());
        }

        let child_procs = CgroupFile::open(
            dest.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.procs"),
            CgroupFileMode::ReadWrite,
        )
        .await?;

        let orphans = tokio::task::spawn_blocking(move || {
            let mut orphans = Vec::new();
            for pid in pids {
                match child_procs.sync_write(pid.to_string().as_bytes()) {
                    Ok(()) => {
                        let comm = fs_util::read_to_string(
                            AbsPath::new(&format!("/proc/{}/comm", pid)).unwrap(),
                        )
                        .map(|s| s.trim().to_owned())
                        .unwrap_or_default();
                        orphans.push(OrphanProcessInfo { pid, comm });
                    }
                    // Ignore the expected race where a process exits after we read cgroup.procs but
                    // before we move it.
                    Err(e) if is_process_gone_error(&e) => {}
                    Err(e) => {
                        return Err(buck2_error::Error::from(e)
                            .context(format!("Writing cgroup file cgroup.procs for pid {pid}")));
                    }
                }
            }
            buck2_error::Ok(orphans)
        })
        .await??;

        Ok(orphans)
    }

    /// Enable subtree controllers on this cgroup as specified and convert to an internal cgroup
    pub(crate) async fn enable_subtree_control_and_into_internal(
        self,
        enabled_controllers: EnabledControllers,
    ) -> buck2_error::Result<Cgroup<NoMemoryMonitoring, CgroupKindInternal>> {
        let subtree_control = CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.subtree_control"),
            CgroupFileMode::ReadWrite,
        )
        .await?;
        for controller in &*enabled_controllers.0 {
            subtree_control.write(controller.clone()).await?;
        }

        Ok(self.into_internal_with_subtree_control_already_enabled(enabled_controllers))
    }

    pub(crate) fn into_internal_with_subtree_control_already_enabled(
        self,
        enabled_controllers: EnabledControllers,
    ) -> Cgroup<NoMemoryMonitoring, CgroupKindInternal> {
        Cgroup {
            dir: self.dir,
            path: self.path,
            memory: self.memory,
            kind: CgroupKindInternal {
                controllers: enabled_controllers,
            },
        }
    }
}

impl<M: MemoryMonitoring> Cgroup<M, CgroupKindLeaf> {
    /// Configures a Command to run in this cgroup using pre_exec.
    ///
    /// Uses pre_exec hook which runs in the child process after fork() but before exec().
    /// This ensures the process is migrated to the cgroup before it starts executing.
    ///
    /// Parent process: Command new() -> ... -> spwan()
    ///                  ↓
    /// System: fork() creates child process
    ///                  ↓
    /// Child process: pre_exec() closure runs (cgroup migration)
    ///                  ↓
    /// Child process: exec() run the command
    pub fn setup_command(&self, command: &mut Command) {
        // NOTE: We need to make sure that this fd is kept alive until the point where the command
        // is spawned/forked. The lifetimes on this type don't prevent the user from dropping the
        // `Cgroup` before that happens.
        //
        // This is accomplished by capturing the `Arc<CgroupFile>` into the `pre_exec` closure. The
        // closure is not dropped until the surrounding command is dropped, at which point the spawn
        // must have happened.
        //
        // Some testing shows that the closure is not dropped in the child process at all; that's
        // good, as we don't actually want to do the `free` that may be implicated by dropping the
        // `Arc`
        let procs = self.kind.procs.dupe();

        // 0 means current process
        let pre_exec = move || procs.sync_write(b"0");
        // Safety: The unsafe block is required for pre_exec which is inherently unsafe due to fork/exec restrictions.
        // However, it's safe here because:
        // 1. We only call async-signal-safe functions (write to file)
        // 2. No memory allocation or complex operations that could deadlock
        unsafe {
            command.pre_exec(pre_exec);
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_use_some_memory_process(
        &self,
        allocate_count: u64,
        each_tick_allocate_memory_mbs: f64,
        tick_duration: Option<Duration>,
        pre_exit_sleep_duration: Option<Duration>,
    ) -> buck2_error::Result<Child> {
        use std::process::Stdio;

        use buck2_util::process::background_command;

        let bin = std::env::var("USE_SOME_MEMORY_BIN").unwrap();
        let mut cmd = background_command(bin);
        self.setup_command(&mut cmd);
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd.arg("--allocate-count")
            .arg(allocate_count.to_string())
            .arg("--each-tick-allocate-memory")
            .arg(each_tick_allocate_memory_mbs.to_string());
        if let Some(tick_duration) = tick_duration {
            cmd.arg("--tick-duration")
                .arg(tick_duration.as_secs_f64().to_string());
        }
        if let Some(pre_exit_sleep_duration) = pre_exit_sleep_duration {
            cmd.arg("--pre-exit-sleep-duration")
                .arg(pre_exit_sleep_duration.as_secs_f64().to_string());
        }
        Ok(cmd.spawn()?)
    }
}

impl<M: MemoryMonitoring, K: CgroupKind> Cgroup<M, K> {
    fn sync_discouraged_make_child_impl(
        dir: &OwnedFd,
        path: &CgroupPath,
        child: &FileName,
    ) -> buck2_error::Result<CgroupMinimal> {
        nix::sys::stat::mkdirat(dir, child.as_str(), Mode::all())?;

        let fd = nix::fcntl::openat(
            dir,
            child.as_str(),
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY,
            Mode::empty(),
        )?;

        Ok(CgroupMinimal {
            dir: Arc::new(fd),
            path: path.join(child.as_forward_rel_path()),
            memory: NoMemoryMonitoring,
            kind: CgroupKindUndecided,
        })
    }

    pub(crate) fn sync_discouraged_make_child(
        &self,
        child: &FileName,
    ) -> buck2_error::Result<CgroupMinimal> {
        Self::sync_discouraged_make_child_impl(&self.dir, &self.path, child)
    }

    /// Make a child cgroup
    ///
    /// Use strongly discouraged; almost always use `make_internal_child` or `make_leaf_child` instead
    pub(crate) async fn discouraged_make_child(
        &self,
        child: FileNameBuf,
    ) -> buck2_error::Result<CgroupMinimal> {
        let dir = self.dir.dupe();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            Self::sync_discouraged_make_child_impl(&dir, &path, &child)
        })
        .await?
    }
}

impl<M: MemoryMonitoring> Cgroup<M, CgroupKindInternal> {
    /// Make a child cgroup
    ///
    /// Unlike above, not discouraged because we know this to be an internal cgroup
    pub(crate) async fn make_child(
        &self,
        child: FileNameBuf,
    ) -> buck2_error::Result<CgroupMinimal> {
        self.discouraged_make_child(child).await
    }

    pub(crate) async fn make_internal_child(
        &self,
        child: FileNameBuf,
    ) -> buck2_error::Result<Cgroup<NoMemoryMonitoring, CgroupKindInternal>> {
        let c = self.make_child(child).await?;
        c.enable_subtree_control_and_into_internal(self.kind.controllers.dupe())
            .await
    }

    pub(crate) async fn make_leaf_child(
        &self,
        child: FileNameBuf,
    ) -> buck2_error::Result<Cgroup<NoMemoryMonitoring, CgroupKindLeaf>> {
        let c = self.make_child(child).await?;
        c.into_leaf().await
    }
}

impl<K: CgroupKind> Cgroup<NoMemoryMonitoring, K> {
    pub(crate) async fn enable_memory_monitoring(
        self,
    ) -> buck2_error::Result<Cgroup<WithMemoryMonitoring, K>> {
        let open = |f| {
            let d = &self.dir;
            async move {
                CgroupFile::open(
                    d.dupe(),
                    FileNameBuf::unchecked_new(f),
                    CgroupFileMode::ReadOnly,
                )
                .await
            }
        };
        let (memory_stat, memory_current, memory_swap_current, memory_pressure) = tokio::try_join!(
            open("memory.stat"),
            open("memory.current"),
            open("memory.swap.current"),
            open("memory.pressure"),
        )?;
        Ok(Cgroup {
            memory: WithMemoryMonitoring {
                memory_stat,
                memory_current,
                swap_current: memory_swap_current,
                memory_pressure,
            },
            dir: self.dir,
            path: self.path,
            kind: self.kind,
        })
    }
}

impl<K: CgroupKind> Cgroup<WithMemoryMonitoring, K> {
    pub async fn read_memory_stat(&self) -> buck2_error::Result<MemoryStat> {
        self.memory.memory_stat.read_memory_stat().await
    }

    pub async fn read_memory_current(&self) -> buck2_error::Result<u64> {
        self.memory.memory_current.read_int().await
    }

    pub async fn read_swap_current(&self) -> buck2_error::Result<u64> {
        self.memory.swap_current.read_int().await
    }

    /// Reads the memory pressure of this cgroup
    ///
    /// Memory pressure is not a point in time value - it's always an average over some time period.
    /// For a given handle, this function returns 0 the first time it's called, and on subsequent
    /// calls returns the average memory pressure since the last call using the same handle.
    pub async fn read_memory_pressure_total(
        &self,
        handle: &mut MemoryPressureHandle,
    ) -> buck2_error::Result<f64> {
        let before = Instant::now();
        let new_total = self
            .memory
            .memory_pressure
            .read_resource_pressure()
            .await?
            .full
            .total;
        let after = Instant::now();

        let pressure = match handle.time {
            Some(last_time) => {
                // The provided value is in microseconds of stall time, we convert to a percentage-like unit
                let delta = new_total - handle.total;
                // Note: The time interval we use is from before the last measurement to after the
                // current measurment. This causes us to overcount some time, but in exchange means
                // that the time interval is strictly longer than the "true" one. That way if for
                // some reason something stalls or this takes a long time, we don't accidentally
                // divide by a too short interval and report a vary large value. It's also for this
                // reason that it's quite important that we measure the time here in this function,
                // and don't use some higher-level current time
                //
                // Ideally, we'd check if (after - before) is greater than some reasonable value for
                // how long this should take and then log a warning or something. In practice
                // though, that would only happen in cases where resource contention is already
                // extremely high, and we don't appear to actually be capable of emitting logs at
                // that time.
                //
                // Note also that none of the above is theoretical, this is based on data from
                // actual prod runs.
                let delta_time = after.duration_since(last_time);
                let delta_time_us = delta_time.as_secs_f64() * 1_000_000f64;
                100.0 * (delta as f64) / delta_time_us
            }
            None => 0.0f64,
        };
        handle.time = Some(before);
        handle.total = new_total;
        Ok(pressure)
    }

    pub async fn read_memory_pressure(&self) -> buck2_error::Result<ResourcePressure> {
        self.memory.memory_pressure.read_resource_pressure().await
    }
}

pub struct MemoryPressureHandle {
    time: Option<Instant>,
    total: u64,
}

impl MemoryPressureHandle {
    pub fn new() -> Self {
        Self {
            time: None,
            total: 0,
        }
    }
}

pub struct CgroupFreezeGuard {
    file: CgroupFile,
}

impl Drop for CgroupFreezeGuard {
    fn drop(&mut self) {
        drop(self.file.sync_write(b"0"));
    }
}

impl<M: MemoryMonitoring, K: CgroupKind> Cgroup<M, K> {
    pub async fn freeze(&self) -> buck2_error::Result<CgroupFreezeGuard> {
        let f = CgroupFile::open(
            self.dir.dupe(),
            FileNameBuf::unchecked_new("cgroup.freeze"),
            CgroupFileMode::ReadWrite,
        )
        .await?;
        f.write(b"1").await?;
        Ok(CgroupFreezeGuard { file: f })
    }
}

#[cfg(test)]
impl CgroupMinimal {
    pub(crate) async fn create_minimal_for_test() -> Option<Self> {
        use buck2_fs::paths::abs_norm_path::AbsNormPath;
        use buck2_util::process::background_command;

        if !cfg!(buck_build) || !cfg!(target_os = "linux") {
            return None;
        }

        // Skip if `systemd-run --user --slice-inherit` doesn't work here (e.g. CI has no
        // user systemd session). `prep_cgroup.sh` runs the same command and would panic.
        let probe_ok = background_command("systemd-run")
            .args([
                "--user",
                "--slice-inherit",
                "--scope",
                "--quiet",
                "--",
                "/bin/true",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !probe_ok {
            return None;
        }

        let prep_script = std::env::var("PREP_CGROUP_SCRIPT").unwrap();
        let output = background_command(&prep_script)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "PREP_CGROUP_SCRIPT failed with status {}.\nStderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
        let path = String::from_utf8(output.stdout).unwrap();
        let path = path.trim();
        assert!(
            !path.is_empty(),
            "PREP_CGROUP_SCRIPT produced no output on stdout",
        );
        let path = CgroupPath::new(AbsNormPath::new(path).unwrap());

        // Attempt to actually spawn a process into the cgroup, see below for why
        let cgroup = Self::try_from_path(path.to_buf())
            .await
            .unwrap()
            .into_leaf()
            .await
            .unwrap();
        let mut cmd = background_command("true");
        cgroup.setup_command(&mut cmd);
        let res = cmd.status();
        drop(cgroup);

        // At this point the one process in that cgroup is dead, so it's ok to treat it as a minimal
        // one again
        let cgroup = Self::try_from_path(path.to_buf()).await.unwrap();
        let controllers = cgroup.read_enabled_controllers().await.unwrap();
        let parent = cgroup
            .enable_subtree_control_and_into_internal(controllers)
            .await
            .unwrap();

        match res {
            Ok(_) => {
                // Use a fresh child cgroup - we don't want to use the one above since it has some
                // leftover metrics corresponding to the process we just ran in it
                let child = parent
                    .discouraged_make_child(FileNameBuf::unchecked_new("test_group"))
                    .await
                    .unwrap();
                Some(child)
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // The script above generates a cgroup for us to use by starting a new systemd user
                // service. However, depending on the cgroup in which *this test* is running, we may
                // not be able to spawn or move any processes into there. Practically, this actually
                // depends on whether the buck that is running this test has resource control
                // enabled, since that effects the cgroup this test runs in.
                //
                // In case where that's an issue, work around it by attempting to use sudo perms to
                // move ourselves somewhere from where we can do what we need. This unfortunately
                // means that attempting to run multiple tests like this in the same process won't
                // work, but alas this is the best we can do
                let leaf = parent
                    .make_child(FileNameBuf::unchecked_new("_buck_leaf"))
                    .await
                    .unwrap();
                // Move ourselves into the cgroup we just created
                let cmd = format!(
                    "echo {} | sudo tee {}/cgroup.procs",
                    std::process::id(),
                    leaf.path(),
                );
                assert!(
                    background_command("bash")
                        .arg("-c")
                        .arg(cmd)
                        .status()
                        .unwrap()
                        .success()
                );
                Some(
                    parent
                        .make_child(FileNameBuf::unchecked_new("test_group"))
                        .await
                        .unwrap(),
                )
            }
            Err(e) => panic!("Failed to open cgroup.procs: {e:?}"),
        }
    }
}

#[cfg(test)]
impl Cgroup<NoMemoryMonitoring, CgroupKindLeaf> {
    pub(crate) async fn create_leaf_for_test() -> Option<Self> {
        Some(
            Cgroup::create_minimal_for_test()
                .await?
                .into_leaf()
                .await
                .unwrap(),
        )
    }
}

#[cfg(test)]
impl Cgroup<NoMemoryMonitoring, CgroupKindInternal> {
    pub(crate) async fn create_internal_for_test() -> Option<Self> {
        let cgroup = Cgroup::create_minimal_for_test().await?;
        let controllers = cgroup.read_enabled_controllers().await.unwrap();
        Some(
            cgroup
                .enable_subtree_control_and_into_internal(controllers)
                .await
                .unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    use std::time::Instant;

    use buck2_fs::fs_util::uncategorized as fs_util;
    use buck2_fs::paths::file_name::FileNameBuf;
    use buck2_util::process::background_command;
    use dupe::Dupe;

    use super::is_process_gone_error;
    use crate::cgroup::Cgroup;
    use crate::cgroup::MemoryPressureHandle;
    use crate::cgroup_files::CgroupFile;
    use crate::cgroup_files::CgroupFileMode;

    #[test]
    fn test_is_process_gone_error() {
        assert!(is_process_gone_error(&std::io::Error::from_raw_os_error(
            nix::libc::ESRCH,
        )));
        assert!(!is_process_gone_error(&std::io::Error::from_raw_os_error(
            nix::libc::EBUSY,
        )));
        assert!(!is_process_gone_error(&std::io::Error::from_raw_os_error(
            nix::libc::EPERM,
        )));
    }

    #[tokio::test]
    async fn self_test_harness() {
        let Some(cgroup) = Cgroup::create_leaf_for_test().await else {
            return;
        };

        assert!(fs_util::try_exists(cgroup.path().as_abs_path()).unwrap());
        assert_eq!(cgroup.kind.procs.read_to_string().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn repro_drop_cgroup_before_command_spawn() {
        let Some(cgroup) = Cgroup::create_leaf_for_test().await else {
            return;
        };

        let mut cmd = background_command("true");
        cgroup.setup_command(&mut cmd);

        drop(cgroup);

        assert!(cmd.status().unwrap().success());
    }

    #[tokio::test]
    async fn test_cpu_controller_enabled() {
        // FIXME(JakobDegen): This isn't really a good test, we should do it at a higher level and
        // actually run a command. But setting up tests inside cgroups is a bit hard right now, so
        // just do this
        let Some(cgroup) = Cgroup::create_internal_for_test().await else {
            return;
        };
        let leaf = cgroup
            .make_leaf_child(FileNameBuf::unchecked_new("leaf"))
            .await
            .unwrap();

        let enabled =
            fs_util::read_to_string(leaf.path().as_abs_path().join("cgroup.controllers")).unwrap();
        assert!(enabled.contains("memory"));
        assert!(enabled.contains("cpu"));
    }

    #[tokio::test]
    async fn test_reading_effective_cgroup_constraints() {
        let Some(cgroup) = Cgroup::create_internal_for_test().await else {
            return;
        };

        fn pages(b: u64) -> String {
            (b * 4096).to_string()
        }

        let subg1 = cgroup
            .make_internal_child(FileNameBuf::unchecked_new("subg1"))
            .await
            .unwrap();
        CgroupFile::open(
            subg1.dir.dupe(),
            FileNameBuf::unchecked_new("memory.high"),
            CgroupFileMode::ReadWrite,
        )
        .await
        .unwrap()
        .write(pages(1000))
        .await
        .unwrap();
        CgroupFile::open(
            subg1.dir.dupe(),
            FileNameBuf::unchecked_new("memory.max"),
            CgroupFileMode::ReadWrite,
        )
        .await
        .unwrap()
        .write(pages(1000))
        .await
        .unwrap();

        let subg2 = subg1
            .make_internal_child(FileNameBuf::unchecked_new("subg2"))
            .await
            .unwrap();
        CgroupFile::open(
            subg2.dir.dupe(),
            FileNameBuf::unchecked_new("memory.high"),
            CgroupFileMode::ReadWrite,
        )
        .await
        .unwrap()
        .write(pages(500))
        .await
        .unwrap();

        let constraints = subg2.read_effective_resouce_constraints().await.unwrap();
        assert_eq!(constraints.memory_high, Some(500 * 4096));
        assert_eq!(constraints.memory_max, Some(1000 * 4096));

        // Depending on where this test runs, these might be non-zero and we can't really do much
        // about that, so just do our best
        let base_constraints = cgroup.read_effective_resouce_constraints().await.unwrap();
        assert_eq!(
            constraints.memory_swap_high,
            base_constraints.memory_swap_high
        );
        assert_eq!(
            constraints.memory_swap_max,
            base_constraints.memory_swap_max
        );
    }

    #[tokio::test]
    async fn test_read_memory_metrics() {
        let Some(cgroup) = Cgroup::create_leaf_for_test().await else {
            return;
        };
        let cgroup = cgroup.enable_memory_monitoring().await.unwrap();
        let mut pressure_handle = MemoryPressureHandle::new();
        assert_eq!(
            cgroup
                .read_memory_pressure_total(&mut pressure_handle)
                .await
                .unwrap(),
            0.0
        );

        let stat = cgroup.read_memory_stat().await.unwrap();
        // Never spawned a process into it, so should be empty
        assert_eq!(stat.anon, 0);
        assert_eq!(stat.file, 0);
        assert_eq!(cgroup.read_memory_current().await.unwrap(), 0);
        assert_eq!(cgroup.read_swap_current().await.unwrap(), 0);
        assert_eq!(
            cgroup
                .read_memory_pressure_total(&mut pressure_handle)
                .await
                .unwrap(),
            0.0
        );

        cgroup.set_memory_high("19000000").await.unwrap();
        let mut child = cgroup
            .spawn_use_some_memory_process(1, 20.0, None, Some(Duration::from_secs(100)))
            .unwrap();

        // Above `memory.high` the kernel reclaims the process's pages to swap, and how far it
        // gets depends on the host, so check the footprint across memory and swap. Wait for it
        // rather than sleeping a fixed time: the process starts and allocates asynchronously.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let stat = cgroup.read_memory_stat().await.unwrap();
            let memory_current = cgroup.read_memory_current().await.unwrap();
            let swap_current = cgroup.read_swap_current().await.unwrap();
            if stat.anon + swap_current > 15000000
                && memory_current + swap_current > 15000000
                && swap_current > 500000
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{stat:?}, memory.current {memory_current}, memory.swap.current {swap_current}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let memory_pressure = cgroup
            .read_memory_pressure_total(&mut pressure_handle)
            .await
            .unwrap();
        let check_memory_pressure;
        #[cfg(fbcode_build)]
        {
            if environment::is_on_demand() {
                // In OD environments, memory pressure may be lower due to different cgroup configurations
                // or resource constraints, so skip this assertion there.
                check_memory_pressure = false;
            } else {
                check_memory_pressure = true;
            }
        }
        #[cfg(not(fbcode_build))]
        {
            check_memory_pressure = true;
        }
        if check_memory_pressure {
            assert!(memory_pressure > 20.0, "{:?}", memory_pressure);
        }
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn test_drain_to_child() {
        let Some(cgroup) = Cgroup::create_minimal_for_test().await else {
            return;
        };
        let controllers = cgroup.read_enabled_controllers().await.unwrap();
        let parent = cgroup
            .enable_subtree_control_and_into_internal(controllers)
            .await
            .unwrap();

        let source = parent
            .make_child(FileNameBuf::unchecked_new("source"))
            .await
            .unwrap();
        let dest = parent
            .make_child(FileNameBuf::unchecked_new("dest"))
            .await
            .unwrap();

        // Empty cgroup should drain nothing
        let orphans = source.drain_to(&dest).await.unwrap();
        assert!(orphans.is_empty());

        // Spawn a process into source via the filesystem
        let mut cmd = background_command("sleep");
        cmd.arg("300");
        let mut child = cmd.spawn().unwrap();
        let child_pid = child.id();
        let source_procs_path = source.path().as_abs_path().join("cgroup.procs");
        std::fs::write(&source_procs_path, child_pid.to_string()).unwrap();

        assert_eq!(source.read_pid_count().await.unwrap(), 1);
        assert_eq!(dest.read_pid_count().await.unwrap(), 0);

        // Drain should move the process to dest
        let orphans = source.drain_to(&dest).await.unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].pid, child_pid);
        assert_eq!(orphans[0].comm, "sleep");

        assert_eq!(source.read_pid_count().await.unwrap(), 0);
        assert_eq!(dest.read_pid_count().await.unwrap(), 1);

        // Draining again should be a no-op
        let orphans = source.drain_to(&dest).await.unwrap();
        assert!(orphans.is_empty());

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn test_kill_remaining_pids() {
        let Some(cgroup) = Cgroup::create_leaf_for_test().await else {
            return;
        };

        // Empty cgroup should have no PIDs
        assert_eq!(cgroup.read_pid_count().await.unwrap(), 0);

        // Spawn a long-running process into the cgroup
        let mut cmd = background_command("sleep");
        cmd.arg("300");
        cgroup.setup_command(&mut cmd);
        let mut child = cmd.spawn().unwrap();

        assert_eq!(cgroup.read_pid_count().await.unwrap(), 1);

        let orphans = cgroup.kill_remaining_pids().await.unwrap();
        assert!(!orphans.is_empty(), "Should have found orphan processes");
        assert_eq!(orphans[0].pid, child.id());
        assert_eq!(orphans[0].comm, "sleep");

        // Verify the process was killed by SIGKILL (signal 9) from cgroup.kill.
        // sleep 300 wouldn't exit on its own and we never called child.kill(),
        // so this confirms kill_remaining_pids actually worked.
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert_eq!(status.signal(), Some(9));

        // Killing again should report no remaining PIDs.
        assert!(cgroup.kill_remaining_pids().await.unwrap().is_empty());
    }
}
