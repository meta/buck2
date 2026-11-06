/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::future::Future;
use std::process::Stdio;
use std::time::Duration;
use std::time::SystemTime;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::daemon::client::connect::BuckdProcessInfo;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::thread_dump::thread_dump_command;
use buck2_client_ctx::upload_re_logs::upload_re_logs;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_common::manifold::Bucket;
use buck2_common::manifold::ManifoldClient;
use buck2_data::InstantEvent;
use buck2_data::RageResult;
use buck2_data::instant_event::Data;
use buck2_error::BuckErrorContext;
use buck2_event_log::file_names::do_find_log_by_trace_id;
use buck2_event_log::file_names::get_local_logs;
use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::read::EventLogSummary;
use buck2_events::BuckEvent;
use buck2_events::sink::remote::RemoteEventConfig;
use buck2_events::sink::remote::RemoteEventSink;
use buck2_events::sink::remote::new_remote_event_sink_if_enabled;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::IntentionallyStdHashMap;
use buck2_util::process::async_background_command;
use buck2_wrapper_common::invocation_id::TraceId;
use derive_more::Display;
use dupe::Dupe;
use futures::future::FutureExt;
use futures::future::LocalBoxFuture;
use serde::Serialize;
use superconsole::Stdin;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

use crate::manifold::file_to_manifold;
use crate::manifold::manifold_leads;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum RageError {
    #[error("Failed to get a valid user selection")]
    InvalidSelectionError,
    #[error("Failed to find the logs for command")]
    LogNotFoundError,
    #[error("Pastry command failed with code '{0}' and error '{1}' ")]
    PastryCommandError(i32, String),
}

#[derive(Debug, clap::Parser)]
#[clap(
    name = "rage",
    about = "Record information about the previous failed buck2 command",
    group = clap::ArgGroup::new("invocation").multiple(false)
)]
pub struct RageCommand {
    /// Stop collecting information after `<timeout>` seconds
    #[clap(long, default_value = "120")]
    timeout: u64,
    /// Use value 0 to select last invocation, 1 to select second to last and so on
    #[clap(long, group = "invocation")]
    invocation_offset: Option<usize>,
    /// Select invocation directly using the invocation's UUID
    #[clap(long, group = "invocation")]
    invocation_id: Option<TraceId>,
    /// Collect rage report about buck2 in general, not about specific invocation
    #[clap(long, group = "invocation")]
    no_invocation: bool,
    /// We may want to omit paste if this is not a user
    /// or is called in a machine with no pastry command
    #[clap(long)]
    no_paste: bool,
}

impl RageCommand {
    pub fn exec(self, _matches: BuckArgMatches<'_>, ctx: ClientCommandContext<'_>) -> ExitResult {
        buck2_core::facebook_only();

        ctx.with_runtime(|ctx| async move {
            self.exec_impl(ctx).await?;
            ExitResult::success()
        })
    }

