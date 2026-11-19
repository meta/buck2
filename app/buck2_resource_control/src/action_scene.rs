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
use std::time::Instant;

use dupe::Dupe;

use crate::CommandType;
use crate::RetryFuture;
use crate::cgroup::CgroupLeaf;
use crate::memory_tracker::MemoryTrackerHandle;
use crate::path::CgroupPathBuf;
use crate::scheduler::SceneDescription;
use crate::scheduler::SceneId;
use crate::scheduler::SceneResourceReading;

#[derive(Debug, Clone)]
pub struct ActionCgroupResult {
    pub memory_peak: Option<u64>,
    pub swap_peak: Option<u64>,
    pub error: Option<buck2_error::Error>,
    pub suspend_duration: Option<Duration>,
    pub suspend_count: u64,
}

pub(crate) struct ActionScene {
    pub(crate) cgroup: CgroupLeaf,
    pub(crate) memory_initial: u64,
    pub(crate) swap_initial: u64,
    error: Option<buck2_error::Error>,
}

impl ActionScene {
    #[allow(clippy::result_large_err)]
    async fn new(cgroup: CgroupLeaf) -> Result<Self, (buck2_error::Error, CgroupLeaf)> {
        let (memory_initial, swap_initial) =
            match tokio::try_join!(cgroup.read_memory_current(), cgroup.read_swap_current(),) {
                Ok(x) => x,
                Err(e) => return Err((e, cgroup)),
            };

        Ok(ActionScene {
            cgroup,
            memory_initial,
            swap_initial,
            error: None,
        })
    }

    pub(crate) async fn poll_resources(&mut self) -> Option<SceneResourceReading> {
        match tokio::try_join!(
            self.cgroup.read_memory_current(),
            self.cgroup.read_swap_current(),
        ) {
            Ok((memory_current, swap_current)) => {
                let memory_current = memory_current.saturating_sub(self.memory_initial);
                let swap_current = swap_current.saturating_sub(self.swap_initial);
                Some(SceneResourceReading {
                    memory_current,
                    swap_current,
                })
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }
}

// Interface between forkserver/executors and ActionCgroups used to report when commands
// are active and return cgroup results for a single command.
pub struct ActionCgroupSession {
    // Pointer to the cgroup pool and not owned by the session. This is mainly used for the session to mark a cgroup as being used
    // when starting a command and then releasing it back to the pool when the command finishes.
    tracker: MemoryTrackerHandle,
    scene_id: SceneId,
    pub path: CgroupPathBuf,
}

impl ActionCgroupSession {
    pub async fn maybe_create(
        tracker: Option<MemoryTrackerHandle>,
        command_type: CommandType,
        action_digest: Option<String>,
        disable_kill_and_retry_suspend: bool,
    ) -> buck2_error::Result<Option<(Self, RetryFuture)>> {
        let Some(tracker) = tracker else {
            return Ok(None);
        };

        let cgroup = tracker.pool.lock().await.acquire().await?;
        let path = cgroup.path().to_buf();

        let action_scene = match ActionScene::new(cgroup).await {
            Ok(x) => x,
            Err((e, cgroup)) => {
                // FIXME(JakobDegen): A proper drop impl on the id would be better than this
                tracker.pool.lock().await.release(cgroup);
                return Err(e);
            }
        };

        let (scene_id, start_future) = tracker.action_cgroups.lock().unwrap().scene_started(
            SceneDescription {
                action_digest,
                command_type,
            },
            disable_kill_and_retry_suspend,
        );

        tracker
            .scene_action_mapping
            .lock()
            .await
            .insert(scene_id.as_ref(), action_scene);

        Ok(Some((
            ActionCgroupSession {
                tracker: tracker.dupe(),
                scene_id,
                path,
            },
            start_future,
        )))
    }

    pub async fn action_finished(self) -> ActionCgroupResult {
        let action_scene = self
            .tracker
            .scene_action_mapping
            .lock()
            .await
            .remove(&self.scene_id.as_ref())
            .unwrap();

        self.tracker.pool.lock().await.release(action_scene.cgroup);

        let res = self
            .tracker
            .action_cgroups
            .lock()
            .unwrap()
            .scene_finished(self.scene_id, Instant::now());

        if let Some(error) = action_scene.error {
            ActionCgroupResult {
                memory_peak: None,
                swap_peak: None,
                error: Some(error),
                suspend_duration: None,
                suspend_count: 0,
            }
        } else {
            ActionCgroupResult {
                memory_peak: Some(res.memory_peak),
                swap_peak: Some(res.swap_peak),
                error: None,
                suspend_duration: res.suspend_duration,
                suspend_count: res.suspend_count,
            }
        }
    }
}
