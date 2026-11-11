/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A tenant's own daemon directory as the proxy of a generation owns it: the connection files it
//! publishes there, and the lifecycle lock clients of every release take to change the owner.

use std::fs::File;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::time::Duration;
use std::time::Instant;

use buck2_cli_proto::DaemonProcessInfo;
use buck2_common::daemon_compatibility::TENANT_PROXY_SPEC_FILE;
use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_common::daemon_dir::DaemonDir;
use buck2_common::invocation_paths::TenantPaths;
use buck2_core::fs::project::ProjectRoot;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_wrapper_common::BUCKD_LIFECYCLE;
use buck2_wrapper_common::kill;
use buck2_wrapper_common::pid::Pid;

pub(crate) fn proxy_dir(spec: &TenantProxySpec) -> buck2_error::Result<DaemonDir> {
    TenantPaths::new(
        ProjectRoot::new_unchecked(AbsNormPathBuf::try_from(spec.tenant.project_root.clone())?),
        FileNameBuf::try_from(spec.tenant.isolation.clone())?,
    )
    .unshared_daemon_dir()
}

pub(crate) fn legacy_lock(directory: &DaemonDir) -> buck2_error::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(
            directory
                .path
                .join(FileName::unchecked_new(BUCKD_LIFECYCLE)),
        )?;
    file.try_lock().map_err(std::io::Error::from)?;
    Ok(file)
}

/// Takes `buckd.lifecycle`, waiting out a client that holds it for discovery or a kill.
pub(crate) fn lock_legacy_slot(
    directory: &DaemonDir,
    timeout: Duration,
) -> buck2_error::Result<File> {
    let deadline = Instant::now() + timeout;
    loop {
        match legacy_lock(directory) {
            Ok(file) => return Ok(file),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error),
        }
    }
}

/// The pid recorded in the slot's `buckd.info` if it may belong to a live process other than
/// `own`, judged by [`kill::recorded_process_may_be_running`]. A record that cannot be parsed
/// falls back to `buckd.pid`, so that a damaged record does not hide a live owner; with neither
/// naming a live process, or a record that names an impossible pid, the slot is treated as
/// unowned. A record that exists but cannot be opened is an error: it may name a live owner.
pub(crate) fn live_slot_owner(directory: &DaemonDir, own: i64) -> buck2_error::Result<Option<i64>> {
    let recorded_at = |path: AbsNormPathBuf| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    };
    let file = match File::open(directory.buckd_info()) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Ok(info) = serde_json::from_reader::<_, DaemonProcessInfo>(file) else {
        let pid = std::fs::read_to_string(directory.buckd_pid())
            .ok()
            .and_then(|pid| pid.trim().parse::<i64>().ok())
            .filter(|pid| *pid > 0 && *pid != own)
            .and_then(|pid| Some((pid, Pid::from_i64(pid).ok()?)));
        return match pid {
            Some((pid, process))
                if kill::recorded_process_may_be_running(
                    process,
                    0,
                    recorded_at(directory.buckd_pid()),
                )? =>
            {
                Ok(Some(pid))
            }
            _ => Ok(None),
        };
    };
    if info.pid == own || info.pid <= 0 {
        return Ok(None);
    }
    let Ok(process) = Pid::from_i64(info.pid) else {
        return Ok(None);
    };
    let live = kill::recorded_process_may_be_running(
        process,
        info.pid_start_time,
        recorded_at(directory.buckd_info()),
    )?;
    Ok(live.then_some(info.pid))
}

/// The files a proxy publishes in its tenant's daemon directory, and its spec.
pub(crate) fn slot_files(directory: &DaemonDir) -> [AbsNormPathBuf; 3] {
    [
        directory.buckd_info(),
        directory.buckd_pid(),
        directory
            .path
            .join(FileName::unchecked_new(TENANT_PROXY_SPEC_FILE)),
    ]
}

pub(crate) fn slot_record(directory: &DaemonDir) -> Option<DaemonProcessInfo> {
    File::open(directory.buckd_info())
        .ok()
        .and_then(|file| serde_json::from_reader::<_, DaemonProcessInfo>(file).ok())
}

/// Whether the slot still belongs to the proxy of `spec` running as `pid`: its record names that
/// process and its spec file still describes that generation, so a replacement that has written
/// its own spec but not yet published is not mistaken for it.
pub(crate) fn slot_matches(spec: &TenantProxySpec, directory: &DaemonDir, pid: i64) -> bool {
    let slot_spec = File::open(
        directory
            .path
            .join(FileName::unchecked_new(TENANT_PROXY_SPEC_FILE)),
    )
    .ok()
    .and_then(|file| serde_json::from_reader::<_, TenantProxySpec>(file).ok());
    slot_spec.is_some_and(|slot_spec| slot_spec.reference == spec.reference)
        && slot_record(directory).is_some_and(|info| info.pid == pid)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::Duration;
    use std::time::SystemTime;

    use buck2_cli_proto::DaemonProcessInfo;
    use buck2_common::daemon_dir::DaemonDir;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_wrapper_common::kill;

    use super::live_slot_owner;

    #[test]
    fn damaged_slot_record_falls_back_to_the_pid_file() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let directory = DaemonDir {
            path: temp.path().root().to_buf(),
        };
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            None,
            "an empty slot has no owner"
        );

        std::fs::write(directory.buckd_info(), "{ not json")?;
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            None,
            "a damaged record alone does not prove an owner"
        );

        let live = i64::from(std::process::id());
        std::fs::write(directory.buckd_pid(), live.to_string())?;
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            Some(live),
            "the pid file names a live owner behind the damaged record"
        );
        assert_eq!(
            live_slot_owner(&directory, live)?,
            None,
            "the proxy itself is not a foreign owner"
        );
        Ok(())
    }

    #[test]
    fn slot_record_naming_a_reused_pid_has_no_owner() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let directory = DaemonDir {
            path: temp.path().root().to_buf(),
        };
        let live = i64::from(std::process::id());
        let start_time = kill::process_start_time(None).expect("own start time should be readable");
        let record = |pid_start_time: u64| -> buck2_error::Result<()> {
            let info = DaemonProcessInfo {
                pid: live,
                pid_start_time,
                ..Default::default()
            };
            std::fs::write(directory.buckd_info(), serde_json::to_string(&info)?)?;
            Ok(())
        };

        record(start_time)?;
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            Some(live),
            "the recorded process"
        );
        record(start_time - 60)?;
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            None,
            "another start time means the pid was reused"
        );

        // A release that did not record start times, long before this process started.
        record(0)?;
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            Some(live),
            "a record without a start time, written after the process started"
        );
        File::options()
            .write(true)
            .open(directory.buckd_info())?
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(start_time - 3600))?;
        assert_eq!(
            live_slot_owner(&directory, 1)?,
            None,
            "a process that started an hour after the record reused the pid"
        );
        Ok(())
    }
}