    async fn exec_impl(self, mut ctx: ClientCommandContext<'_>) -> buck2_error::Result<()> {
        let paths = ctx.paths()?;
        let daemon_dir = paths.daemon_dir()?;
        // Resolved before anything else borrows `ctx`; cheap, and it needs no daemon.
        let settings = ctx.settings_report().map(|report| report.render(true));
        let stderr_path = daemon_dir.buckd_stderr();
        let re_logs_dir = ctx.paths()?.re_logs_dir();
        let logdir = paths.log_dir();
        let dice_dump_dir = paths.dice_dump_dir();

        let client_ctx = ctx.empty_client_context("rage")?;

        // Don't fail the rage if you can't figure out whether to do vpnless.
        let manifold = ManifoldClient::new().await?;

        let rage_id = TraceId::new();
        let mut manifold_id = format!("{rage_id}");
        let sink = create_scribe_sink(&ctx)?;

        buck2_client_ctx::eprintln!(
            "Data collection will terminate after {} seconds (override with --timeout param)",
            self.timeout
        )?;

        // If there is a daemon, start connecting.
        let info = BuckdProcessInfo::load(&daemon_dir);

        let buckd = match &info {
            Ok(info) => async { info.create_channel().await?.upgrade().await }.boxed_local(),
            Err(e) => futures::future::ready(Err(e.dupe())).boxed_local(),
        }
        .shared();

        let selected_invocation = maybe_select_invocation(ctx.stdin(), &logdir, &self).await?;
        let invocation_id = get_trace_id(&selected_invocation).await?;
        if let Some(ref invocation_id) = invocation_id {
            manifold_id = format!("{invocation_id}_{manifold_id}");
        }

        buck2_client_ctx::eprintln!("Collecting debug info...")?;

        let thread_dump = self.section("Thread dump", || {
            upload_thread_dump(&info, &manifold, &manifold_id)
        });
        let build_info_command = self.skippable_section(
            "Associated invocation info",
            selected_invocation
                .as_ref()
                .map(|inv| || crate::build_info::get(inv)),
        );

        let (thread_dump, build_info) = tokio::join!(
            // Get thread dump before making any new connections to daemon (T159606309)
            thread_dump,
            // We need the RE session ID from here to upload RE logs
            build_info_command
        );

        let system_info_command = self.section("System info", crate::system_info::get);
        let settings_command = self.section("Buck settings", || async { settings });
        let daemon_stderr_command = self.section("Daemon stderr", || {
            upload_daemon_stderr(stderr_path, &manifold, &manifold_id)
        });
        let hg_snapshot_id_command =
            self.section("Source control", crate::source_control::get_info);
        let dice_dump_command = self.section("Dice dump", || async {
            crate::dice::upload_dice_dump(
                buckd.clone().await?,
                dice_dump_dir,
                &client_ctx,
                &manifold,
                &manifold_id,
            )
            .await
        });
        let materializer_state = self.section("Materializer state", || {
            crate::materializer::upload_materializer_data(
                buckd.clone(),
                &client_ctx,
                &manifold,
                &manifold_id,
                MaterializerRageUploadData::State,
            )
        });
        let materializer_fsck = self.section("Materializer fsck", || {
            crate::materializer::upload_materializer_data(
                buckd.clone(),
                &client_ctx,
                &manifold,
                &manifold_id,
                MaterializerRageUploadData::Fsck,
            )
        });
        let event_log_command = self.skippable_section(
            "Event log upload",
            selected_invocation
                .as_ref()
                .map(|path| || upload_event_logs(path, &manifold, &manifold_id)),
        );

        let re_logs_command = self.skippable_section(
            "RE logs upload",
            build_info
                .get_field(|o| o.re_session_id.clone())
                .map(|id| || upload_re_logs_impl(&manifold, &re_logs_dir, id)),
        );

        let (
            system_info,
            settings,
            daemon_stderr_dump,
            hg_snapshot_id,
            dice_dump,
            materializer_state,
            materializer_fsck,
            event_log_dump,
            re_logs,
        ) = tokio::join!(
            system_info_command,
            settings_command,
            daemon_stderr_command,
            hg_snapshot_id_command,
            dice_dump_command,
            materializer_state,
            materializer_fsck,
            event_log_command,
            re_logs_command
        );
        let sections = vec![
            build_info.to_string(),
            system_info.to_string(),
            settings.to_string(),
            daemon_stderr_dump.to_string(),
            hg_snapshot_id.to_string(),
            dice_dump.to_string(),
            materializer_state.to_string(),
            materializer_fsck.to_string(),
            thread_dump.to_string(),
            event_log_dump.to_string(),
            re_logs.to_string(),
        ];
        output_rage(self.no_paste, &sections.join("")).await?;

        self.send_to_scuba(
            sink,
            invocation_id,
            system_info,
            daemon_stderr_dump,
            hg_snapshot_id,
            dice_dump,
            materializer_state,
            materializer_fsck,
            thread_dump,
            event_log_dump,
            build_info,
            re_logs,
        )
        .await?;
        Ok(())
    }

