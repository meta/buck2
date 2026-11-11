/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;

use async_trait::async_trait;
use buck2_cli_proto::CounterWithExamples;
use buck2_cli_proto::TestRequest;
use buck2_cli_proto::TestSessionOptions;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::common::CommonBuildConfigurationOptions;
use buck2_client_ctx::common::CommonCommandOptions;
use buck2_client_ctx::common::CommonEventLogOptions;
use buck2_client_ctx::common::CommonStarlarkOptions;
use buck2_client_ctx::common::build::CommonBuildOptions;
use buck2_client_ctx::common::target_cfg::TargetCfgOptions;
use buck2_client_ctx::common::timeout::CommonTimeoutOptions;
use buck2_client_ctx::common::ui::CommonConsoleOptions;
use buck2_client_ctx::daemon::client::BuckdClientConnector;
use buck2_client_ctx::daemon::client::NoPartialResultHandler;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::final_console::FinalConsole;
use buck2_client_ctx::output_destination_arg::OutputDestinationArg;
use buck2_client_ctx::path_arg::PathArg;
use buck2_client_ctx::streaming::StreamingCommand;
use buck2_client_ctx::subscribers::superconsole::test::TestCounterColumn;
use buck2_client_ctx::subscribers::superconsole::test::span_from_build_failure_count;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ExitCode;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::working_dir::AbsWorkingDir;
use superconsole::Line;
use superconsole::Span;

use crate::commands::build::print_buck_ui;
use crate::commands::build::print_build_result;

fn forward_output_to_path(
    output: &str,
    path_arg: &PathArg,
    working_dir: &AbsWorkingDir,
) -> buck2_error::Result<()> {
    fs_util::write(path_arg.resolve(working_dir), output)
        // input path from --test-executor-stderr=FILEPATH
        .categorize_input()
        .buck_error_context("Failed to write test executor output to path")
}

fn print_error_counter(
    console: &FinalConsole,
    counter: &CounterWithExamples,
    error_type: &str,
    symbol: &str,
) -> buck2_error::Result<()> {
    if counter.count > 0 {
        console.print_error(&format!("{} {}", counter.count, error_type))?;
        for test_name in &counter.example_tests {
            console.print_error(&format!("  {symbol} {test_name}"))?;
        }
        if counter.count > counter.max {
            console.print_error(&format!(
                "  ...and {} more not shown...",
                counter.count - counter.max
            ))?;
        }
    }
    Ok(())
}

/// Check if we should warn about potentially misplaced --include/--exclude flags.
/// Returns Some(suspicious_labels) if warning should be shown, where suspicious_labels
/// are label values that look like they might be target patterns (contain '/' or ':').
fn should_warn_about_flag_position(
    patterns: &[String],
    include: &[String],
    exclude: &[String],
) -> Option<Vec<String>> {
    if patterns.is_empty() && (!include.is_empty() || !exclude.is_empty()) {
        let suspicious_labels: Vec<String> = include
            .iter()
            .chain(exclude.iter())
            .filter(|label| label.contains('/') || label.contains(':'))
            .cloned()
            .collect();

        Some(suspicious_labels)
    } else {
        None
    }
}

