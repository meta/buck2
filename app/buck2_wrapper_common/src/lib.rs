/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Code shared between `buck2_wrapper` and `buck2`.
//!
//! Careful! The wrapper is not released as part of the regular buck version bumps,
//! meaning code changes here are not "atomically" updated.

#![feature(once_cell_try)]

use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use buck2_hash::BuckMutSet;
use is_buck2::WhoIsAsking;
use sysinfo::ProcessRefreshKind;
use sysinfo::ProcessesToUpdate;
use sysinfo::System;
use sysinfo::UpdateKind;

use crate::is_buck2::is_buck2_exe;
use crate::pid::Pid;

mod cleanall;
pub mod invocation_id;
pub mod is_buck2;
pub mod kill;
pub mod pid;
mod process;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub mod win;

pub use cleanall::cleanall_stale;
pub use process::async_background_command;
pub use process::background_command;

pub const BUCK2_WRAPPER_ENV_VAR: &str = "BUCK2_WRAPPER";
/// Identifies the launching wrapper only when it matches the client's direct parent.
pub const BUCK_WRAPPER_PID_ENV_VAR: &str = "BUCK_WRAPPER_PID";
pub const BUCK_WRAPPER_UUID_ENV_VAR: &str = "BUCK_WRAPPER_UUID";
pub const BUCK_WRAPPER_START_TIME_ENV_VAR: &str = "BUCK_WRAPPER_START_TIME";
pub const BUCKD_LIFECYCLE: &str = "buckd.lifecycle";
/// Set to `1` on the `buck2 clean --stale` processes that `cleanall --stale` spawns, so their
/// clean-stale runs are logged with the cleanall origin. An env var rather than a flag because the
/// spawned client may be an older release that would reject an unknown flag.
pub const BUCK2_CLEANALL_ENV_VAR: &str = "BUCK2_CLEANALL";
const BUCK2_TEST_HOME_DIR_ENV_VAR: &str = "BUCK2_TEST_HOME_DIR";
/// Default buck2 isolation dir. Must match the `--isolation-dir` clap
/// `default_value` in `app/buck2/src/lib.rs`; the default-isolation golden test
/// (`denied.golden.stderr`) catches drift.
pub const DEFAULT_ISOLATION_DIR: &str = "v2";
pub const CLEAN_STALE_HELP: &str =
    "Delete artifacts from buck-out using the configured clean-stale policy";
pub const EXPERIMENTS_FILENAME: &str = "experiments_from_buck_start";
pub const DOT_BUCKCONFIG_D: &str = ".buckconfig.d";
pub const SETTINGS_ROLLOUTS_FILENAME: &str = ".bucksettings.rollouts";

