/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::VecDeque;

use buck2_common::init::ResourceControlConfig;
use buck2_fs::paths::file_name::FileNameBuf;

use crate::cgroup::CgroupInternal;
use crate::cgroup::CgroupLeaf;

pub(crate) struct CgroupPool {
    pool_cgroup: CgroupInternal,
    next_worker_id: u64,
    per_cgroup_memory_high: Option<String>,
    available: VecDeque<CgroupLeaf>,
}

impl CgroupPool {
    fn worker_name(i: u64) -> FileNameBuf {
        if i < 1000 {
            FileNameBuf::unchecked_new(format!("worker_{i:03}"))
        } else {
            FileNameBuf::unchecked_new(format!("worker_{i}"))
        }
    }

    /// The new cgroup is assumed to be in use
    async fn reserve_additional_cgroup(&mut self) -> buck2_error::Result<CgroupLeaf> {
        let id = self.next_worker_id;
        self.next_worker_id += 1;
        let worker_name = Self::worker_name(id);
        let cgroup = self
            .pool_cgroup
            .make_leaf_child(worker_name)
            .await?
            .enable_memory_monitoring()
            .await?;

        // Set memory.high limit if provided
        if let Some(per_cgroup_memory_high) = &self.per_cgroup_memory_high {
            cgroup.set_memory_high(per_cgroup_memory_high).await?;
        }

        Ok(cgroup)
    }

    /// Create a cgroup pool in the provided parent cgroup.
    pub(crate) async fn create_in_parent_cgroup(
        parent: &CgroupInternal,
        config: &ResourceControlConfig,
    ) -> buck2_error::Result<Self> {
        let pool_cgroup = parent
            .make_internal_child(FileNameBuf::unchecked_new("actions_cgroup_pool"))
            .await?
            .enable_memory_monitoring()
            .await?;

        if let Some(pool_memory_high) = &config.memory_high_actions {
            pool_cgroup.set_memory_high(pool_memory_high).await?;
        }
        if let Some(pool_memory_max) = &config.memory_max_actions {
            pool_cgroup.set_memory_max(pool_memory_max).await?;
        }
        if let Some(pool_memory_swap_max) = &config.memory_swap_max_actions {
            pool_cgroup
                .set_memory_swap_max(pool_memory_swap_max)
                .await?;
        }

        Ok(CgroupPool {
            available: VecDeque::new(),
            next_worker_id: 0,
            per_cgroup_memory_high: config.memory_high_per_action.clone(),
            pool_cgroup,
        })
    }

    /// Acquire a worker cgroup from the pool. If no available worker cgroup, create a new one.
    pub(crate) async fn acquire(&mut self) -> buck2_error::Result<CgroupLeaf> {
        if let Some(cgroup) = self.available.pop_front() {
            Ok(cgroup)
        } else {
            self.reserve_additional_cgroup().await
        }
    }

    pub(crate) fn release(&mut self, cgroup: CgroupLeaf) {
        // TODO(nero): Reset memory peak
        // reset memory peak is available for linux kernel 6.12 and above, do this when we upgrade the kernel
        // TODO(nero): Kill all processes in the cgroup
        // As Jakob said: it's possible for someone to spawn a persistent daemon from a test.
        // We needs a broader announcement/rollout for this change.
        self.available.push_back(cgroup);
    }
}