#[derive(Debug, clap::Parser)]
#[clap(name = "test", about = "Build and test the specified targets")]
pub struct TestCommand {
    #[clap(
        long = "exclude",
        num_args = 1..,
        help = "Labels on targets to exclude from tests"
    )]
    exclude: Vec<String>,

    #[clap(
        long = "include",
        alias = "labels",
        help = "Labels on targets to include from tests. Prefixing with `!` means to exclude. First match wins unless overridden by `always-exclude` flag.\n\
If any include pattern without a `!` prefix is present, regardless of whether exclude patterns are present, then all targets are by default excluded unless explicitly included. Include patterns that all carry a `!` prefix only exclude.",
        num_args=1..,
    )]
    include: Vec<String>,

    #[clap(
        long = "always-exclude",
        alias = "always_exclude",
        help = "Whether to always exclude if the label appears in `exclude`, regardless of which appears first"
    )]
    always_exclude: bool,

    #[clap(
        long = "build-filtered",
        help = "Whether to build tests that are excluded via labels."
    )]
    build_filtered_targets: bool, // TODO(bobyf) this flag should always override the buckconfig option when we use it

    /// Will allow tests that are compatible with RE (setup to run from the repo root and
    /// use relative paths) to run from RE.
    #[clap(long, group = "re_options", alias = "unstable-allow-tests-on-re")]
    unstable_allow_compatible_tests_on_re: bool,

    /// Will run tests to on RE even if they are missing required settings (running from the root +
    /// relative paths). Those required settings just get overridden.
    #[clap(long, group = "re_options", alias = "unstable-force-tests-on-re")]
    unstable_allow_all_tests_on_re: bool,

    #[clap(name = "TARGET_PATTERNS", help = "Patterns to test", value_hint = clap::ValueHint::Other)]
    patterns: Vec<String>,

    /// Writes the test executor stdout to the provided path
    ///
    /// --test-executor-stdout=- will write to stdout
    ///
    /// --test-executor-stdout=FILEPATH will write to the provided filepath, overwriting the current
    /// file if it exists
    ///
    /// By default the test executor's stdout stream is captured
    #[clap(long)]
    test_executor_stdout: Option<OutputDestinationArg>,

    /// Normally testing will follow the `tests` attribute of all targets, to find their associated tests.
    /// When passed, this flag will disable that, and only run the directly supplied targets.
    #[clap(long)]
    ignore_tests_attribute: bool,

    /// Writes the test executor stderr to the provided path
    ///
    /// --test-executor-stderr=- will write to stderr
    ///
    /// --test-executor-stderr=FILEPATH will write to the provided filepath, overwriting the current
    /// file if it exists
    ///
    /// By default test executor's stderr stream is captured
    #[clap(long)]
    test_executor_stderr: Option<OutputDestinationArg>,

    /// Additional arguments passed to the test executor.
    ///
    /// Test executor is expected to have `--env` flag to pass environment variables.
    /// Can be used like this:
    ///
    /// buck2 test //foo:bar -- --env PRIVATE_KEY=123
    #[clap(name = "TEST_EXECUTOR_ARGS", raw = true)]
    test_executor_args: Vec<String>,

    /// Also build DefaultInfo provider, which is what `buck2 build` builds.
    ///
    /// This overrides the `buck2.test_builds_targets` buckconfig.
    #[clap(long, group = "default-info")]
    build_default_info: bool,

    /// Do not build DefaultInfo provider.
    ///
    /// This overrides the `buck2.test_builds_targets` buckconfig.
    #[clap(long, group = "default-info")]
    skip_default_info: bool,

    /// Also build RunInfo provider, which builds artifacts needed for `buck2 run`.
    ///
    /// This overrides the `buck2.test_builds_targets` buckconfig.
    #[clap(long, group = "run-info")]
    build_run_info: bool,

    /// Do not build RunInfo provider.
    ///
    /// This overrides the `buck2.test_builds_targets` buckconfig.
    #[clap(long, group = "run-info")]
    skip_run_info: bool,

    /// This option does nothing. It is here to keep compatibility with Buck1 and ci
    #[clap(long = "deep", hide = true)]
    _deep: bool,

    // ignored. only for e2e tests. compatibility with v1.
    #[clap(long = "xml", hide = true)]
    _xml: Option<String>,

    #[clap(flatten)]
    build_opts: CommonBuildOptions,

    #[clap(flatten)]
    target_cfg: TargetCfgOptions,

    #[clap(flatten)]
    timeout_options: CommonTimeoutOptions,

    /// Write the test session ID into this file
    #[clap(long, value_name = "PATH")]
    write_test_id: Option<PathArg>,

    #[clap(flatten)]
    common_opts: CommonCommandOptions,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = TestExecutor)]
