/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_events::dispatch::get_dispatcher_opt;
use remote_execution::ActionHistoryInfo;
use remote_execution::BuckInfo;
use remote_execution::RemoteExecutionMetadata;

use crate::re::action_identity::ReActionIdentity;

pub trait RemoteExecutionMetadataExt {
    fn metadata(&self, identity: Option<&ReActionIdentity>) -> RemoteExecutionMetadata;
}

impl RemoteExecutionMetadataExt for RemoteExecutorUseCase {
    fn metadata(&self, identity: Option<&ReActionIdentity>) -> RemoteExecutionMetadata {
        let trace_id = match get_dispatcher_opt() {
            Some(dispatcher) => dispatcher.trace_id().to_string(),
            // See the FIXME added in D54396421
            None => String::new(),
        };
        RemoteExecutionMetadata {
            use_case_id: self.as_str().to_owned(),
            buck_info: Some(BuckInfo {
                build_id: trace_id,
                ..Default::default()
            }),
            action_history_info: identity.map(|identity| ActionHistoryInfo {
                action_key: identity.action_key.clone(),
                disable_retry_on_oom: false,
                ..Default::default()
            }),
            disable_cancel_on_disconnect: true,
            ..Default::default()
        }
    }
}
