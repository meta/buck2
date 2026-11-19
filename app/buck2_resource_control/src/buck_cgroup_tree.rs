/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::init::ResourceControlConfig;
use buck2_core::soft_error;
use buck2_error::buck2_error;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use dupe::Dupe;

use crate::cgroup::CgroupInternal;
use crate::cgroup::CgroupLeaf;
use crate::cgroup::CgroupMinimal;
use crate::cgroup::EffectiveResourceConstraints;
use crate::cgroup_files::CgroupFile;
use crate::cgroup_files::CgroupFileMode;
use crate::path::CgroupPathBuf;

#[derive(buck2_error::Error)]
#[buck2(tag = Input)]
enum CgroupConfigParsingError {
    #[error("Not a percentage: {0}")]
    NotAPercentage(String),
    #[error("Expected an integer or percentage: {0}")]
    NotAMemoryConfig(String),
}

#[derive(buck2_error::Error)]
#[buck2(tag = Input)]
enum CgroupParsingError {
    #[error("Process appears to not be a part of a cgroupv2 hierarchy: {0}")]
    NoCgroupV2Membership(String),
}

pub(crate) fn parse_procfs_cgroup_output(out: &str) -> buck2_error::Result<CgroupPathBuf> {
    fn find_v2(out: &str) -> Option<&str> {
        // Filter out any membership in v1 hierarchies
        let mut filt = out.lines().filter_map(|l| l.strip_prefix("0::"));
        let cgroup_v2 = filt.next()?;
        if filt.next().is_some() {
            return None;
        }
        Some(cgroup_v2)
    }

    let Some(cgroup) = find_v2(out) else {
        return Err(CgroupParsingError::NoCgroupV2Membership(out.trim().to_owned()).into());
    };
    Ok(CgroupPathBuf::new_in_cgroup_fs(AbsNormPath::new(cgroup)?))
}

/// Read this process's cgroup v2 path from `/proc/self/cgroup` for best-effort logging.
///
/// Returns `None` if procfs cannot be read or if the process is not in exactly one cgroup v2
/// hierarchy. This intentionally uses synchronous I/O because it reads a small local procfs file
/// during daemon startup/logging, where a best-effort value is sufficient.
pub fn read_current_cgroup() -> Option<String> {
    let procfs_out = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    parse_procfs_cgroup_output(&procfs_out)
        .ok()
        .map(|p| p.to_string())
}

/// Read the cgroup path of the buck2 daemon process based on its pid from the client side
pub fn read_cgroup_path_of_buck2_daemon(daemon_pid: i64) -> buck2_error::Result<Option<String>> {
    let path = format!("/proc/{}/cgroup", daemon_pid);
    let procfs_out = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let cgroup_path = parse_procfs_cgroup_output(&procfs_out)?;
    Ok(Some(cgroup_path.to_string()))
}

pub struct PreppedBuckCgroups {
    /// The cgroup the daemon was launched in.
    ///
    /// Whoever launched the daemon owns this cgroup's attributes. When that's systemd, it rewrites
    /// them from its own configuration of the unit whenever it reapplies that configuration, for
    /// example on `daemon-reload`. So the only thing buck ever writes here is
    /// `cgroup.subtree_control`, which cgroup delegation requires the delegatee to do; everything
    /// else, including daemon-wide resource limits, goes on `allprocs` or below.
    launch: CgroupMinimal,
    allprocs: CgroupMinimal,
    daemon: CgroupMinimal,
}

