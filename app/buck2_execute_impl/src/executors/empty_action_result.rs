/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::execution_types::executor_config::RePlatformFields;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobsBuilder;
use remote_execution as RE;
use remote_execution::TActionResult2;
use remote_execution::TExecutedActionMetadata;

use crate::executors::to_re_platform::RePlatformFieldsToRePlatform;

/// Create an empty action result for permission check.
pub(crate) fn empty_action_result(
    platform: &RePlatformFields,
    digest_config: DigestConfig,
) -> buck2_error::Result<(ActionDigestAndBlobs, TActionResult2)> {
    let mut blobs = ActionDigestAndBlobsBuilder::new(digest_config);

    let command = blobs.add_command(&RE::Command {
        arguments: vec![
            "/command".to_owned(),
            "-to".to_owned(),
            "check".to_owned(),
            "permission".to_owned(),
            // Random string for xbgs.
            "EMPTY_ACTION_RESULT_fztiucvwawdmarhheqoz".to_owned(),
        ],
        #[allow(deprecated)]
        platform: Some(platform.to_re_platform()),
        ..Default::default()
    });

    let action = blobs.build(&RE::Action {
        command_digest: Some(command.to_grpc()),
        ..Default::default()
    });

    let action_result = TActionResult2 {
        stdout_raw: Some(Vec::new()),
        stderr_raw: Some(Vec::new()),
        exit_code: 0,
        execution_metadata: Box::new(TExecutedActionMetadata {
            execution_attempts: 0,
            ..Default::default()
        }),
        ..Default::default()
    };

    Ok((action, action_result))
}
