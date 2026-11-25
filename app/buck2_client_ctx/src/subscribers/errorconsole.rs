/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;

use crate::subscribers::subscriber::EventSubscriber;

/// This console is what is used for `--console none` and only prints errors.
///
/// It is also used as a part of simpleconsole's implementation.
pub struct ErrorConsole;

#[async_trait]
impl EventSubscriber for ErrorConsole {
    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        if let buck2_cli_proto::CommandResult {
            result: Some(buck2_cli_proto::command_result::Result::Error(error)),
            ..
        } = result
        {
            crate::eprintln!("Command failed: ")?;
            crate::eprintln!("{}", error.message)?;
        }

        Ok(())
    }
}
