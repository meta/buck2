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

use allocative::Allocative;
use pagable::Pagable;

use crate::execution_types::execution::ExecutionPlatform;
use crate::target::label::label::TargetLabel;

pub type ExecutionPlatforms = Arc<ExecutionPlatformsData>;

#[derive(Debug, Allocative, Pagable)]
pub enum ExecutionPlatformFallback {
    Error,
    UseUnspecifiedExec,
    Platform(ExecutionPlatform),
}

#[derive(Debug, Allocative, Pagable)]
pub struct ExecutionPlatformsData {
    execution_platforms_target: TargetLabel,
    platforms: Vec<ExecutionPlatform>,
    fallback: ExecutionPlatformFallback,
}

impl ExecutionPlatformsData {
    pub fn new(
        execution_platforms_target: TargetLabel,
        platforms: Vec<ExecutionPlatform>,
        fallback: ExecutionPlatformFallback,
    ) -> Self {
        Self {
            execution_platforms_target,
            platforms,
            fallback,
        }
    }

    pub fn execution_platforms_target(&self) -> &TargetLabel {
        &self.execution_platforms_target
    }

    pub fn candidates(&self) -> impl Iterator<Item = &ExecutionPlatform> {
        self.platforms.iter()
    }

    pub fn fallback(&self) -> &ExecutionPlatformFallback {
        &self.fallback
    }
}
