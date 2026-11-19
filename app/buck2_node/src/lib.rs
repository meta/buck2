/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(box_patterns)]
#![allow(clippy::len_without_is_empty)]

pub mod attrs;
pub mod bzl_or_bxl_path;
pub mod call_stack;
pub mod cfg_constructor;
pub mod configuration;
pub mod configured_universe;
pub mod execution;
pub mod load_patterns;
pub mod metadata;
pub mod modifiers;
pub mod nodes;
pub mod oncall;
pub mod package;
pub mod package_values_calculation;
pub mod package_visibility;
pub mod provider_id_set;
pub mod query;
pub mod rule;
pub mod rule_type;
pub mod super_package;
pub mod target_calculation;
pub mod visibility;
