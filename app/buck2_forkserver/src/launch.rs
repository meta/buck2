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
use std::ffi::OsString;
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::Arc;

use buck2_error::BuckErrorContext;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_resource_control::buck_cgroup_tree::BuckCgroupTree;
use buck2_util::process::background_command;
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::client::ForkserverClient;
use crate::state_dir::ForkserverStateDir;

pub async fn launch_forkserver(
    exe: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    state_dir: &AbsNormPath,
    cgroup_tree: Option<&BuckCgroupTree>,
) -> buck2_error::Result<ForkserverClient> {
    let owned_path = state_dir.to_owned();
    let state_dir_owner = Arc::new(
        tokio::task::spawn_blocking(move || ForkserverStateDir::create(&owned_path)).await??,
    );
    let exe = exe.as_ref().to_owned();
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect::<Vec<OsString>>();
    launch_with_owner(&exe, args, cgroup_tree, state_dir_owner).await
}

async fn launch_with_owner(
    exe: &OsStr,
    args: Vec<OsString>,
    cgroup_tree: Option<&BuckCgroupTree>,
    state_dir_owner: Arc<ForkserverStateDir>,
) -> buck2_error::Result<ForkserverClient> {
    let (client_io, server_io) =
        UnixStream::pair().buck_error_context("Failed to create fork server channel")?;

    let server_io = server_io
        .into_std()
        .buck_error_context("Failed to convert server_io to std")?;

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
        .arg(state_dir_owner.path().as_path())
        .arg("--state-dir-lock-fd")
        .arg(state_dir_owner.lease_fd().to_string());

    let fds = [server_io.as_raw_fd(), state_dir_owner.lease_fd()];

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

    ForkserverClient::new(
        child,
        buck2_grpc::make_channel(client_io, "forkserver"),
        state_dir_owner,
    )
    .await
    .buck_error_context("Error creating ForkserverClient")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU8;

    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;

    use super::*;
    use crate::state_dir::CLEANUP_PENDING;
    use crate::state_dir::CleanupOutcome;
    use crate::state_dir::await_cleanup_and_reclaim;

    #[tokio::test]
    async fn failed_spawn_removes_state_dir() {
        let root = tempfile::tempdir().unwrap();
        let root_path = AbsNormPathBuf::new(root.path().to_owned()).unwrap();
        let dir = AbsNormPathBuf::new(root.path().join("forkserver")).unwrap();
        let probe = Arc::new(AtomicU8::new(CLEANUP_PENDING));
        let owned_dir = dir.clone();
        let owned_probe = probe.clone();
        let owner = Arc::new(
            tokio::task::spawn_blocking(move || {
                ForkserverStateDir::create_with_probe(&owned_dir, owned_probe)
            })
            .await
            .unwrap()
            .unwrap(),
        );
        let exe = root.path().join("missing-executable");
        let args = vec![OsString::from("forkserver")];
        assert!(
            launch_with_owner(exe.as_os_str(), args, None, owner)
                .await
                .is_err()
        );
        let outcome = await_cleanup_and_reclaim(probe, dir, root_path).await;
        assert!(matches!(
            outcome,
            CleanupOutcome::Removed | CleanupOutcome::Deferred
        ));
    }

    #[tokio::test]
    async fn existing_state_dir_is_not_claimed_or_removed() {
        let root = tempfile::tempdir().unwrap();
        let dir =
            buck2_fs::paths::abs_norm_path::AbsNormPathBuf::new(root.path().to_owned()).unwrap();
        let sentinel = root.path().join("sentinel");
        std::fs::write(&sentinel, "existing state").unwrap();
        assert!(
            launch_forkserver(
                root.path().join("missing-executable"),
                ["forkserver"],
                &dir,
                None
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "existing state");
    }
}
