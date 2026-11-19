/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp;
use std::iter;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_core::env::registry::Applicability;
use buck2_core::env::registry::ENV_INFO;
use buck2_core::env::registry::EnvInfoEntry;
use buck2_error::internal_error;

/// Print help for environment variables used by buck2.
#[derive(Debug, clap::Parser)]
pub struct HelpEnvCommand {
    /// Also print those environment variables that are only used for buck2 integration tests.
    ///
    /// These are all unstable and not meant to be used by most users.
    #[clap(long)]
    self_testing: bool,
}

impl HelpEnvCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, _ctx: ClientCommandContext<'_>) -> ExitResult {
        // TODO(nga): print special buckconfigs too.

        // This command depends on `linkme` aggregating all the environment variables.
        if let Some(res) = ExitResult::retry_command_with_full_binary()? {
            return res;
        }

        let mut env_info: Vec<EnvInfoEntry> = ENV_INFO
            .iter()
            .copied()
            .filter(|x| match x.applicability {
                Applicability::All => true,
                Applicability::Testing => self.self_testing,
                Applicability::Internal => !buck2_core::is_open_source(),
            })
            .collect();
        env_info.sort();
        env_info.dedup();

        let longest_name = env_info.iter().map(|e| e.name.len()).max().ok_or_else(|| {
            internal_error!("No environment variables stored defined, this is a bug")
        })?;
        let longest_ty = env_info
            .iter()
            .map(|e| e.ty_short().len())
            .max()
            .ok_or_else(|| {
                internal_error!("No environment variables stored defined, this is a bug")
            })?;
        let longest_default = env_info
            .iter()
            .filter_map(|e| e.default)
            .map(|d| d.len())
            .max()
            .unwrap_or(0);
        let name_column_title = "Name";
        let ty_column_title = "Type";
        let default_column_title = "Default";
        let name_column_width = cmp::max(longest_name, name_column_title.len());
        let ty_column_width = cmp::max(longest_ty, ty_column_title.len());
        let default_column_width = cmp::max(longest_default, default_column_title.len());
        let rows = iter::once((name_column_title, ty_column_title, default_column_title)).chain(
            env_info
                .iter()
                .map(|e| (e.name, e.ty_short(), e.default.unwrap_or_default())),
        );
        for (name, ty, default) in rows {
            let line = format!(
                "{name:name_column_width$} {ty:ty_column_width$} {default:default_column_width$}",
            );
            buck2_client_ctx::println!("{}", line.trim_end())?;
        }
        ExitResult::success()
    }
}
