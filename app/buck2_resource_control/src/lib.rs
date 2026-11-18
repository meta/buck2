/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(trait_alias)]

use futures::Stream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

#[cfg(unix)]
pub mod action_scene;
#[cfg(unix)]
pub mod buck_cgroup_tree;
#[cfg(unix)]
pub mod cgroup;
#[cfg(unix)]
pub mod cgroup_files;
#[cfg(unix)]
pub(crate) mod host_swap;
#[cfg(unix)]
pub mod memory_tracker;
pub mod path;
#[cfg(unix)]
pub mod pool;
#[cfg(unix)]
pub(crate) mod scheduler;
pub mod spawn_daemon;

pub struct HasResourceControl(pub bool);

/// Info about an orphan process that was killed via cgroup cleanup.
#[derive(Debug, Clone)]
pub struct OrphanProcessInfo {
    /// The process ID.
    pub pid: u32,
    /// The process name from `/proc/<pid>/comm`.
    pub comm: String,
}

#[cfg(not(unix))]
pub mod buck_cgroup_tree {
    use buck2_common::init::ResourceControlConfig;

    /// Return the current cgroup path for logging, if supported on this platform.
    pub fn read_current_cgroup() -> Option<String> {
        None
    }

    pub(crate) fn parse_procfs_cgroup_output(
        _out: &str,
    ) -> buck2_error::Result<crate::path::CgroupPathBuf> {
        unreachable!("not used on windows")
    }

    pub struct PreppedBuckCgroups;

    impl PreppedBuckCgroups {
        pub fn prep_current_process() -> buck2_error::Result<Self> {
            unreachable!("not used on windows")
        }
    }

    #[derive(allocative::Allocative)]
    pub struct BuckCgroupTree;

    impl BuckCgroupTree {
        pub async fn set_up(
            _prepped: PreppedBuckCgroups,
            _config: &ResourceControlConfig,
        ) -> buck2_error::Result<Self> {
            unreachable!("not used on windows")
        }
    }
}

#[cfg(not(unix))]
pub mod memory_tracker {
    use std::sync::Arc;

    use allocative::Allocative;
    use buck2_common::init::ResourceControlConfig;
    use buck2_events::daemon_id::DaemonId;

    use crate::buck_cgroup_tree::BuckCgroupTree;

    #[derive(Allocative)]
    pub struct MemoryTracker {
        pub cgroup_tree: BuckCgroupTree,
    }

    impl MemoryTracker {
        /// Never signals pressure; its sender is already gone.
        pub fn memory_pressure(&self) -> tokio::sync::watch::Receiver<bool> {
            tokio::sync::watch::Sender::new(false).subscribe()
        }
    }

    pub type MemoryTrackerHandle = Arc<MemoryTracker>;

    pub async fn create_memory_tracker(
        _cgroup_tree: Option<BuckCgroupTree>,
        _resource_control_config: &ResourceControlConfig,
        _daemon_id: &DaemonId,
    ) -> buck2_error::Result<Option<MemoryTrackerHandle>> {
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CommandType {
    Build,
    Test,
    Worker,
}

impl std::fmt::Display for CommandType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CommandType::Build => "BUILD",
            CommandType::Test => "TEST",
            CommandType::Worker => "WORKER",
        };
        write!(f, "{}", s)
    }
}

pub enum ActionFreezeEvent {
    Freeze,
    Unfreeze,
}

pub trait ActionFreezeEventReceiver = Stream<Item = ActionFreezeEvent> + Send + 'static;

/// A channel associated with a running action that resolves when the action is killed for
/// resource control
#[derive(Debug)]
pub struct KillFuture(pub oneshot::Receiver<RetryFuture>);

/// A channel associated with a killed action that resolves when the action should be restarted
#[derive(Debug)]
pub struct RetryFuture(
    pub oneshot::Receiver<(KillFuture, mpsc::UnboundedReceiver<ActionFreezeEvent>)>,
);

#[cfg(not(unix))]
pub mod action_scene {
    use std::time::Duration;

    use crate::CommandType;
    use crate::RetryFuture;
    use crate::memory_tracker::MemoryTrackerHandle;
    use crate::path::CgroupPathBuf;

    pub struct ActionCgroupResult {
        pub memory_peak: Option<u64>,
        pub error: Option<buck2_error::Error>,
        pub suspend_duration: Option<Duration>,
        pub suspend_count: u64,
    }

    pub struct ActionCgroupSession {
        pub path: CgroupPathBuf,
    }
    impl ActionCgroupSession {
        pub async fn maybe_create(
            _tracker: Option<MemoryTrackerHandle>,
            _command_type: CommandType,
            _action_digest: Option<String>,
            _disable_kill_and_retry_suspend: bool,
        ) -> buck2_error::Result<Option<(Self, RetryFuture)>> {
            Ok(None)
        }

        pub async fn action_started(&mut self, _cgroup_path: CgroupPathBuf) {}

        pub async fn action_finished(self) -> ActionCgroupResult {
            unreachable!("not supported");
        }
    }
}