impl PreppedBuckCgroups {
    /// Called at daemon startup.
    ///
    /// Expectation is that the current process was started with something like systemd-run and:
    ///
    ///  1. The cgroup it's in has this process and no others
    ///  2. Buck2 may manage the child cgroups by itself without interference from outside things;
    ///     in systemd this is the `Delegate=yes` property.
    ///
    /// This function is the part of the cgroup prepping that must be done early on during daemon
    /// startup because it moves the daemon process.
    pub fn prep_current_process() -> buck2_error::Result<Self> {
        let procfs_out = fs_util::read_to_string(AbsPath::new("/proc/self/cgroup").unwrap())
            .categorize_internal()?;
        let launch_cgroup_path = parse_procfs_cgroup_output(&procfs_out)?;
        let launch_cgroup = CgroupMinimal::sync_try_from_path(launch_cgroup_path)?;
        // Make our own cgroups and move ourselves into the daemon one. That's all we have to do at
        // this point, the rest can be done when we complete the cgroup setup later
        let allprocs_cgroup =
            launch_cgroup.sync_discouraged_make_child(FileName::unchecked_new("allprocs"))?;
        let daemon_cgroup =
            allprocs_cgroup.sync_discouraged_make_child(FileName::unchecked_new("daemon"))?;
        let daemon_procs = CgroupFile::sync_open(
            daemon_cgroup.dir_fd(),
            FileNameBuf::unchecked_new("cgroup.procs"),
            CgroupFileMode::ReadWrite,
        )?;
        daemon_procs.sync_write(b"0")?;

        // SAFETY: This is called early enough in the process to be moving cgroups, so we can set
        // env vars too
        unsafe {
            std::env::set_var("CHGDISABLE", "1");
        }

        Ok(PreppedBuckCgroups {
            launch: launch_cgroup,
            allprocs: allprocs_cgroup,
            daemon: daemon_cgroup,
        })
    }

    #[cfg(test)]
    pub(crate) async fn testing_new_in(launch: CgroupMinimal) -> Self {
        let allprocs = launch
            .discouraged_make_child(FileNameBuf::unchecked_new("allprocs"))
            .await
            .unwrap();
        let daemon = allprocs
            .discouraged_make_child(FileNameBuf::unchecked_new("daemon"))
            .await
            .unwrap();
        PreppedBuckCgroups {
            launch,
            allprocs,
            daemon,
        }
    }

    #[cfg(test)]
    pub(crate) async fn testing_new() -> Option<Self> {
        use crate::cgroup::Cgroup;

        Some(Self::testing_new_in(Cgroup::create_minimal_for_test().await?).await)
    }
}

// Resolves a user-specified memory restriction value
fn resolve_memory_restriction_value(
    config: &str,
    from_ancestor: Option<u64>,
) -> buck2_error::Result<Option<u64>> {
    if let Some(perct) = config.strip_suffix("%") {
        let Some(perct) = perct.parse::<u64>().ok().filter(|p| *p > 0 && *p <= 100) else {
            return Err(CgroupConfigParsingError::NotAPercentage(perct.to_owned()).into());
        };
        match from_ancestor {
            // The product can exceed `u64`; the result cannot, since the percentage is at most 100.
            Some(base) => Ok(Some((u128::from(base) * u128::from(perct) / 100) as u64)),
            // Maybe ideally we'd know the system memory but eh
            None => Ok(None),
        }
    } else {
        match config.parse::<u64>() {
            Ok(m) => Ok(Some(m)),
            Err(_) => Err(CgroupConfigParsingError::NotAMemoryConfig(config.to_owned()).into()),
        }
    }
}

/// Type that represents the daemon's view of the cgroups it manages
///
/// Only in use with the action cgroup pool.
pub struct BuckCgroupTree {
    /// The topmost cgroup that buck owns outright. See [`PreppedBuckCgroups`] for why its parent
    /// is off limits.
    allprocs: CgroupInternal,
    forkserver_and_actions: CgroupInternal,
    forkserver: CgroupLeaf,
    daemon: CgroupLeaf,
    /// The resource constraints imposed by the ancestors of the buck cgroup tree
    ///
    /// This does not reflect any of our own configuration
    effective_resource_constraints: EffectiveResourceConstraints,
    /// Whether the `cpuset` controller is available in the parent cgroup. Used to gate
    /// per-action cpuset features so they no-op on hosts that don't have it.
    cpuset_available: bool,
}

