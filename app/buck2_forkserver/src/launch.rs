/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Stdio;

use buck2_error::BuckErrorContext;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_resource_control::buck_cgroup_tree::BuckCgroupTree;
use buck2_util::process::background_command;
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::client::ForkserverClient;

pub async fn launch_forkserver(
    exe: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    state_dir: &AbsNormPath,
    cgroup_tree: Option<&BuckCgroupTree>,
) -> buck2_error::Result<ForkserverClient> {
    let (client_io, server_io) =
        UnixStream::pair().buck_error_context("Failed to create fork server channel")?;

    let server_io = server_io
        .into_std()
        .buck_error_context("Failed to convert server_io to std")?;

    let exe = exe.as_ref();

    let mut command = background_command(exe);
    if let Some(cgroup_tree) = cgroup_tree {
        cgroup_tree.forkserver().setup_command(&mut command);
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit()) // TODO
        .stderr(Stdio::inherit()) // TODO
        .arg0("(buck2-forkserver)")
        .args(args)
        .arg("--fd")
        .arg(server_io.as_raw_fd().to_string())
        .arg("--state-dir")
        .arg(state_dir.as_path());

    let fds = [server_io.as_raw_fd()];

    unsafe {
        command.pre_exec(move || {
            // Clear CLOEXEC on the 2 FDs we intend to pass. It's better to set CLOEXEC and
            // clear it specifically here since that ensure that if we spawn anything else
            // concurrently, those FDs will be released on exec.

            for fd in &fds {
                let flags = libc::fcntl(*fd, libc::F_GETFD);
                if flags < 0 {
                    return Err(io::Error::last_os_error());
                }

                if libc::fcntl(*fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }

            Ok(())
        });
    }

    let child = Command::from(command).spawn().with_buck_error_context(|| {
        format!("Failed to start Forkserver `{}`", exe.to_string_lossy())
    })?;

    let channel = buck2_grpc::make_channel(client_io, "forkserver")
        .await
        .buck_error_context("Error connecting to Forkserver")?;

    ForkserverClient::new(child, channel)
        .await
        .buck_error_context("Error creating ForkserverClient")
}
