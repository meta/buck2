/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The providers that describe configurations and platforms. They are consumed by
//! configuration resolution, not by the build itself, so they live outside `buck2_build_api`.

pub mod configuration_info;
pub mod constraint_setting_info;
pub mod constraint_value_info;
pub mod dep_only_incompatible_info;
pub mod dep_only_incompatible_rollout;
pub mod execution_platform_info;
pub mod execution_platform_registration_info;
pub mod platform_info;
