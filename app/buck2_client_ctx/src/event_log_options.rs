/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::Stdio;

use buck2_common::init::LogDownloadMethod;
use buck2_common::temp_path::TempPath;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_event_log::file_names::find_log_by_trace_id;
use buck2_event_log::file_names::retrieve_nth_recent_log;
use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::utils::Encoding;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_util::indent::indent;
use buck2_util::process::async_background_command;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;

use crate::client_ctx::ClientCommandContext;
use crate::path_arg::PathArg;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = LogCmd)]
enum EventLogOptionsError {
    #[error("{0} failed; stderr:\n{}", indent("  ", _1))]
    DownloadFailed(String, String),
    #[error("Log not found locally by trace id `{0}`")]
    LogNotFoundLocally(TraceId),
}

#[derive(Debug, Clone, clap::Parser)]
#[clap(group = clap::ArgGroup::new("event_log"))]
pub struct EventLogOptions {
    /// Open the event-log file from a recent command.
    #[clap(long, group = "event_log", value_name = "NUMBER")]
    pub recent: Option<usize>,

    /// Show log by trace id.
    #[clap(long, group = "event_log", value_name = "ID")]
    pub trace_id: Option<TraceId>,

    /// This option does nothing.
    #[clap(long, requires = "trace_id")]
    pub allow_remote: bool,

    /// Do not allow downloading the log from manifold if it's not found locally.
    #[clap(long, requires = "trace_id")]
    pub no_remote: bool,

    /// A path to an event-log file to read from.
    #[clap(group = "event_log", value_name = "PATH")]
    pub path: Option<PathArg>,
}

impl EventLogOptions {
    pub async fn get(
        &self,
        ctx: &ClientCommandContext<'_>,
    ) -> buck2_error::Result<EventLogPathBuf> {
        if let Some(path) = &self.path {
            EventLogPathBuf::infer(path.resolve(&ctx.working_dir))
        } else if let Some(id) = &self.trace_id {
            if let Some(log_path) = find_log_by_trace_id(&ctx.paths()?.log_dir(), id)? {
                Ok(log_path)
            } else if !self.no_remote {
                EventLogPathBuf::infer(self.download_remote_id(id, ctx).await?)
            } else {
                Err(EventLogOptionsError::LogNotFoundLocally(id.dupe()).into())
            }
        } else {
            retrieve_nth_recent_log(
                ctx.paths()
                    .buck_error_context("Error identifying log dir")?,
                self.recent.unwrap_or(0),
            )
        }
    }

    fn random_string() -> String {
        let mut s = String::with_capacity(10);
        for _ in 0..10 {
            s.push(rand::random_range('a'..='z'));
        }
        s
    }

    async fn download_remote_id(
        &self,
        trace_id: &TraceId,
        ctx: &ClientCommandContext<'_>,
    ) -> buck2_error::Result<AbsPathBuf> {
        let log_file_name = FileNameBuf::try_from(format!(
            "{}{}",
            trace_id,
            // TODO(nga): hardcoded default, should at least use the same default buck2 uses,
            //   or better enumerate all the possible suffixes.
            Encoding::PROTO_ZSTD.extensions[0]
        ))?;

        let dl_cache_dir = ctx.paths()?.log_download_cache_dir();
        let log_path = dl_cache_dir.join(&log_file_name);

        if fs_util::try_exists(&log_path)? {
            return Ok(log_path.into_abs_path_buf());
        }

        fs_util::create_dir_all(&dl_cache_dir)?;
        let temp_path = dl_cache_dir.join(FileName::new(&format!(
            "dl.{}.{}.tmp",
            log_file_name,
            Self::random_string()
        ))?);

        // Delete the file on failure.
        let temp_path = TempPath::new_path(temp_path);
        let (command_name, command) = match ctx.log_download_method()? {
            LogDownloadMethod::Manifold => {
                let args = [
                    "get",
                    &format!("buck2_logs/flat/{log_file_name}"),
                    temp_path
                        .path()
                        .as_os_str()
                        .to_str()
                        .internal_error("temp_path is not valid UTF-8")?,
                ];
                crate::eprintln!("Spawning: manifold {}", args.join(" "))?;
                (
                    "Manifold",
                    async_background_command("manifold")
                        .args(args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .spawn()?,
                )
            }
            LogDownloadMethod::Curl(log_url) => {
                let log_url = log_url.trim_end_matches('/');

                let args = [
                    "--fail",
                    "-L",
                    &format!("{log_url}/v1/logs/get/{trace_id}"),
                    "-o",
                    temp_path
                        .path()
                        .as_os_str()
                        .to_str()
                        .internal_error("temp_path is not valid UTF-8")?,
                ];
                crate::eprintln!("Spawning: curl {}", args.join(" "))?;
                (
                    "Curl",
                    async_background_command("curl")
                        .args(args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .spawn()?,
                )
            }
            LogDownloadMethod::None => {
                return Err(EventLogOptionsError::LogNotFoundLocally(trace_id.dupe()).into());
            }
        };

        // No timeout here, just press Ctrl-C if you want it to cancel.
        let result = command.wait_with_output().await?;
        if !result.status.success() {
            return Err(EventLogOptionsError::DownloadFailed(
                command_name.to_owned(),
                String::from_utf8_lossy(&result.stderr).into_owned(),
            )
            .into());
        }

        fs_util::create_dir_all(
            log_path
                .parent()
                .internal_error("Error identifying log dir")?,
        )?;
        fs_util::rename(temp_path.path(), &log_path)
            .categorize_tagged(ErrorTag::EventLogDownload)?;
        crate::eprintln!("Downloaded event-log to `{}`", log_path.display())?;

        temp_path.close()?;

        Ok(log_path.into_abs_path_buf())
    }
}
