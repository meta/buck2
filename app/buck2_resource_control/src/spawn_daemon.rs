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
use std::io::ErrorKind;
use std::num::ParseIntError;

use buck2_common::init::ResourceControlConfig;
use buck2_common::init::ResourceControlInit;
use buck2_common::init::ResourceControlStatus;
use buck2_core::soft_error;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_util::process;
use buck2_util::process::async_background_command;

use crate::buck_cgroup_tree::parse_procfs_cgroup_output;
use crate::buck_cgroup_tree::read_current_cgroup;
#[cfg(unix)]
use crate::cgroup::Cgroup;
#[cfg(unix)]
use crate::cgroup::CgroupKindInternal;
#[cfg(unix)]
use crate::cgroup::NoMemoryMonitoring;

const DAEMON_ORIGINATING_CGROUP_FLAG: &str = "--daemon-originating-cgroup";

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Environment)]
enum SystemdNotAvailableReason {
    #[error("Unexpected `systemctl --version` output format: {0}")]
    UnexpectedVersionOutputFormat(String),
    #[error("Failed to parse systemd version number into u32: {0:#}")]
    VersionNumberParseError(ParseIntError),
    #[error("Detected systemd version {detected}. Minimum requirement is {min_required}.")]
    TooOldSystemdVersion { detected: u32, min_required: u32 },
    #[error("Systemctl command returned non-zero: {0}")]
    SystemctlCommandReturnedNonZero(String),
    #[error("Systemctl command failed to launch: {0:#}")]
    SystemctlCommandLaunchFailed(std::io::Error),
    #[error("Systemctl command not found in PATH.")]
    SystemctlCommandNotFound,
    #[error("Resource control with systemd is only supported on Linux.")]
    UnsupportedPlatform,
    #[error("systemd-run --user probe failed (exit code {code}): {stderr}")]
    SystemdRunUserProbeFailure { code: String, stderr: String },
    #[error("systemd-run --user probe returned unusable cgroup membership `{cgroup}`")]
    SystemdRunUserProbeInvalidCgroup {
        cgroup: String,
        #[source]
        error: buck2_error::Error,
    },
}

const SYSTEMD_RUN_SCOPE_ARGS: &[&str] = &[
    "--user",
    "--scope",
    "--quiet",
    "--collect",
    "--property=Delegate=yes",
    "--slice=buck2",
];

enum DaemonSpawner {
    None,
    Systemd,
    #[cfg(unix)]
    Cgroup(Cgroup<NoMemoryMonitoring, CgroupKindInternal>),
}

async fn get_daemon_spawner(init: &ResourceControlInit) -> buck2_error::Result<DaemonSpawner> {
    match &init {
        ResourceControlInit::Systemd => systemd_check_available()
            .await
            .map(|_| DaemonSpawner::Systemd),
        #[cfg(unix)]
        ResourceControlInit::Cgroup(path) => {
            use crate::cgroup::CgroupMinimal;
            use crate::path::CgroupPathBuf;

            let path = CgroupPathBuf::new_in_cgroup_fs(path);
            let parent = CgroupMinimal::try_from_path(path).await?;
            let controllers = parent.read_enabled_controllers().await?;
            for controller in ["memory", "cpu"] {
                if !controllers.contains(controller) {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Buck parent cgroup does not have {} controller enabled",
                        controller
                    ));
                }
            }
            Ok(DaemonSpawner::Cgroup(
                parent.into_internal_with_subtree_control_already_enabled(controllers),
            ))
        }
    }
}