    async fn send_to_scuba(
        &self,
        sink: Option<RemoteEventSink>,
        invocation_id: Option<TraceId>,
        system_info: RageSection<crate::system_info::SystemInfo>,
        daemon_stderr_dump: RageSection<String>,
        hg_snapshot_id: RageSection<String>,
        dice_dump: RageSection<String>,
        materializer_state: RageSection<String>,
        materializer_fsck: RageSection<String>,
        thread_dump: RageSection<String>,
        event_log_dump: RageSection<String>,
        build_info: RageSection<crate::build_info::BuildInfo>,
        re_logs: RageSection<String>,
    ) -> buck2_error::Result<()> {
        let dice_dump = dice_dump.output();
        let materializer_state = materializer_state.output();
        let materializer_fsck = materializer_fsck.output();
        let thread_dump = thread_dump.output();
        let daemon_stderr_dump = daemon_stderr_dump.output();
        let hg_snapshot_id = hg_snapshot_id.output();
        let event_log_dump = event_log_dump.output();
        let re_logs = re_logs.output();
        let invocation_id2 = invocation_id
            .clone()
            .map(|inv| inv.to_string())
            .unwrap_or_default();

        let mut string_data: IntentionallyStdHashMap<String, _> = [
            ("dice_dump", dice_dump.clone()),
            ("materializer_state", materializer_state.clone()),
            ("materializer_fsck", materializer_fsck.clone()),
            ("thread_dump", thread_dump.clone()),
            ("daemon_stderr_dump", daemon_stderr_dump.clone()),
            ("hg_snapshot_id", hg_snapshot_id.clone()),
            ("invocation_id", invocation_id2.clone()),
            ("event_log_dump", event_log_dump.clone()),
            ("re_logs", re_logs.clone()),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();

        let command = build_info.get_field(|o| Some(o.command.to_owned()));
        let buck2_revision = build_info.get_field(|o| Some(o.buck2_revision.to_owned()));
        let username = system_info.get_field(|o| o.username.to_owned());
        let hostname = system_info.get_field(|o| o.hostname.to_owned());
        let os = system_info.get_field(|o| Some(o.os.to_owned()));
        let os_version = system_info.get_field(|o| o.os_version.to_owned());

        insert_if_some(&mut string_data, "command", command.clone());
        insert_if_some(&mut string_data, "buck2_revision", buck2_revision.clone());
        insert_if_some(&mut string_data, "username", username.clone());
        insert_if_some(&mut string_data, "hostname", hostname.clone());
        insert_if_some(&mut string_data, "os", os.clone());
        insert_if_some(&mut string_data, "os_version", os_version.clone());

        let mut int_data = IntentionallyStdHashMap::new();
        let daemon_uptime_s = build_info.get_field(|o| o.daemon_uptime_s);
        insert_if_some(&mut int_data, "daemon_uptime_s", daemon_uptime_s);

        let timestamp =
            build_info.get_field(|o| Some(SystemTime::from(o.timestamp.timestamp()).into()));
        let command_duration = build_info.get_field(|o| {
            Some(prost_types::Duration {
                seconds: o.command_duration?.as_secs() as i64,
                nanos: o.command_duration?.subsec_nanos() as i32,
            })
        });

        // We store in Ent via Ingress that rage was run for specific invocation
        if let Some(invocation_id) = invocation_id {
            dispatch_result_event(
                sink.as_ref(),
                &invocation_id,
                RageResult {
                    // TODO iguridi: remove string_data and int_data
                    string_data,
                    int_data,
                    timestamp,
                    daemon_uptime_s: daemon_uptime_s.map(|s| s as i64),
                    command_duration,
                    dice_dump,
                    materializer_state,
                    materializer_fsck,
                    thread_dump,
                    daemon_stderr_dump,
                    hg_snapshot_id,
                    invocation_id: invocation_id2,
                    event_log_dump,
                    re_logs,
                    command,
                    buck2_revision,
                    username,
                    hostname,
                    os,
                    os_version,
                },
            )
            .await?;
        }
        Ok(())
    }

    fn section<'a, Fut, T>(
        &'a self,
        title: &'a str,
        command: impl FnOnce() -> Fut,
    ) -> LocalBoxFuture<'a, RageSection<T>>
    where
        Fut: Future<Output = buck2_error::Result<T>> + 'a,
        T: std::fmt::Display + 'a,
    {
        let timeout = Duration::from_secs(self.timeout);
        RageSection::get(title, timeout, command)
    }

