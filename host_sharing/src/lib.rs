/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![deny(unused_crate_dependencies)]
mod named_semaphores;
pub use named_semaphores::NamedSemaphores;

pub mod host_sharing;
pub use crate::host_sharing::HostSharingBroker;
pub use crate::host_sharing::HostSharingRequirements;
pub use crate::host_sharing::HostSharingStrategy;
pub use crate::host_sharing::WeightClass;
pub use crate::host_sharing::WeightPercentage;