enum ExecutorError {
    #[buck2(tag = TestRunnerInternal)]
    #[error("Internal error in test runner")]
    InternalError,
    #[buck2(tag = Input)]
    #[error("Tests completed with cancellations")]
    CompletedWithCancellations,
    #[buck2(tag = Input)]
    #[error("Tests passed with warnings")]
    PassWithWarnings,
    #[error(transparent)]
    Fail(TestStatusError),
    #[error(transparent)]
    NeedsBaseRevisionRetry(TestStatusError),
    #[error(transparent)]
    NeedsAdditionalVerification(TestStatusError),
    #[buck2(tag = TestRunnerUnknownExitCode)]
    #[error("Test Executor Failed with exit code {0}")]
    UnexpectedExitCode(i32),
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = TestExecutor)]
enum TestStatusError {
    #[error("Test execution completed but the tests failed")]
    #[buck2(tag = TestFailed)]
    TestFailed,
    #[error("Test listing failed")]
    #[buck2(tag = TestListingFailed)]
    ListingFailed,
    #[error("Fatal error encountered during test execution")]
    #[buck2(tag = TestFatal)]
    Fatal,
    #[error("Infra Failure error encountered during test execution")]
    #[buck2(tag = TestInfraFailure)]
    InfraFailure,
    #[error("Test execution completed but some tests timed out")]
    #[buck2(tag = TestTimeout)]
    TestTimeout,
    #[error("Unexpected failure during test execution")]
    #[buck2(tag = TestStatusUnknown)]
    Unknown,
}

impl ExecutorError {
    fn new(
        exit_code: i32,
        test_statuses: &buck2_cli_proto::test_response::TestStatuses,
    ) -> Option<Self> {
        let status_error = TestStatusError::new(test_statuses);
        // exit codes from tpx::outcome::RunVerdict
        match exit_code {
            0 => None,
            1 => Some(Self::InternalError),
            2 => Some(Self::CompletedWithCancellations),
            32 => Some(Self::Fail(status_error)),
            42 => Some(Self::NeedsBaseRevisionRetry(status_error)),
            43 => Some(Self::NeedsAdditionalVerification(status_error)),
            64 => Some(Self::PassWithWarnings),
            _ => Some(Self::UnexpectedExitCode(exit_code)),
        }
    }
}

impl TestStatusError {
    fn new(test_statuses: &buck2_cli_proto::test_response::TestStatuses) -> Self {
        if let Some(fatal) = &test_statuses.fatals
            && fatal.count > 0
        {
            Self::Fatal
        } else if let Some(infra_failure) = &test_statuses.infra_failure
            && infra_failure.count > 0
        {
            Self::InfraFailure
        } else if let Some(listing_failed) = &test_statuses.listing_failed
            && listing_failed.count > 0
        {
            Self::ListingFailed
        } else if let Some(failed) = &test_statuses.failed
            && failed.count > 0
        {
            Self::TestFailed
        } else if let Some(timed_out) = &test_statuses.timed_out
            && timed_out.count > 0
        {
            Self::TestTimeout
        } else {
            Self::Unknown
        }
    }
}

fn test_executor_error(
    executor_exit_code: i32,
    test_statuses: &buck2_cli_proto::test_response::TestStatuses,
) -> Option<buck2_error::Error> {
    if let Some(error) = ExecutorError::new(executor_exit_code, test_statuses) {
        let exit_code_tag = if let ExecutorError::UnexpectedExitCode(exit_code) = error {
            Some(exit_code.to_string())
        } else {
            None
        };

        let mut error = buck2_error::Error::from(error);
        if let Some(tag) = exit_code_tag {
            error = error.string_tag(&tag);
        }
        Some(error)
    } else {
        None
    }
}

#[async_trait(?Send)]
impl StreamingCommand for TestCommand {
    const COMMAND_NAME: &'static str = "test";