    fn skippable_section<'a, Fut, T>(
        &'a self,
        title: &'a str,
        command: Option<impl FnOnce() -> Fut>,
    ) -> LocalBoxFuture<'a, RageSection<T>>
    where
        Fut: Future<Output = buck2_error::Result<T>> + 'a,
        T: std::fmt::Display + 'a,
    {
        let timeout = Duration::from_secs(self.timeout);
        RageSection::get_skippable(title, timeout, command)
    }

    pub fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }
}

#[derive(Debug, PartialEq, Serialize)]
struct RageSection<T> {
    title: String,
    status: CommandStatus<T>,
}

#[derive(Display)]
pub enum MaterializerRageUploadData {
    #[display("state")]
    State,
    #[display("fsck")]
    Fsck,
}

#[derive(Debug, PartialEq, Serialize)]
enum CommandStatus<T> {
    Success { output: T },
    Failure { error: String },
    Timeout,
    Skipped,
}

impl<'a, T> RageSection<T>
where
    T: std::fmt::Display + 'a,
{
    fn get<Fut>(
        title: &str,
        timeout: Duration,
        command: impl FnOnce() -> Fut,
    ) -> LocalBoxFuture<'a, Self>
    where
        Fut: Future<Output = buck2_error::Result<T>> + 'a,
    {
        let fut = command();
        let title = title.to_owned();
        async move {
            let status = match tokio::time::timeout(timeout, fut).await {
                Err(_) => CommandStatus::Timeout,
                Ok(Ok(output)) => CommandStatus::Success { output },
                Ok(Err(e)) => CommandStatus::Failure {
                    error: format!("Error: {e:?}"),
                },
            };
            RageSection { title, status }
        }
        .boxed_local()
    }

    fn get_skippable<Fut>(
        title: &str,
        timeout: Duration,
        command: Option<impl FnOnce() -> Fut>,
    ) -> LocalBoxFuture<'a, Self>
    where
        Fut: Future<Output = buck2_error::Result<T>> + 'a,
    {
        if let Some(command) = command {
            Self::get(title, timeout, command)
        } else {
            let status = CommandStatus::Skipped;
            let title = title.to_owned();
            async { RageSection { title, status } }.boxed_local()
        }
    }

    fn output(&self) -> String {
        match &self.status {
            CommandStatus::Success { output } => output.to_string(),
            CommandStatus::Failure { error } => error.to_owned(),
            CommandStatus::Timeout => "Timeout".to_owned(),
            CommandStatus::Skipped => "Skipped".to_owned(),
        }
    }

    fn get_field<D>(&self, extract_field: impl FnOnce(&T) -> Option<D>) -> Option<D> {
        match &self.status {
            CommandStatus::Success { output } => extract_field(output),
            _ => None,
        }
    }

    fn pretty_print_section(
        &self,
        f: &mut fmt::Formatter,
        content: String,
    ) -> Result<(), std::fmt::Error> {
        let content_divider = "-".repeat(30);
        write!(
            f,
            "{title}\n{content_divider}\n{content}\n\n\n",
            title = self.title
        )
    }
}

impl<T> fmt::Display for RageSection<T>
where
    T: std::fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.pretty_print_section(f, self.output())
    }
}

fn insert_if_some<D>(data: &mut IntentionallyStdHashMap<String, D>, key: &str, value: Option<D>) {
    if let Some(value) = value {
        data.insert(key.to_owned(), value);
    }
}

async fn upload_daemon_stderr(
    path: AbsNormPathBuf,
    manifold: &ManifoldClient,
    manifold_id: &str,
) -> buck2_error::Result<String> {
    file_to_manifold(manifold, &path, format!("flat/{manifold_id}.stderr")).await
}

async fn upload_event_logs(
    path: &EventLogPathBuf,
    manifold: &ManifoldClient,
    manifold_id: &str,
) -> buck2_error::Result<String> {
    let filename = format!("flat/{}-event_log{}", manifold_id, path.extension());
    file_to_manifold(manifold, path.path(), filename).await
}

