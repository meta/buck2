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
use buck2_cli_proto::InstallRequest;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::command_outcome::CommandOutcome;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::build::CommonBuildOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::streaming::StreamingCommand;

use crate::commands::build::print_buck_ui;

#[derive(Debug, clap::Parser)]
#[clap(name = "install", about = "Build and install an application")]
pub struct InstallCommand {
    #[clap(
        long,
        name = "installer-debug",
        help = "Prints installer output to stderr. It might break superconsole"
    )]
    installer_debug: bool,

    #[clap(flatten)]
    android_install_opts: AndroidInstallOptions,

    #[clap(name = "TARGET", help = "Target to build and install", value_hint = clap::ValueHint::Other)]
    patterns: Vec<String>,

    #[clap(
        name = "INSTALL_ARGS",
        help = "Additional arguments passed to the install when running it",
        raw = true
    )]
    extra_run_args: Vec<String>,

    #[clap(flatten)]
    build_opts: CommonBuildOptions,

    #[clap(flatten)]
    target_cfg: TargetCfgOptions,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

/// Defines install options for Android that exist only for compatibility
/// with buck1, and which are all automatically forwarded to the installer.
#[derive(Debug, clap::Parser)]
struct AndroidInstallOptions {
    #[clap(
        short,
        long,
        help = "Run an Android activity. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    run: bool,

    #[clap(
        short,
        long,
        help = "Use this option to use emulators only on Android. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    emulator: bool,

    #[clap(
        short,
        long,
        help = "Use this option to use real devices only on Android. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    device: bool,

    #[clap(
        short,
        long,
        alias = "udid",
        help = "Use Android device or emulator with specific serial or UDID number. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    serial: Option<String>,

    #[clap(
        short = 'x',
        long,
        help = "Use all connected Android devices and/or emulators (multi-install mode). Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    all_devices: bool,

    #[clap(
        short,
        long,
        help = "Android activity to launch e.g. com.facebook/.LoginActivity. Implies -r. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    activity: Option<String>,

    #[clap(
        short,
        long,
        help = "Android Intent URI to launch e.g. fb://profile. Implies -r. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    intent_uri: Option<String>,

    #[clap(
        short,
        long,
        help = "Have the launched Android process wait for the debugger. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    wait_for_debugger: bool,

    #[clap(
        short,
        long,
        help = "Use this option to uninstall an installed app before installing again. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    uninstall: bool,

    #[clap(
        short,
        long,
        help = "Use this option to Keep user data when uninstalling. Here for compatibility with buck1 - it is automatically forwarded to the installer"
    )]
    keep: bool,
}

#[async_trait(?Send)]
impl StreamingCommand for InstallCommand {
    const COMMAND_NAME: &'static str = "install";
    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let context = ctx.client_context(matches, &self)?;

        let mut extra_run_args: Vec<String> = vec![];
        if self.android_install_opts.run {
            extra_run_args.push("-r".to_owned());
        }
        if self.android_install_opts.emulator {
            extra_run_args.push("-e".to_owned());
        }
        if self.android_install_opts.device {
            extra_run_args.push("-d".to_owned());
        }
        if let Some(serial) = self.android_install_opts.serial {
            extra_run_args.push("-s".to_owned());
            extra_run_args.push(serial);
        }
        if self.android_install_opts.all_devices {
            extra_run_args.push("-x".to_owned());
        }
        if let Some(activity) = self.android_install_opts.activity {
            extra_run_args.push("-a".to_owned());
            extra_run_args.push(activity);
        }
        if let Some(intent_uri) = self.android_install_opts.intent_uri {
            extra_run_args.push("-i".to_owned());
            extra_run_args.push(intent_uri);
        }
        if self.android_install_opts.wait_for_debugger {
            extra_run_args.push("-w".to_owned());
        }
        if self.android_install_opts.uninstall {
            extra_run_args.push("-u".to_owned());
        }
        if self.android_install_opts.keep {
            extra_run_args.push("-k".to_owned());
        }

        // Add the additional run args passed to buck.
        // They are added last to allow for `buck install -- --some-installer-arg -- --arbitrary-app-arg1 --another-app-arg`
        // as otherwise we'll add the above installer options *after* the installer extra args `--` separator.
        extra_run_args.extend(self.extra_run_args.clone());

        let response = buckd
            .with_flushing()
            .install(
                InstallRequest {
                    context: Some(context),
                    target_patterns: self.patterns.clone(),
                    target_cfg: Some(self.target_cfg.target_cfg()),
                    build_opts: Some(self.build_opts.to_proto()),
                    installer_run_args: extra_run_args,
                    installer_debug: self.installer_debug,
                },
                events_ctx,
                ctx.console_interaction_stream(&self.common_opts.console_opts),
                &mut NoPartialResultHandler,
            )
            .await?;
        let console = self.common_opts.console_opts.final_console();
        print_buck_ui(&console, ctx, events_ctx.used_superconsole)?;

        match response {
            CommandOutcome::Success(_) => {
                if self.patterns.is_empty() {
                    console.print_warning("NO BUILD TARGET PATTERNS SPECIFIED")?;
                } else if ctx.verbosity.print_success_message() {
                    console.print_success("INSTALL SUCCEEDED")?;
                }
                ExitResult::success()
            }
            CommandOutcome::Failure(exit_result) => {
                console.print_error("INSTALL FAILED")?;
                exit_result
            }
        }
    }

    fn console_opts(&self) -> &CommonConsoleOptions {
        &self.common_opts.console_opts
    }

    fn event_log_opts(&self) -> &CommonEventLogOptions {
        &self.common_opts.event_log_opts
    }

    fn build_config_opts(&self) -> &CommonBuildConfigurationOptions {
        &self.common_opts.config_opts
    }

    fn starlark_opts(&self) -> &CommonStarlarkOptions {
        &self.common_opts.starlark_opts
    }
}