impl BuckCgroupTree {
    /// Finishes setting up buck's cgroups from the prepped ones
    pub async fn set_up(
        prepped: PreppedBuckCgroups,
        config: &ResourceControlConfig,
    ) -> buck2_error::Result<Self> {
        let enabled_controllers = prepped.launch.read_enabled_controllers().await?;

        // Drain any orphan processes from the launch cgroup into the daemon cgroup.
        // This is necessary because processes may have been spawned in the launch cgroup
        // before prep_current_process() moved the daemon. cgroupv2 requires that a
        // cgroup has no processes directly in it before enabling subtree controllers.
        let _orphans = prepped.launch.drain_to(&prepped.daemon).await?;

        let cpuset_available = enabled_controllers.contains("cpuset");
        if !cpuset_available {
            soft_error!(
                "daemon_cpuset_unavailable",
                buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "cpuset controller is not available in the cgroup the daemon was launched in"
                )
            );
        }

        prepped
            .launch
            .enable_subtree_control_and_into_internal(enabled_controllers.dupe())
            .await?;

        let allprocs = prepped
            .allprocs
            .enable_subtree_control_and_into_internal(enabled_controllers)
            .await?
            .enable_memory_monitoring()
            .await?;

        let daemon = prepped
            .daemon
            .into_leaf()
            .await?
            .enable_memory_monitoring()
            .await?;

        let forkserver_and_actions = allprocs
            .make_internal_child(FileNameBuf::unchecked_new("forkserver_and_actions"))
            .await?
            .enable_memory_monitoring()
            .await?;

        let forkserver = forkserver_and_actions
            .make_leaf_child(FileNameBuf::unchecked_new("forkserver"))
            .await?
            .enable_memory_monitoring()
            .await?;

        let effective_resource_constraints = allprocs.read_effective_resouce_constraints().await?;

        if let Some(config_memory_max) = &config.memory_max {
            if let Some(allprocs_memory_max) = resolve_memory_restriction_value(
                config_memory_max,
                effective_resource_constraints.memory_max,
            )? {
                allprocs
                    .set_memory_max(&allprocs_memory_max.to_string())
                    .await?;
            }
        }
        if let Some(config_memory_high) = &config.memory_high {
            if let Some(allprocs_memory_high) = resolve_memory_restriction_value(
                config_memory_high,
                effective_resource_constraints.memory_high,
            )? {
                allprocs
                    .set_memory_high(&allprocs_memory_high.to_string())
                    .await?;
            }
        }
        allprocs.set_memory_oom_group().await?;

