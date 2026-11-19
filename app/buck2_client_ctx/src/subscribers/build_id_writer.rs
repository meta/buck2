/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use async_trait::async_trait;
use buck2_error::BuckErrorContext;
use buck2_events::BuckEvent;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::paths::abs_path::AbsPathBuf;

use crate::subscribers::subscriber::EventSubscriber;

pub struct BuildIdWriter {
    path: AbsPathBuf,
}

impl BuildIdWriter {
    pub fn new(path: AbsPathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl EventSubscriber for BuildIdWriter {
    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        for event in events {
            if event.command_start()?.is_some() {
                // input path from --write-build-id
                async_fs_util::write(&self.path, event.trace_id()?.to_string())
                    .await
                    .categorize_input()
                    .buck_error_context("Error writing build ID")?;
            }
        }
        Ok(())
    }
}
