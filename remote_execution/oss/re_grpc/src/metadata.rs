/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

pub type TPlatform = crate::grpc::Platform;
pub type TProperty = crate::grpc::Property;

#[derive(Clone, Default)]
pub struct ActionHistoryInfo {
    pub action_key: String,
    pub disable_retry_on_oom: bool,
    pub _dot_dot: (),
}

#[derive(Clone, Default)]
pub struct BuckInfo {
    pub build_id: String,
    pub version: String,
    pub _dot_dot: (),
}

#[derive(Clone, Default)]
pub struct TClientContextMetadata {
    pub attributes: BTreeMap<String, String>,
    pub _dot_dot: (),
}

#[derive(Clone, Default)]
pub struct RemoteExecutionMetadata {
    pub action_history_info: Option<ActionHistoryInfo>,
    pub buck_info: Option<BuckInfo>,
    pub platform: Option<TPlatform>,
    pub use_case_id: String,
    pub do_not_cache: bool,
    pub respect_file_symlinks: Option<bool>,
    pub client_context: Option<TClientContextMetadata>,
    pub disable_cancel_on_disconnect: bool,
    pub _dot_dot: (),
}