        Ok(Self {
            allprocs,
            forkserver_and_actions,
            forkserver,
            daemon,
            effective_resource_constraints,
            cpuset_available,
        })
    }

    pub fn forkserver_and_actions(&self) -> &CgroupInternal {
        &self.forkserver_and_actions
    }

    pub fn forkserver(&self) -> &CgroupLeaf {
        &self.forkserver
    }

    /// The parent cgroup that contains all other cgroups buck manages as descendants
    pub fn allprocs(&self) -> &CgroupInternal {
        &self.allprocs
    }

    pub fn daemon(&self) -> &CgroupLeaf {
        &self.daemon
    }

    pub(crate) fn effective_resource_constraints(&self) -> &EffectiveResourceConstraints {
        &self.effective_resource_constraints
    }

    pub fn cpuset_available(&self) -> bool {
        self.cpuset_available
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::init::ResourceControlConfig;
    use buck2_fs::paths::file_name::FileNameBuf;

    use crate::buck_cgroup_tree::BuckCgroupTree;
    use crate::buck_cgroup_tree::PreppedBuckCgroups;
    use crate::buck_cgroup_tree::parse_procfs_cgroup_output;
    use crate::buck_cgroup_tree::resolve_memory_restriction_value;
    use crate::cgroup::Cgroup;
    use crate::path::CgroupPath;

    /// A percentage of an ancestor limit is computed without overflowing `u64`.
    #[test]
    fn test_resolve_memory_restriction_value_large_ancestor() {
        let r = resolve_memory_restriction_value("50%", Some(1_000_000_000_000_000_000)).unwrap();
        assert_eq!(r, Some(500_000_000_000_000_000));
        let r = resolve_memory_restriction_value("100%", Some(u64::MAX)).unwrap();
        assert_eq!(r, Some(u64::MAX));
        let r = resolve_memory_restriction_value("1%", Some(u64::MAX)).unwrap();
        assert_eq!(r, Some(u64::MAX / 100));
    }

    #[test]
    fn test_cgroup_info_parse() {
        let cgroup = "\
0::/user.slice/user-532497.slice/user@532497.service/buck2.cg\n";
        assert_eq!(
            "/sys/fs/cgroup/user.slice/user-532497.slice/user@532497.service/buck2.cg",
            parse_procfs_cgroup_output(cgroup).unwrap().to_string()
        );

        let cgroup = "\
5:cpuacct,cpu,cpuset:/daemons
0::/user.slice/user-532497.slice/user@532497.service/buck2.cg\n";
        assert_eq!(
            "/sys/fs/cgroup/user.slice/user-532497.slice/user@532497.service/buck2.cg",
            parse_procfs_cgroup_output(cgroup).unwrap().to_string()
        );
    }

    #[test]
    fn test_resolve_memory_restriction_value() {
        assert_eq!(
            Some(100),
            resolve_memory_restriction_value("100", None).unwrap()
        );
        assert_eq!(
            Some(100),
            resolve_memory_restriction_value("100", Some(10000)).unwrap()
        );
        assert_eq!(
            Some(1000),
            resolve_memory_restriction_value("1000", Some(100)).unwrap()
        );
        assert_eq!(
            Some(50),
            resolve_memory_restriction_value("50%", Some(100)).unwrap()
        );
        assert_eq!(None, resolve_memory_restriction_value("50%", None).unwrap());
    }

    /// Tests that an orphan process sitting directly in a cgroup causes EBUSY
    /// when enabling subtree control, due to cgroupv2's no-internal-process constraint.
    #[tokio::test]
    async fn test_orphan_process_causes_ebusy() {
        use buck2_util::process::background_command;

        let Some(r) = Cgroup::create_internal_for_test().await else {
            return;
        };

        // Spawn a long-running process to act as the orphan
        let mut orphan = background_command("sleep");
        orphan.arg("300");
        let mut orphan = orphan.spawn().unwrap();
        let orphan_pid = orphan.id();

        // Create a fresh cgroup and move the orphan into it
        let root = r
            .make_child(FileNameBuf::unchecked_new("ebusy_root"))
            .await
            .unwrap();
        let root_path = root.path().as_abs_path().to_path_buf();
        let root_procs_path = root_path.join("cgroup.procs");
        std::fs::write(&root_procs_path, orphan_pid.to_string()).unwrap();

        // Verify the orphan is in the root cgroup
        let procs_content = std::fs::read_to_string(&root_procs_path).unwrap();
        assert!(
            procs_content.contains(&orphan_pid.to_string()),
            "Orphan PID {} should be in root cgroup, got: {}",
            orphan_pid,
            procs_content.trim()
        );

        // Enabling subtree control fails with EBUSY when a process sits directly in the
        // cgroup. Write "+memory" directly to cgroup.subtree_control via the filesystem.
        let subtree_control_path = root_path.join("cgroup.subtree_control");
        let ebusy_result = std::fs::write(&subtree_control_path, "+memory");
        match ebusy_result {
            Err(e) => {
                assert_eq!(
                    e.raw_os_error(),
                    Some(nix::libc::EBUSY),
                    "Expected EBUSY, got: {}",
                    e
                );
            }
            Ok(_) => panic!("Expected EBUSY error, but subtree control write succeeded"),
        }

        orphan.kill().unwrap();
        orphan.wait().unwrap();
    }

    /// Tests that `drain_to_child()` (via `set_up()`) moves an orphan process out of the
    /// scope root and into the daemon child cgroup before enabling subtree control.
    #[tokio::test]
    async fn test_drain_to_child_moves_orphan_to_daemon() {
        use buck2_util::process::background_command;

        let Some(r) = Cgroup::create_internal_for_test().await else {
            return;
        };

        // Spawn a long-running process to act as the orphan
        let mut orphan = background_command("sleep");
        orphan.arg("300");
        let mut orphan = orphan.spawn().unwrap();
        let orphan_pid = orphan.id();

        // Create a fresh cgroup and move the orphan into it
        let root = r
            .make_child(FileNameBuf::unchecked_new("drain_root"))
            .await
            .unwrap();
        let root_procs_path = root.path().as_abs_path().join("cgroup.procs");
        std::fs::write(&root_procs_path, orphan_pid.to_string()).unwrap();

        // Run the full set_up() flow, which drains orphans via drain_to_child().
        let p = PreppedBuckCgroups::testing_new_in(root).await;
        let t = BuckCgroupTree::set_up(
            p,
            &ResourceControlConfig {
                ..ResourceControlConfig::testing_default()
            },
        )
        .await
        .unwrap();

        // Verify the orphan was moved to the daemon child cgroup
        let daemon_procs_path = t.daemon().path().as_abs_path().join("cgroup.procs");
        let daemon_procs_content = std::fs::read_to_string(&daemon_procs_path).unwrap();
        assert!(
            daemon_procs_content.contains(&orphan_pid.to_string()),
            "Orphan PID {} should have been moved to daemon cgroup, got: {}",
            orphan_pid,
            daemon_procs_content.trim()
        );

        orphan.kill().unwrap();
        orphan.wait().unwrap();
    }

    #[tokio::test]
    async fn test_memory_limit_relative_to_parent() {
        let Some(r) = Cgroup::create_internal_for_test().await else {
            return;
        };
        r.set_memory_high(&(1 << 20).to_string()).await.unwrap();
        let root = r
            .make_child(FileNameBuf::unchecked_new("root"))
            .await
            .unwrap();
        let p = PreppedBuckCgroups::testing_new_in(root).await;
        let t = BuckCgroupTree::set_up(
            p,
            &ResourceControlConfig {
                memory_high: Some("50%".to_owned()),
                ..ResourceControlConfig::testing_default()
            },
        )
        .await
        .unwrap();
        let c = t
            .allprocs()
            .read_effective_resouce_constraints()
            .await
            .unwrap();
        assert_eq!(c.memory_high, Some(1 << 19));
    }

    /// The launch cgroup's attributes belong to whoever launched the daemon, so the daemon-wide
    /// memory attributes must land on `allprocs` instead.
    #[tokio::test]
    async fn test_daemon_wide_memory_attributes_placement() {
        let Some(r) = Cgroup::create_internal_for_test().await else {
            return;
        };
        let launch = r
            .make_child(FileNameBuf::unchecked_new("launch"))
            .await
            .unwrap();
        let launch_path = launch.path().to_buf();
        let p = PreppedBuckCgroups::testing_new_in(launch).await;
        let t = BuckCgroupTree::set_up(
            p,
            &ResourceControlConfig {
                memory_high: Some((1 << 20).to_string()),
                ..ResourceControlConfig::testing_default()
            },
        )
        .await
        .unwrap();

        let read = |cgroup: &CgroupPath, file: &str| {
            std::fs::read_to_string(cgroup.as_abs_path().join(file))
                .unwrap()
                .trim()
                .to_owned()
        };
        let allprocs_path = t.allprocs().path();
        assert_eq!(allprocs_path.parent(), Some(&*launch_path));
        assert_eq!(read(&launch_path, "memory.oom.group"), "0");
        assert_eq!(read(&launch_path, "memory.high"), "max");
        assert_eq!(read(allprocs_path, "memory.oom.group"), "1");
        assert_eq!(read(allprocs_path, "memory.high"), (1 << 20).to_string());
    }
}