/// Returns the home directory used for Buck2 state.
pub fn buck2_home_dir() -> Option<PathBuf> {
    std::env::var_os(BUCK2_TEST_HOME_DIR_ENV_VAR)
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// Because `sysinfo::Process` is not `Clone`.
struct ProcessInfo {
    pid: Pid,
    name: String,
    isolation_dir: Option<String>,
    cwd: Option<PathBuf>,
}

/// Restricts which buck2 processes [`killall`] targets. The default value matches every process.
///
/// Both criteria are best-effort: they rely on information read from each candidate process
/// (command line and working directory). Processes for which that information is unavailable
/// are skipped and reported, not killed.
#[derive(Default)]
pub struct KillallFilter {
    /// Only kill processes whose command line carries this `--isolation-dir` value.
    ///
    /// Daemons always qualify because the client spawns them with the flag; client processes
    /// launched without an explicit `--isolation-dir` argument have an unknown isolation dir.
    pub isolation_dir: Option<String>,
    /// Only kill processes whose working directory is inside this project root.
    ///
    /// The path is compared against `/proc`-style fully-resolved working directories, so it
    /// should be canonicalized by the caller.
    pub project_root: Option<PathBuf>,
}

enum FilterResult {
    Kill,
    Exclude,
    /// The process cannot be attributed for the given reason, e.g. its command line does not
    /// carry an isolation dir.
    Unknown(&'static str),
}

impl KillallFilter {
    fn classify(&self, process: &ProcessInfo) -> FilterResult {
        if let Some(want) = &self.isolation_dir {
            match &process.isolation_dir {
                Some(dir) if dir == want => {}
                Some(_) => return FilterResult::Exclude,
                None => return FilterResult::Unknown("unknown isolation dir"),
            }
        }
        if let Some(root) = &self.project_root {
            match &process.cwd {
                Some(cwd) if cwd.starts_with(root) => {}
                Some(_) => return FilterResult::Exclude,
                None => return FilterResult::Unknown("unknown working directory"),
            }
        }
        FilterResult::Kill
    }
}

/// Extract the isolation dir from a buck2 process's command line.
///
/// buck2 daemons, forkservers and clients are launched with the global
/// `--isolation-dir <name>` flag (see `buck2_client_ctx::daemon::client::connect`),
/// accepting both the `--isolation-dir <name>` and `--isolation-dir=<name>` forms.
/// Returns `None` when the flag is absent (e.g. the isolation dir was supplied via
/// the `BUCK_ISOLATION_DIR` env var, which does not appear in argv, or the process
/// predates the flag being added to forkserver spawns).
///
/// Like clap, the last occurrence wins when the flag is given both before and after the
/// subcommand, and arguments after `--` belong to the program being run, not to buck2.
fn parse_isolation_dir(cmd: &[String]) -> Option<String> {
    let mut isolation_dir = None;
    let mut args = cmd.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if let Some(value) = arg.strip_prefix("--isolation-dir=") {
            isolation_dir = Some(value.to_owned());
        } else if arg == "--isolation-dir" {
            isolation_dir = args.next().cloned();
        }
    }
    isolation_dir
}

/// Get the list of all PIDs on Linux
///
/// As of sysinfo 0.30, the `processes` function returns all posix TIDs (what the kernel calls
/// PIDs), and not just all posix PIDs (what the kernel calls TGIDs). In order to make sure that we
/// don't kill any of the TIDs in our PID, we need to filter the list of TIDs down. This function
/// returns the list of all PIDs on the system.
fn get_all_tgids_linux() -> Option<BuckMutSet<sysinfo::Pid>> {
    if !cfg!(target_os = "linux") {
        return None;
    }

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Some(BuckMutSet::default());
    };

    let mut all_tgids = BuckMutSet::default();

    for e in entries {
        let Ok(e) = e else {
            continue;
        };
        let Ok(file_type) = e.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        all_tgids.insert(sysinfo::Pid::from_u32(pid));
    }

    Some(all_tgids)
}

/// Find all buck2 processes in the system.
///
/// Working directories are only collected when `collect_cwd` is set, as sysinfo has to read
/// them per-process.
fn find_buck2_processes(who_is_asking: WhoIsAsking, collect_cwd: bool) -> Vec<ProcessInfo> {
    let mut system = System::new();
    let linux_tgids =
        get_all_tgids_linux().map(|pids| pids.into_iter().collect::<Vec<sysinfo::Pid>>());
    let processes_to_update = match &linux_tgids {
        Some(pids) => ProcessesToUpdate::Some(pids),
        None => ProcessesToUpdate::All,
    };
    system.refresh_processes_specifics(
        processes_to_update,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );

    let mut current_parents = BuckMutSet::default();
    let mut parent = Some(sysinfo::Pid::from_u32(std::process::id()));
    while let Some(pid) = parent {
        // There is a small chance on Windows that the PID of a dead parent
        // was reused by some of its descendants, and this can create a loop.
        if !current_parents.insert(pid) {
            break;
        }
        parent = system.process(pid).and_then(|p| p.parent());
    }

    let mut buck2_processes = Vec::new();
    for (sys_pid, process) in system.processes() {
        let Some(exe) = process.exe() else {
            continue;
        };
        if is_buck2_exe(exe, who_is_asking) && !current_parents.contains(sys_pid) {
            let Ok(pid) = Pid::from_u32(sys_pid.as_u32()) else {
                continue;
            };
            buck2_processes.push((
                *sys_pid,
                ProcessInfo {
                    pid,
                    name: process.name().to_string_lossy().into_owned(),
                    isolation_dir: None,
                    cwd: None,
                },
            ));
        }
    }

    let matched_pids = buck2_processes
        .iter()
        .map(|(pid, _)| *pid)
        .collect::<Vec<_>>();
    if !matched_pids.is_empty() {
        let mut refresh_kind = ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always);
        if collect_cwd {
            refresh_kind = refresh_kind.with_cwd(UpdateKind::Always);
        }
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&matched_pids),
            true,
            refresh_kind,
        );
    }

    buck2_processes
        .into_iter()
        .map(|(pid, mut process_info)| {
            if let Some(process) = system.process(pid) {
                let cmd = process
                    .cmd()
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                process_info.isolation_dir = parse_isolation_dir(&cmd);
                process_info.cwd = process.cwd().map(|cwd| cwd.to_path_buf());
            }
            process_info
        })
        .collect()
}