async fn upload_re_logs_impl(
    manifold: &ManifoldClient,
    re_logs_dir: &AbsNormPath,
    re_session_id: String,
) -> buck2_error::Result<String> {
    let bucket = Bucket::RAGE_DUMPS;
    let filename = format!("flat/{}-re_logs.zst", re_session_id);
    upload_re_logs(manifold, bucket, re_logs_dir, &re_session_id, &filename).await?;

    Ok(manifold_leads(&bucket, filename))
}

async fn dispatch_result_event(
    sink: Option<&RemoteEventSink>,
    rage_id: &TraceId,
    result: RageResult,
) -> buck2_error::Result<()> {
    let data = Some(Data::RageResult(Box::new(result)));
    dispatch_event_to_scribe(sink, rage_id, InstantEvent { data }).await?;
    Ok(())
}

async fn dispatch_event_to_scribe(
    sink: Option<&RemoteEventSink>,
    trace_id: &TraceId,
    event: InstantEvent,
) -> buck2_error::Result<()> {
    if let Some(sink) = sink {
        let _res = sink
            .send_now(BuckEvent::new(
                SystemTime::now(),
                trace_id.to_owned(),
                None,
                None,
                event.into(),
            ))
            .await;
    }
    Ok(())
}

#[allow(unused_variables)] // Conditional compilation
fn create_scribe_sink(ctx: &ClientCommandContext) -> buck2_error::Result<Option<RemoteEventSink>> {
    // TODO(swgiillespie) scribe_logging is likely the right feature for this, but we should be able to inject a sink
    // without using configurations at the call site
    let config = buck2_client_ctx::remote_sink_config::with_buckconfig_overrides(
        ctx.maybe_paths()?,
        RemoteEventConfig::default(),
    );
    new_remote_event_sink_if_enabled(ctx.fbinit(), config)
}

async fn maybe_select_invocation(
    stdin: &mut Stdin,
    logdir: &AbsNormPathBuf,
    command: &RageCommand,
) -> buck2_error::Result<Option<EventLogPathBuf>> {
    if command.no_invocation {
        return Ok(None);
    };

    if let Some(trace_id) = &command.invocation_id {
        return Ok(Some(do_find_log_by_trace_id(logdir, trace_id)?));
    }

    let logs = get_local_logs(logdir)?;
    let mut logs = logs
        .into_iter()
        .rev() // newest first
        .collect::<Vec<_>>();
    if logs.is_empty() {
        return Ok(None);
    }
    let index = log_index(stdin, &logs, command.invocation_offset).await?;
    if index >= logs.len() {
        return Err(RageError::LogNotFoundError.into());
    }
    Ok(Some(logs.swap_remove(index)))
}

async fn log_index(
    stdin: &mut Stdin,
    logs: &[EventLogPathBuf],
    invocation_offset: Option<usize>,
) -> Result<usize, buck2_error::Error> {
    let index = match invocation_offset {
        Some(i) => i,
        None => {
            let mut stdin = BufReader::new(stdin);
            user_prompt_select_log(&mut stdin, logs).await?
        }
    };
    Ok(index)
}

async fn user_prompt_select_log(
    stdin: impl AsyncBufRead + Unpin,
    logs: &[EventLogPathBuf],
) -> buck2_error::Result<usize> {
    buck2_client_ctx::eprintln!("Which buck invocation would you like to report?\n")?;
    let logs_summary = buck2_util::future::join_all(
        logs.iter()
            .map(|log_path| async move { log_path.get_summary().await.ok() }),
    )
    .await;
    for (index, log_summary) in logs_summary.iter().enumerate() {
        print_log_summary(index, log_summary)?;
    }
    buck2_client_ctx::eprintln!()?;
    let prompt = format!(
        "Invocation: (type a number between 0 and {}) ",
        logs_summary.len() - 1
    );
    let selection = get_user_selection(stdin, &prompt, |i| i < logs_summary.len()).await?;

    buck2_client_ctx::eprintln!("Selected invocation {}\n", selection)?;
    Ok(selection)
}