/// Generates the command to spawn a daemon process, wrapped as appropriate for the resource control setup.
///
/// Returns the command, followed by a list of additional flags to pass to `buck2 daemon`.
pub async fn create_daemon_spawn_command(
    config: &ResourceControlConfig,
    program: impl AsRef<OsStr>,
    unit_name: String,
    working_directory: &AbsNormPath,
) -> buck2_error::Result<(std::process::Command, Vec<String>)> {
    let daemon_spawner = {
        if config.status == ResourceControlStatus::Off
            || buck2_core::buck2_env!(
                "BUCK2_TEST_DISABLE_DAEMON_CGROUP",
                type = bool,
                applicability = testing,
            )?
            .unwrap_or(false)
        {
            DaemonSpawner::None
        } else {
            match (config.status, get_daemon_spawner(&config.init).await) {
                (ResourceControlStatus::Off, _) => unreachable!("Checked above"),
                (_, Ok(s)) => s,
                (ResourceControlStatus::Required, Err(e)) => return Err(e),
                (ResourceControlStatus::IfAvailable, Err(e)) => {
                    soft_error!("daemon_spawner_resource_control_unavailable", e);
                    DaemonSpawner::None
                }
            }
        }
    };

    // These are special in systemd so avoid them
    let unit_name = unit_name.replace("-", "_").replace(":", "_");

    match daemon_spawner {
        DaemonSpawner::None => Ok((process::background_command(program), Vec::new())),
        DaemonSpawner::Systemd => Ok((
            systemd_run_command(program, &unit_name, working_directory),
            daemon_resource_control_args(),
        )),
        #[cfg(unix)]
        DaemonSpawner::Cgroup(parent) => {
            use buck2_error::BuckErrorContext;
            use buck2_fs::paths::file_name::FileName;

            // The daemon treats this cgroup the same way it treats a systemd scope: it builds its
            // own tree beneath it and leaves the attributes of the cgroup itself alone.
            let child = parent
                .make_leaf_child(
                    FileName::new(&unit_name)
                        .internal_error("Unitname isn't a valid filename")?
                        .to_owned(),
                )
                .await?;
            let mut cmd = process::background_command(program);
            child.setup_command(&mut cmd);

            Ok((cmd, daemon_resource_control_args()))
        }
    }
}

fn daemon_resource_control_args() -> Vec<String> {
    let mut args = vec!["--has-cgroup".to_owned()];

    if let Some(originating_cgroup) = read_current_cgroup() {
        args.push(DAEMON_ORIGINATING_CGROUP_FLAG.to_owned());
        args.push(originating_cgroup);
    }

    args
}

fn systemd_run_command(
    program: impl AsRef<OsStr>,
    unit_name: &str,
    working_directory: &AbsNormPath,
) -> std::process::Command {
    let mut cmd = process::background_command("systemd-run");
    cmd.args(SYSTEMD_RUN_SCOPE_ARGS);
    // N.B. the slice name here is used by BPFJailer to assign the `buck` Role
    // ID to the daemon process, which is what gives the daemon permission to
    // exit the jail.
    cmd.arg(format!("--working-directory={working_directory}"));
    cmd.arg(format!("--unit={unit_name}"));
    cmd.arg(program);
    cmd
}

fn validate_systemd_run_cgroup(raw_stdout: &[u8]) -> Result<(), SystemdNotAvailableReason> {
    let stdout = String::from_utf8_lossy(raw_stdout);
    parse_procfs_cgroup_output(&stdout)
        .map(|_| ())
        .map_err(
            |error| SystemdNotAvailableReason::SystemdRunUserProbeInvalidCgroup {
                cgroup: stdout.trim().to_owned(),
                error,
            },
        )
}

fn validate_systemd_version(raw_stdout: &[u8]) -> Result<(), SystemdNotAvailableReason> {
    const SYSTEMD_MIN_VERSION: u32 = 253;

    let stdout = String::from_utf8_lossy(raw_stdout);
    let version = stdout
        .split(' ')
        .nth(1)
        .ok_or_else(|| {
            SystemdNotAvailableReason::UnexpectedVersionOutputFormat(stdout.to_string())
        })?
        .parse::<u32>()
        .map_err(SystemdNotAvailableReason::VersionNumberParseError)?;

    if version < SYSTEMD_MIN_VERSION {
        Err(SystemdNotAvailableReason::TooOldSystemdVersion {
            detected: version,
            min_required: SYSTEMD_MIN_VERSION,
        })
    } else {
        Ok(())
    }
}