/// Kills all running Buck2 processes matching `filter`, except this process's hierarchy.
/// Processes the filter cannot attribute are reported and left alone. Returns whether it
/// succeeded without errors.
pub fn killall(who_is_asking: WhoIsAsking, filter: &KillallFilter, write: impl Fn(String)) -> bool {
    let found = find_buck2_processes(who_is_asking, filter.project_root.is_some());
    let found_any = !found.is_empty();

    let mut buck2_processes = Vec::new();
    for process in found {
        match filter.classify(&process) {
            FilterResult::Kill => buck2_processes.push(process),
            FilterResult::Exclude => {}
            FilterResult::Unknown(reason) => {
                write(format!(
                    "Skipped {} ({}): {reason}",
                    process.name, process.pid
                ));
            }
        }
    }

    if buck2_processes.is_empty() {
        if found_any {
            write("No buck2 processes matched the requested filter".to_owned());
        } else {
            write("No buck2 processes found".to_owned());
        }
        return true;
    }

    struct Printer<F> {
        write: F,
        /// All processes were killed successfully.
        ok: bool,
    }

    impl<F: Fn(String)> Printer<F> {
        fn fmt_status(&mut self, process: &ProcessInfo, status: &str) -> String {
            let isolation_dir = match &process.isolation_dir {
                Some(dir) => format!(" ({dir})"),
                None => String::new(),
            };
            format!(
                "{} {} ({}){}",
                status, process.name, process.pid, isolation_dir
            )
        }

        fn failed_to_kill(&mut self, process: &ProcessInfo, error: buck2_error::Error) {
            let mut message = self.fmt_status(process, "Failed to kill");
            for line in format!("{error:?}").lines() {
                message.push_str("\n  ");
                message.push_str(line);
            }
            (self.write)(message);

            self.ok = false;
        }

        fn killed(&mut self, process: &ProcessInfo) {
            let message = self.fmt_status(process, "Killed");
            (self.write)(message);
        }
    }

    let mut printer = Printer { write, ok: true };

    // Send a kill signal and collect the processes that are still alive.

    let mut processes_still_alive: Vec<(ProcessInfo, _)> = Vec::new();
    for process in buck2_processes {
        match kill::kill(process.pid) {
            Ok(Some(handle)) => processes_still_alive.push((process, handle)),
            Ok(None) => {}
            Err(e) => printer.failed_to_kill(&process, e),
        };
    }

    // Wait for the processes to exit.

    let start = Instant::now();
    while !processes_still_alive.is_empty() {
        let timeout_secs = 10;

        processes_still_alive.retain(|(process, handle)| match handle.has_exited() {
            Err(e) => {
                printer.failed_to_kill(process, e);
                false
            }
            Ok(true) => {
                printer.killed(process);
                false
            }
            Ok(false) => true,
        });

        if Instant::now() - start > Duration::from_secs(timeout_secs) {
            for process in processes_still_alive {
                printer.failed_to_kill(
                    &process.0,
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::DaemonWontDieFromKill,
                        "Process still alive after {timeout_secs}s after kill sent"
                    ),
                );
            }
            break;
        }

        thread::sleep(Duration::from_millis(100));
    }

    printer.ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_isolation_dir() {
        for (args, expected) in [
            (
                &["buck2", "--isolation-dir", "custom", "daemon"][..],
                Some("custom"),
            ),
            (
                &["buck2", "--isolation-dir=custom", "daemon"][..],
                Some("custom"),
            ),
            (&["buck2", "daemon"][..], None),
            (&["buck2", "--isolation-dir"][..], None),
        ] {
            let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
            assert_eq!(expected.map(str::to_owned), parse_isolation_dir(&args));
        }
    }

    #[test]
    fn test_killall_filter_classify() {
        let process = |isolation_dir: Option<&str>, cwd: Option<&str>| ProcessInfo {
            pid: Pid::from_u32(1).expect("1 should be a valid pid"),
            name: "buck2".to_owned(),
            isolation_dir: isolation_dir.map(str::to_owned),
            cwd: cwd.map(PathBuf::from),
        };
        let filter = |isolation_dir: Option<&str>, project_root: Option<&str>| KillallFilter {
            isolation_dir: isolation_dir.map(str::to_owned),
            project_root: project_root.map(PathBuf::from),
        };

        for (filter, process, expected) in [
            // Empty filter kills everything, attributed or not.
            (filter(None, None), process(None, None), "kill"),
            // Isolation dir filter.
            (filter(Some("v2"), None), process(Some("v2"), None), "kill"),
            (
                filter(Some("v2"), None),
                process(Some("other"), None),
                "exclude",
            ),
            (filter(Some("v2"), None), process(None, None), "unknown"),
            // Project root filter, including a process in a subdirectory of the root.
            (
                filter(None, Some("/repo")),
                process(None, Some("/repo")),
                "kill",
            ),
            (
                filter(None, Some("/repo")),
                process(None, Some("/repo/sub/dir")),
                "kill",
            ),
            // Component-wise comparison: `/repo2` is not inside `/repo`.
            (
                filter(None, Some("/repo")),
                process(None, Some("/repo2")),
                "exclude",
            ),
            (filter(None, Some("/repo")), process(None, None), "unknown"),
            // Both criteria must pass.
            (
                filter(Some("v2"), Some("/repo")),
                process(Some("v2"), Some("/repo")),
                "kill",
            ),
            (
                filter(Some("v2"), Some("/repo")),
                process(Some("v2"), Some("/elsewhere")),
                "exclude",
            ),
            (
                filter(Some("v2"), Some("/repo")),
                process(Some("other"), Some("/repo")),
                "exclude",
            ),
        ] {
            let actual = match filter.classify(&process) {
                FilterResult::Kill => "kill",
                FilterResult::Exclude => "exclude",
                FilterResult::Unknown(_) => "unknown",
            };
            assert_eq!(
                expected, actual,
                "filter ({:?}, {:?}) applied to process ({:?}, {:?})",
                filter.isolation_dir, filter.project_root, process.isolation_dir, process.cwd,
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_find_buck2_processes() {
        use std::fs;
        use std::path::PathBuf;
        use std::process::Child;
        use std::process::Stdio;
        use std::time::SystemTime;

        use crate::background_command;

        struct ChildGuard {
            child: Child,
            temp_dir: PathBuf,
        }

        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ignored = self.child.kill();
                let _ignored = self.child.wait();
                let _ignored = fs::remove_dir_all(&self.temp_dir);
            }
        }

        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!(
            "buck2_wrapper_common_process_scan_{}_{}",
            std::process::id(),
            nonce,
        ));
        fs::create_dir(&temp_dir).expect("temporary test directory should be created");
        let fake_buck2 = temp_dir.join("buck2");
        fs::copy("/bin/sh", &fake_buck2).expect("test buck2 executable should be copied");

        // Under OSS `cargo test` (all tests share one process, unlike buck's
        // per-test process isolation) a sibling test's `fork` can inherit the
        // write fd `fs::copy` briefly holds on `fake_buck2`, making `exec` fail
        // with ETXTBSY until that fd clears, which is when the sibling's child
        // execs. On a loaded runner that can take a while. Retry past the window.
        let mut child = None;
        for _ in 0..1000 {
            match background_command(&fake_buck2)
                .args(["-c", "read _", "--isolation-dir=process-scan-test"])
                .stdin(Stdio::piped())
                .spawn()
            {
                Ok(spawned) => {
                    child = Some(spawned);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(nix::libc::ETXTBSY) => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("fake buck2 process should start: {e:?}"),
            }
        }
        let child = child.expect("fake buck2 process should start before ETXTBSY window closes");
        let child_pid = child.id();
        let _guard = ChildGuard { child, temp_dir };

        let processes = find_buck2_processes(WhoIsAsking::Buck2, true);
        let child_process = processes
            .iter()
            .find(|process| process.pid.to_u32() == child_pid)
            .expect("scan should find the fake buck2 child process");
        assert_eq!(
            Some("process-scan-test"),
            child_process.isolation_dir.as_deref(),
        );
        assert!(
            child_process.cwd.is_some(),
            "scan should collect the child's working directory when asked to",
        );

        let current_pid = std::process::id();
        assert!(
            processes
                .iter()
                .all(|process| process.pid.to_u32() != current_pid),
            "scan should exclude the invoking process",
        );
    }

    fn parse(cmd: &str) -> Option<String> {
        parse_isolation_dir(&cmd.split(' ').map(str::to_owned).collect::<Vec<_>>())
    }

    /// The scan agrees with clap: the last `--isolation-dir` wins, and flags after `--` belong
    /// to the program being run.
    #[test]
    fn test_parse_isolation_dir_takes_the_last_flag_before_double_dash() {
        assert_eq!(
            Some("b".to_owned()),
            parse("buck2 --isolation-dir a build --isolation-dir b")
        );
        assert_eq!(None, parse("buck2 run //t -- --isolation-dir x"));
        assert_eq!(
            Some("a".to_owned()),
            parse("buck2 --isolation-dir a run //t -- --isolation-dir x")
        );
    }
}
