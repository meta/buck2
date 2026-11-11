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
use buck2_cli_proto::EvictTenantRequest;
use buck2_cli_proto::UnstableListTenantsRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;

#[derive(Debug, clap::Parser)]
pub struct TenantsCommand {
    #[clap(long, hide = true)]
    evict_generation: Option<u64>,
    #[clap(long, hide = true)]
    eviction_timeout_ms: Option<u64>,
    #[clap(long, hide = true)]
    no_wait: bool,
    /// Emit a stable JSON representation for tooling.
    #[clap(long)]
    json: bool,

    #[clap(flatten)]
    common_event_opts: CommonEventLogOptions,
}

#[async_trait(?Send)]
impl StreamingCommand for TenantsCommand {
    const COMMAND_NAME: &'static str = "tenants";

    fn existing_only() -> bool {
        true
    }

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        _matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        if let Some(generation) = self.evict_generation {
            let result = buckd
                .with_flushing()
                .evict_tenant(
                    EvictTenantRequest {
                        context: Some(ctx.empty_client_context(Self::COMMAND_NAME)?),
                        expected_generation: Some(generation),
                        eviction_timeout_ms: self.eviction_timeout_ms,
                        wait_for_completion: Some(!self.no_wait),
                    },
                    events_ctx,
                )
                .await?;
            buck2_client_ctx::println!("{}", serde_json::to_string(&result)?)?;
            return ExitResult::success();
        }
        let response = buckd
            .with_flushing()
            .unstable_list_tenants(UnstableListTenantsRequest {}, events_ctx)
            .await?;

        if self.json {
            let tenants: Vec<_> = response
                .tenants
                .iter()
                .map(|tenant| {
                    serde_json::json!({
                        "project_root": tenant.project_root,
                        "isolation": tenant.isolation,
                        "generation": tenant.generation,
                        "initialized": tenant.initialized,
                    })
                })
                .collect();
            buck2_client_ctx::println!("{}", serde_json::to_string_pretty(&tenants)?)?;
        } else {
            for tenant in response.tenants {
                buck2_client_ctx::println!(
                    "{}\t{}\t{}\t{}",
                    tenant.generation,
                    if tenant.initialized {
                        "ready"
                    } else {
                        "initializing"
                    },
                    tenant.project_root,
                    tenant.isolation,
                )?;
            }
        }

        ExitResult::success()
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        CommonConsoleOptions::none_ref()
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_event_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        CommonBuildConfigurationOptions::default_ref()
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        CommonStarlarkOptions::default_ref()
    }
}