async fn get_user_selection<P>(
    mut stdin: impl AsyncBufRead + Unpin,
    prompt: &str,
    predicate: P,
) -> buck2_error::Result<usize>
where
    P: Fn(usize) -> bool,
{
    buck2_client_ctx::eprint!("{}", prompt)?;

    let mut input = String::new();
    stdin.read_line(&mut input).await?;

    match input.trim().parse() {
        Ok(selection) if predicate(selection) => Ok(selection),
        _ => Err(RageError::InvalidSelectionError.into()),
    }
}

fn print_log_summary(
    index: usize,
    log_summary: &Option<EventLogSummary>,
) -> buck2_error::Result<()> {
    if let Some(log_summary) = log_summary {
        let cmd = crate::build_info::format_cmd(&log_summary.invocation);

        let timestamp = jiff::Timestamp::try_from(log_summary.timestamp)
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
            .to_zoned(jiff::tz::TimeZone::system());
        Ok(buck2_client_ctx::eprintln!(
            "{:<7} {}    {}",
            format!("[{}].", index),
            timestamp.strftime("%c %Z"),
            cmd
        )?)
    } else {
        Ok(buck2_client_ctx::eprintln!(
            "{:<7} <<Unable to display information>>",
            format!("[{}].", index),
        )?)
    }
}

async fn output_rage(no_paste: bool, output: &str) -> buck2_error::Result<()> {
    if no_paste {
        buck2_client_ctx::println!("{}", output)?;
    } else {
        match generate_paste("Buck2 Rage", output).await {
            Err(e) => {
                buck2_client_ctx::eprintln!(
                    "Failed to generate paste automatically with error \"{:?}\".
                    Please create paste manually with `bunnylol paste` using the output below:\n\n\n",
                    e
                )?;
                buck2_client_ctx::println!("{}", output)?;
            }
            Ok(paste) => buck2_client_ctx::eprintln!(
                "\nPlease post in https://fb.workplace.com/groups/buck2users with the following link:\n\n{}\n",
                paste
            )?,
        }
    };
    Ok(())
}

async fn generate_paste(title: &str, content: &str) -> buck2_error::Result<String> {
    let mut pastry = async_background_command("pastry")
        .args(["--title", title])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .buck_error_context("Failed to spawn pastry")?;
    let mut stdin = pastry.stdin.take().expect("Stdin should open");

    let writer = async move {
        stdin
            .write_all(content.as_bytes())
            .await
            .buck_error_context("Error writing to pastry")
    };

    let reader = async move {
        let output = tokio::time::timeout(Duration::from_secs(10), pastry.wait_with_output())
            .await
            .buck_error_context("Pastry command timeout, make sure you are on Lighthouse/VPN")?
            .buck_error_context("Error reading pastry output")?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output
                .status
                .code()
                .ok_or_else(|| RageError::PastryCommandError(1, error.clone()))?;
            return Err(RageError::PastryCommandError(code, error).into());
        }
        let output =
            String::from_utf8(output.stdout).buck_error_context("Error reading pastry output")?;
        Ok(output)
    };

    let ((), paste) = futures::future::try_join(writer, reader).await?;

    Ok(paste)
}

async fn upload_thread_dump(
    buckd: &buck2_error::Result<BuckdProcessInfo<'_>>,
    manifold: &ManifoldClient,
    manifold_id: &String,
) -> buck2_error::Result<String> {
    let buckd = buckd.as_ref().map_err(|e| e.clone())?;
    let command = thread_dump_command(buckd)?
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .buck_error_context("Failed to spawn lldb command")?
        .wait_with_output()
        .await?;

    if command.status.success() {
        let manifold_filename = format!("flat/{manifold_id}_thread_dump");
        crate::manifold::buf_to_manifold(manifold, &command.stdout, manifold_filename).await
    } else {
        let stderr = &command.stderr;
        Ok(String::from_utf8_lossy(stderr).to_string())
    }
}

async fn get_trace_id(
    invocation: &Option<EventLogPathBuf>,
) -> buck2_error::Result<Option<TraceId>> {
    let invocation_id = match invocation {
        None => None,
        Some(invocation) => Some(invocation.uuid_from_filename()?),
    };
    Ok(invocation_id)
}