    async fn exec_impl(
        self,
        buckd: &mut BuckdClientConnector,
        matches: BuckArgMatches<'_>,
        ctx: &mut ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        // Warn if no target patterns but label filters are set
        // This usually means the patterns were accidentally consumed as label values
        // NOTE: maybe these should accept just one arg, but that probably breaks users who
        // are doing buck test //... --include myproject myproject2
        if let Some(suspicious_labels) =
            should_warn_about_flag_position(&self.patterns, &self.include, &self.exclude)
        {
            let console = self.common_opts.console_opts.final_console();

            let mut message = String::new();
            writeln!(
                &mut message,
                "No target patterns specified, but --include/--exclude flags are set."
            )
            .unwrap();
            writeln!(
                &mut message,
                "This is likely a mistake: put targets first, then --include/--exclude, since include/exclude consume all remaining args"
            ).unwrap();
            if !suspicious_labels.is_empty() {
                writeln!(
                    &mut message,
                    "hint: The following requested labels look like target patterns: {sus}\n\
                    hint: Try putting them before --include/--exclude.\n\
                    hint: For example: buck2 test //foo --include mylabel",
                    sus = suspicious_labels
                        .iter()
                        .map(|s| format!("'{}'", s))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .unwrap();
            }
            console.print_warning(&message)?;
        }

        let context = ctx.client_context(matches, &self)?;

        let build_default_info = if self.skip_default_info {
            Some(false)
        } else if self.build_default_info {
            Some(true)
        } else {
            None
        };

        let build_run_info = if self.skip_run_info {
            Some(false)
        } else if self.build_run_info {
            Some(true)
        } else {
            None
        };

        let response = buckd
            .with_flushing()
            .test(
                TestRequest {
                    context: Some(context),
                    target_patterns: self.patterns.clone(),
                    target_cfg: Some(self.target_cfg.target_cfg()),
                    test_executor_args: self.test_executor_args,
                    excluded_labels: self.exclude,
                    included_labels: self.include,
                    always_exclude: self.always_exclude,
                    build_filtered_targets: self.build_filtered_targets,
                    // we don't currently have a different flag for this, so just use the build one.
                    concurrency: self.build_opts.num_threads.unwrap_or(0),
                    build_opts: Some(self.build_opts.to_proto()),
                    session_options: Some(TestSessionOptions {
                        allow_re: self.unstable_allow_compatible_tests_on_re
                            || self.unstable_allow_all_tests_on_re,
                        force_use_project_relative_paths: self.unstable_allow_all_tests_on_re,
                        force_run_from_project_root: self.unstable_allow_all_tests_on_re,
                    }),
                    timeout: self.timeout_options.overall_timeout()?,
                    ignore_tests_attribute: self.ignore_tests_attribute,
                    build_default_info,
                    build_run_info,
                },
                events_ctx,
                ctx.console_interaction_stream(&self.common_opts.console_opts),
                &mut NoPartialResultHandler,
            )
            .await??;

        let statuses = response
            .test_statuses
            .as_ref()
            .expect("Daemon to not return empty statuses");

        let listing_failed = statuses
            .listing_failed
            .as_ref()
            .internal_error("Missing `listing_failed`")?;
        let passed = statuses
            .passed
            .as_ref()
            .internal_error("Missing `passed`")?;
        let failed = statuses
            .failed
            .as_ref()
            .internal_error("Missing `failed`")?;
        let timeout = statuses
            .timed_out
            .as_ref()
            .internal_error("Missing `timed_out`")?;
        let fatals = statuses
            .fatals
            .as_ref()
            .internal_error("Missing `fatals`")?;
        let skipped = statuses
            .skipped
            .as_ref()
            .internal_error("Missing `skipped`")?;
        let omitted = statuses
            .omitted
            .as_ref()
            .internal_error("Missing `omitted`")?;
        let infra_failure = statuses
            .infra_failure
            .as_ref()
            .internal_error("Missing `infra failure`")?;

        let console = self.common_opts.console_opts.final_console();
        print_build_result(&console, &response.build_errors)?;
        print_build_result(&console, &response.test_errors)?;

        let build_error_count: u64 = response.build_errors.len().try_into()?;
        if build_error_count != 0 {
            console.print_error(&format!("{} BUILDS FAILED", build_error_count))?;
        }

        print_buck_ui(&console, ctx, events_ctx.used_superconsole)?;

        let mut line = Line::default();
        line.push(Span::new_unstyled_lossy("Tests finished: "));
        if listing_failed.count > 0 {
            line.push(TestCounterColumn::LISTING_FAIL.to_span_from_test_statuses(statuses)?);
            line.push(Span::new_unstyled_lossy(". "));
        }
        let columns = [
            TestCounterColumn::PASS,
            TestCounterColumn::FAIL,
            TestCounterColumn::TIMEOUT,
            TestCounterColumn::FATAL,
            TestCounterColumn::SKIP,
            TestCounterColumn::OMIT,
            TestCounterColumn::INFRA_FAILURE,
        ];
        for column in columns {
            line.push(column.to_span_from_test_statuses(statuses)?);
            line.push(Span::new_unstyled_lossy(". "));
        }
        line.push(span_from_build_failure_count(build_error_count)?);
        console.print_line(&line)?;

        print_error_counter(&console, listing_failed, "LISTINGS FAILED", "⚠")?;
        print_error_counter(&console, failed, "TESTS FAILED", "✗")?;
        print_error_counter(&console, timeout, "TESTS TIMED OUT", "⏱")?;
        print_error_counter(&console, fatals, "TESTS FATALS", "⚠")?;
        print_error_counter(&console, infra_failure, "TESTS Infra Failed", "🛠")?;

        if passed.count
            + failed.count
            + timeout.count
            + fatals.count
            + skipped.count
            + omitted.count
            + infra_failure.count
            == 0
        {
            console.print_warning("NO TESTS RAN")?;
        }

        let info_messages = response.executor_info_messages;
        for message in info_messages {
            console.print_stderr(message.as_str())?;
        }

        match self.test_executor_stderr {
            Some(OutputDestinationArg::Path(path)) => {
                forward_output_to_path(&response.executor_stderr, &path, &ctx.working_dir)?;
            }
            Some(OutputDestinationArg::Stream) => {
                console.print_error(&response.executor_stderr)?;
            }
            None => {}
        }

        if let Some(build_report) = response.serialized_build_report {
            buck2_client_ctx::println!("{}", build_report)?;
        }

        let exit_result = if !response.build_errors.is_empty() {
            // If we had build errors return their exit code.
            ExitResult::from_command_result_errors(response.build_errors)
        } else {
            let mut errors = response.test_errors;
            // Create an error if executor returned non-zero exit code.
            // Error is for tagging and categorization only, not shown to user.
            if let Some(error) = test_executor_error(response.executor_exit_code, statuses) {
                errors.push((&error).into());
            }
            // If exit code is set in response, it should be used and not derived from command errors.
            let exit_code = if let Ok(code) = response.executor_exit_code.try_into() {
                match code {
                    0 => ExitCode::Success,
                    _ => ExitCode::TestRunner(code),
                }
            } else {
                // The exit code isn't an allowable value, so just switch to generic failure
                ExitCode::UnknownFailure
            };
            ExitResult::status_with_emitted_errors(exit_code, errors)
        };

        match self.test_executor_stdout {
            Some(OutputDestinationArg::Path(path)) => {
                forward_output_to_path(&response.executor_stdout, &path, &ctx.working_dir)?;
                exit_result
            }
            Some(OutputDestinationArg::Stream) => {
                exit_result.with_stdout(response.executor_stdout.into_bytes())
            }
            _ => exit_result,
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

    fn write_test_id(&self) -> &Option<PathArg> {
        &self.write_test_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_warn_when_no_patterns_with_include() {
        let result = should_warn_about_flag_position(&[], &["some_label".to_owned()], &[])
            .expect("should warn");
        assert!(result.is_empty());
    }

    #[test]
    fn test_warn_when_no_patterns_with_exclude() {
        let result = should_warn_about_flag_position(&[], &[], &["some_label".to_owned()])
            .expect("should warn");
        assert!(result.is_empty());
    }

    #[test]
    fn test_warn_detects_suspicious_target_pattern_with_slashes() {
        let result = should_warn_about_flag_position(
            &[],
            &["some_label".to_owned(), "//foobar/...".to_owned()],
            &["//foobar2/...".to_owned(), "//foobar2:".to_owned()],
        )
        .expect("should warn");

        assert_eq!(result, vec!["//foobar/...", "//foobar2/...", "//foobar2:"]);
    }

    #[test]
    fn test_no_warn_when_patterns_present() {
        let result = should_warn_about_flag_position(
            &["//target/...".to_owned()],
            &["some_label".to_owned()],
            &["some_other_label".to_owned()],
        );

        assert!(
            result.is_none(),
            "Should not warn when patterns are present"
        );
    }

    #[test]
    fn test_no_warn_when_nothing_specified() {
        let result = should_warn_about_flag_position(&[], &[], &[]);

        assert!(result.is_none(), "Should not warn when nothing specified");
    }
}
