/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_core::logging::LogConfigurationReloadHandle;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use clap::ArgGroup;

#[cfg(unix)]
type RawFd = std::os::unix::io::RawFd;

#[cfg(windows)]
type RawFd = String;

#[derive(Debug, clap::Parser)]
#[clap(about = "run the forkserver")]
#[clap(group(
    ArgGroup::new("comm_channel").required(true).args(["fd", "socket_path"]),  // can only accept one of the two for the communication channel
))]
pub(crate) struct ForkserverCommand {
    /// File descriptor to use for the communication channel.
    #[clap(long)]
    fd: Option<RawFd>,

    /// Path to the socket to use for the communication channel.
    #[clap(long)]
    socket_path: Option<String>,

    #[clap(long)]
    state_dir: String,
}

impl ForkserverCommand {
    pub(crate) fn exec(
        self,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
        log_reload_handle: Arc<dyn LogConfigurationReloadHandle>,
    ) -> ExitResult {
        let state_dir = AbsNormPathBuf::from(self.state_dir)?;
        fs_util::create_dir_all(&state_dir).map_err(buck2_error::Error::from)?;
        events_ctx.log_invocation_record = false;

        #[cfg(unix)]
        {
            // For us to get this FD it must be non-CLOEXEC but we don't want our children to
            // inherit it.
            if let Some(fd) = self.fd {
                set_cloexec(fd)?;
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;

            rt.block_on(buck2_forkserver::command::run_forkserver(
                self.fd,
                self.socket_path,
                log_reload_handle,
                state_dir,
            ))
            .into()
        }

        #[cfg(not(unix))]
        {
            let _ignored = log_reload_handle;
            ExitResult::bail("The forkserver is only available on UNIX")
        }
    }
}

#[cfg(unix)]
fn set_cloexec(fd: RawFd) -> std::io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }

        if libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}