async fn systemd_check_available() -> buck2_error::Result<()> {
    if !cfg!(target_os = "linux") {
        return Err(SystemdNotAvailableReason::UnsupportedPlatform.into());
    }

    match async_background_command("systemctl")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                validate_systemd_version(&output.stdout)?;
            } else {
                return Err(SystemdNotAvailableReason::SystemctlCommandReturnedNonZero(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                )
                .into());
            }
        }
        Err(e) => match e.kind() {
            ErrorKind::NotFound => {
                return Err(SystemdNotAvailableReason::SystemctlCommandNotFound.into());
            }
            _ => return Err(SystemdNotAvailableReason::SystemctlCommandLaunchFailed(e).into()),
        },
    }

    // Exercise the same scope configuration used for the daemon and verify that
    // the resulting cgroup is visible from this namespace. A cgroup namespace
    // may let systemd-run succeed while reporting a path containing `..`, which
    // the daemon cannot access through its cgroup filesystem.
    match async_background_command("systemd-run")
        .args(SYSTEMD_RUN_SCOPE_ARGS)
        .args(["--", "/bin/cat", "/proc/self/cgroup"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            validate_systemd_run_cgroup(&output.stdout).map_err(Into::into)
        }
        Ok(output) => {
            let code = output
                .status
                .code()
                .map_or("unknown".to_owned(), |c| c.to_string());
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(SystemdNotAvailableReason::SystemdRunUserProbeFailure { code, stderr }.into())
        }
        Err(e) => Err(SystemdNotAvailableReason::SystemctlCommandLaunchFailed(e).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_systemd_version_normal() {
        let raw_output = "systemd 253 (v253.7-1.9.hs+fb.el9)".as_bytes();
        assert!(validate_systemd_version(raw_output).is_ok());
    }

    #[test]
    fn test_validate_systemd_version_unexpected_format() {
        let raw_output = "abc".as_bytes();
        assert!(matches!(
            validate_systemd_version(raw_output).unwrap_err(),
            SystemdNotAvailableReason::UnexpectedVersionOutputFormat(..)
        ));
    }

    #[test]
    fn test_validate_systemd_version_empty() {
        let raw_output = "".as_bytes();
        assert!(matches!(
            validate_systemd_version(raw_output).unwrap_err(),
            SystemdNotAvailableReason::UnexpectedVersionOutputFormat(..)
        ));
    }

    #[test]
    fn test_validate_systemd_version_unexpected_version() {
        let raw_output = "systemd v253.7-1.9.hs+fb.el9".as_bytes();
        assert!(matches!(
            validate_systemd_version(raw_output).unwrap_err(),
            SystemdNotAvailableReason::VersionNumberParseError(..)
        ));
    }

    #[test]
    fn test_validate_systemd_version_old_version() {
        let raw_output = "systemd 111 (v253.7-1.9.hs+fb.el9)".as_bytes();
        assert!(matches!(
            validate_systemd_version(raw_output).unwrap_err(),
            SystemdNotAvailableReason::TooOldSystemdVersion { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_systemd_run_cgroup_visible() {
        assert!(
            validate_systemd_run_cgroup(
                b"0::/user.slice/user-1000.slice/user@1000.service/buck2.slice/run.scope\n"
            )
            .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_systemd_run_cgroup_hidden_parent() {
        assert!(matches!(
            validate_systemd_run_cgroup(b"0::/../../buck2.slice/run.scope\n").unwrap_err(),
            SystemdNotAvailableReason::SystemdRunUserProbeInvalidCgroup { .. }
        ));
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn test_always_unavailable_on_nonlinux() {
        let _error = buck2_error::Error::from(SystemdNotAvailableReason::UnsupportedPlatform);
        assert!(matches!(
            systemd_check_available().await.unwrap_err(),
            _error
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_spawn_into_specific_cgroup() {
        // Ideally this would be an integration test, but as it stands that's a little bit hard rn.
        // So unit test instead
        use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;

        let Some(parent) = Cgroup::create_internal_for_test().await else {
            return;
        };
        let p = parent
            .path()
            .to_str()
            .unwrap()
            .strip_prefix("/sys/fs/cgroup")
            .unwrap()
            .to_owned();
        let (mut cmd, _) = create_daemon_spawn_command(
            &ResourceControlConfig {
                status: ResourceControlStatus::Required,
                init: ResourceControlInit::Cgroup(AbsNormPathBuf::from(p).unwrap()),
                ..ResourceControlConfig::testing_default()
            },
            "sleep",
            "myunitname".to_owned(),
            // Working dir, doesn't matter
            AbsNormPath::new("/").unwrap(),
        )
        .await
        .unwrap();
        cmd.arg("1m");
        let mut c = cmd.spawn().unwrap();

        assert!(parent.read_pid_count().await.unwrap() > 0);

        c.kill().unwrap();
        c.wait().unwrap();
    }
}
