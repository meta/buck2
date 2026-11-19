/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use async_trait::async_trait;
use buck2_cli_proto::BxlProfile;
use buck2_cli_proto::ProfileRequest;
use buck2_cli_proto::ProfileResponse;
use buck2_cli_proto::TargetProfile;
use buck2_cli_proto::profile_request::ProfileOpts;
use buck2_cli_proto::target_profile;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::profiling::BuckProfileMode;
use buck2_client_ctx::common::target_cfg::TargetCfgWithUniverseOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;

use super::bxl::BxlCommandOptions;

#[derive(Debug, clap::Parser)]
#[clap(about = "Run starlark profiler")]
pub enum ProfileCommand {
    Analysis(ProfileAnalysisCommand),
    Loading(ProfileLoadingCommand),
    Bxl(ProfileBxlCommand),
}

impl ProfileCommand {
    pub fn exec(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let submatches = matches.unwrap_subcommand();
        ctx.exec(
            ProfileSubcommand { subcommand: self },
            submatches,
            events_ctx,
        )
    }

    pub fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }
}

/// Profile BXL script.
#[derive(Debug, clap::Parser)]
pub struct ProfileBxlCommand {
    #[clap(flatten)]
    bxl_opts: BxlCommandOptions,

    #[clap(flatten)]
    profile_common_opts: ProfileCommonOptions,
}

/// Profile `BUCK` file evaluation.
#[derive(Debug, clap::Parser)]
pub struct ProfileLoadingCommand {
    #[clap(flatten)]
    buck_opts: AnalysisOrLoadProfileOptions,

    #[clap(flatten)]
    profile_common_opts: ProfileCommonOptions,
}

/// Profile analysis.
#[derive(Debug, clap::Parser)]
pub struct ProfileAnalysisCommand {
    #[clap(flatten)]
    buck_opts: AnalysisOrLoadProfileOptions,

    #[clap(flatten)]
    profile_common_opts: ProfileCommonOptions,
}

/// Common options for `profile loading` and `profile analysis`.
#[derive(Debug, clap::Parser)]
struct AnalysisOrLoadProfileOptions {
    #[clap(value_name = "TARGET_PATTERNS")]
    target_patterns: Vec<String>,

    /// In analysis profiling, capture the profile of the target and its dependencies,
    /// and output the merged profile.
    #[clap(long, short = 'r')]
    recursive: bool,
}

/// Common options for three profile subcommands.
#[derive(Debug, clap::Parser)]
struct ProfileCommonOptions {
    /// Output file path for profile data.
    ///
    /// File will be created if it does not exist, and overwritten if it does.
    #[clap(long, short = 'o', value_name = "PATH")]
    output: PathArg,

    /// Profile mode.
    ///
    /// Memory profiling modes have suffixes either `-allocated` or `-retained`.
    ///
    /// `-retained` means memory kept in frozen starlark heap after analysis complete.
    /// `-retained` does not work when profiling loading,
    /// because no memory is retained after loading and frozen heap is not even created.
    /// This is probably what you want when profiling analysis.
    ///
    /// `-allocated` means allocated memory, including memory which is later garbage collected.
    #[clap(long, value_enum)]
    mode: BuckProfileMode,

    #[clap(flatten)]
    target_cfg: TargetCfgWithUniverseOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

struct ProfileSubcommand {
    subcommand: ProfileCommand,
}

impl ProfileSubcommand {
    fn common_opts(&self) -> &ProfileCommonOptions {
        match &self.subcommand {
            ProfileCommand::Analysis(analysis) => &analysis.profile_common_opts,
            ProfileCommand::Loading(loading) => &loading.profile_common_opts,
            ProfileCommand::Bxl(bxl) => &bxl.profile_common_opts,
        }
    }
}

#[async_trait(?Send)]
impl StreamingCommand for ProfileSubcommand {
    const COMMAND_NAME: &'static str = "profile";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = ctx.client_context(matches, &self)?;

        let destination_path = self.common_opts().output.resolve(&ctx.working_dir);

        let profile_mode = self.common_opts().mode;

        let destination_path = destination_path.into_string()?;

        let console_opts = ctx.console_interaction_stream(self.console_opts());

        let profiler = profile_mode.to_proto();

        let profile_opts = match &self.subcommand {
            ProfileCommand::Loading(loading) => ProfileOpts::TargetProfile(TargetProfile {
                target_patterns: loading.buck_opts.target_patterns.clone(),
                action: target_profile::Action::Loading as i32,
                target_cfg: Some(
                    loading
                        .profile_common_opts
                        .target_cfg
                        .target_cfg
                        .target_cfg(),
                ),
                target_universe: loading
                    .profile_common_opts
                    .target_cfg
                    .target_universe
                    .clone(),
                recursive: loading.buck_opts.recursive,
            }),
            ProfileCommand::Analysis(analysis) => ProfileOpts::TargetProfile(TargetProfile {
                target_patterns: analysis.buck_opts.target_patterns.clone(),
                action: target_profile::Action::Analysis as i32,
                target_cfg: Some(
                    analysis
                        .profile_common_opts
                        .target_cfg
                        .target_cfg
                        .target_cfg(),
                ),
                target_universe: analysis
                    .profile_common_opts
                    .target_cfg
                    .target_universe
                    .clone(),
                recursive: analysis.buck_opts.recursive,
            }),
            ProfileCommand::Bxl(bxl) => {
                if !bxl
                    .profile_common_opts
                    .target_cfg
                    .target_universe
                    .is_empty()
                {
                    return Err::<(), _>(buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "BXL profile does not support target universe"
                    ))
                    .into();
                }
                ProfileOpts::BxlProfile(BxlProfile {
                    bxl_label: bxl.bxl_opts.bxl_label.clone(),
                    bxl_args: bxl.bxl_opts.bxl_args.clone(),
                    target_cfg: Some(bxl.profile_common_opts.target_cfg.target_cfg.target_cfg()),
                })
            }
        };

        let request = ProfileRequest {
            context: Some(context),
            profile_opts: Some(profile_opts),
            destination_path,
            profile_mode: profiler as i32,
        };

        let response = buckd
            .with_flushing()
            .profile(
                request,
                events_ctx,
                console_opts,
                &mut NoPartialResultHandler,
            )
            .await??;

        let ProfileResponse {
            elapsed,
            total_retained_bytes,
        } = response;

        let elapsed = elapsed
            .internal_error("Missing duration")
            .and_then(|d| {
                Duration::try_from(d).map_err(|_| {
                    buck2_error::buck2_error!(buck2_error::ErrorTag::Input, "Duration is negative")
                })
            })
            .buck_error_context("Elapsed is invalid")?;

        buck2_client_ctx::println!(
            "Starlark {:?} profile has been written to {}",
            profile_mode,
            self.common_opts().output.display(),
        )?;
        buck2_client_ctx::println!("Elapsed: {:.3}s", elapsed.as_secs_f64())?;
        buck2_client_ctx::println!("Total retained bytes: {}", total_retained_bytes)?;

        ExitResult::success()
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        &self.common_opts().common_opts.console_opts
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_opts().common_opts.event_log_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        &self.common_opts().common_opts.config_opts
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        &self.common_opts().common_opts.starlark_opts
    }
}
