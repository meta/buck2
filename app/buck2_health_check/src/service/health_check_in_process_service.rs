/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(dead_code)] // Presently used only in oss

use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;

use crate::interface::HealthCheckContextEvent;
use crate::interface::HealthCheckService;
use crate::interface::HealthCheckSnapshotData;
use crate::report::Report;

pub struct HealthCheckInProcessService {}

impl HealthCheckInProcessService {
    pub fn new(_health_check_dir: AbsNormPathBuf) -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl HealthCheckService for HealthCheckInProcessService {
    async fn update_context(&mut self, _event: HealthCheckContextEvent) -> buck2_error::Result<()> {
        Ok(())
    }

    async fn run_checks(
        &mut self,
        _snapshot: HealthCheckSnapshotData,
    ) -> buck2_error::Result<Vec<Report>> {
        Ok(Vec::new())
    }
}
