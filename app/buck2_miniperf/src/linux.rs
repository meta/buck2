/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::fs::File;
use std::io::Read;
use std::mem::MaybeUninit;
use std::os::fd::FromRawFd;
use std::os::fd::IntoRawFd;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use buck2_miniperf_proto::MiniperfCounter;
use buck2_miniperf_proto::MiniperfCounters;
use buck2_miniperf_proto::MiniperfOutput;
use perf_event::Builder;
use perf_event::ReadFormat;
use perf_event::events::Hardware;
use smallvec::SmallVec;

/// PID of the wrapped command, read by the `SIGTERM` handler.
static CHILD_PID: AtomicI32 = AtomicI32::new(0);

/// Forwards `SIGTERM` to the wrapped command, so that a command buck2 asks to stop gets to clean
/// up; miniperf itself keeps waiting for it and reports its exit status as usual.
extern "C" fn forward_sigterm(_signal: libc::c_int) {
    let pid = CHILD_PID.load(Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: `kill` is async-signal-safe.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

/// Blocks or unblocks `SIGTERM` for the calling thread. Async-signal-safe, so it can run between
/// fork and exec.
fn set_sigterm_blocked(blocked: bool) {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` initializes the set `set` points to.
    unsafe { libc::sigemptyset(set.as_mut_ptr()) };
    // SAFETY: `set` was initialized by `sigemptyset`.
    let mut set = unsafe { set.assume_init() };
    // SAFETY: `set` is an initialized signal set and `SIGTERM` is a valid signal number.
    unsafe { libc::sigaddset(&mut set, libc::SIGTERM) };
    let how = if blocked {
        libc::SIG_BLOCK
    } else {
        libc::SIG_UNBLOCK
    };
    // SAFETY: `set` is an initialized signal set; a null old-set pointer means the previous mask
    // is not returned, which `pthread_sigmask` allows.
    unsafe { libc::pthread_sigmask(how, &set, std::ptr::null_mut()) };
}

/// Installs `forward_sigterm` as the `SIGTERM` handler. Returns whether it was installed; if not,
/// says so on stderr, since the wrapped command then gets no chance to handle `SIGTERM`.
fn install_sigterm_forwarding() -> bool {
    let action = MaybeUninit::<libc::sigaction>::zeroed();
    // SAFETY: `sigaction` is a plain C struct, for which all zeroes is a valid value: no flags, an
    // empty mask and a null handler, which is set below.
    let mut action = unsafe { action.assume_init() };
    action.sa_sigaction = forward_sigterm as *const () as libc::sighandler_t;
    action.sa_flags = libc::SA_RESTART;
    // SAFETY: `action` is a valid `sigaction` whose handler only performs an atomic load and an
    // async-signal-safe `kill`; a null old-action pointer means the previous action is not
    // returned, which `sigaction` allows.
    let installed = unsafe { libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) == 0 };
    if !installed {
        eprintln!(
            "miniperf: failed to forward SIGTERM: {}",
            std::io::Error::last_os_error()
        );
    }
    installed
}

/// Runs `cmd`, forwarding `SIGTERM` to it.
///
/// `SIGTERM` is blocked from before the spawn until the handler knows the child's pid, so one that
/// arrives in between is forwarded rather than killing miniperf. The child inherits the signal mask,
/// so it unblocks `SIGTERM` before exec. Forwarding stops while the exited child is still unreaped,
/// so it never signals a reused pid. If the handler cannot be installed, `SIGTERM` keeps its default
/// action, as without forwarding.
fn run_forwarding_sigterm(cmd: &mut Command) -> std::io::Result<std::process::ExitStatus> {
    // SAFETY: `set_sigterm_blocked` only calls async-signal-safe functions.
    unsafe {
        cmd.pre_exec(|| {
            set_sigterm_blocked(false);
            Ok(())
        });
    }
    set_sigterm_blocked(true);
    let spawned = cmd.spawn();
    let forwarding_to = spawned
        .as_ref()
        .ok()
        .and_then(|child| i32::try_from(child.id()).ok())
        .filter(|pid| {
            CHILD_PID.store(*pid, Ordering::Relaxed);
            install_sigterm_forwarding()
        });
    set_sigterm_blocked(false);

    let mut child = spawned?;
    if let Some(pid) = forwarding_to {
        // On error (for example if the child was reaped already), `child.wait()` still reports
        // what can be known about it.
        let _ignored = buck2_process_kill::wait_for_child_exit(pid);
        set_sigterm_blocked(true);
        CHILD_PID.store(0, Ordering::Relaxed);
    }
    child.wait()
}

struct Counters<R = File> {
    user_counter: R,
    kernel_counter: R,
}

#[derive(thiserror::Error, Debug)]
#[error("Error at {}: {}", stage, error)]
struct CounterError {
    stage: &'static str,
    error: anyhow::Error,
}

impl Counters {
    fn open() -> Result<Self, CounterError> {
        // NOTE: Kernel is not enabled here: we want to report only userspace cycles.
        let mut user_counter_builder = Builder::new().kind(Hardware::INSTRUCTIONS);
        user_counter_builder
            .inherit(true)
            .enable_on_exec(true)
            .read_format(ReadFormat::TOTAL_TIME_ENABLED | ReadFormat::TOTAL_TIME_RUNNING);
        let user_counter = user_counter_builder.build().map_err(|error| CounterError {
            stage: "open user",
            error: error.into(),
        })?;

        let mut kernel_counter = Builder::new().kind(Hardware::INSTRUCTIONS);
        kernel_counter
            .include_kernel()
            .exclude_user(true)
            .inherit(true)
            .enable_on_exec(true)
            .read_format(ReadFormat::TOTAL_TIME_ENABLED | ReadFormat::TOTAL_TIME_RUNNING);

        let kernel_counter = kernel_counter.build().map_err(|error| CounterError {
            stage: "open kernel",
            error: error.into(),
        })?;

        Ok(Self {
            user_counter: into_file(user_counter),
            kernel_counter: into_file(kernel_counter),
        })
    }
}

impl<R: Read> Counters<R> {
    fn collect(mut self) -> Result<MiniperfCounters, CounterError> {
        let user_instructions =
            read_counter(&mut self.user_counter).map_err(|error| CounterError {
                stage: "collect user",
                error: error.into(),
            })?;

        let kernel_instructions =
            read_counter(&mut self.kernel_counter).map_err(|error| CounterError {
                stage: "collect kernel",
                error: error.into(),
            })?;

        Ok(MiniperfCounters {
            user_instructions,
            kernel_instructions,
        })
    }
}

/// Size of a read with `TOTAL_TIME_ENABLED | TOTAL_TIME_RUNNING`: the count,
/// `time_enabled` and `time_running`, each a native-endian `u64`.
const COUNTER_READ_SIZE: usize = 3 * size_of::<u64>();

/// Transfers ownership of the counter's file descriptor.
fn into_file(counter: perf_event::Counter) -> File {
    // SAFETY: `into_raw_fd` relinquishes ownership of an open descriptor, which `File` takes over.
    unsafe { File::from_raw_fd(counter.into_raw_fd()) }
}

#[derive(thiserror::Error, Debug)]
enum CounterReadError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(
        "Kernel reported `time_running` ({time_running} ns) exceeding `time_enabled` ({time_enabled} ns), count {count}"
    )]
    RunningExceedsEnabled {
        count: u64,
        time_enabled: u64,
        time_running: u64,
    },
}

/// Reads a counter, rejecting values that cannot be scaled for multiplexing.
///
/// `perf_event::Counter::read_count_and_time` panics on inconsistent kernel timing. These reads
/// must return an error instead so that miniperf can still report the wrapped command's status.
fn read_counter(counter: &mut impl Read) -> Result<MiniperfCounter, CounterReadError> {
    let mut buf = [0; COUNTER_READ_SIZE];
    counter.read_exact(&mut buf)?;
    let field = |index: usize| {
        let start = index * size_of::<u64>();
        u64::from_ne_bytes(
            buf[start..start + size_of::<u64>()]
                .try_into()
                .expect("should be a `u64`-sized slice"),
        )
    };
    let (count, time_enabled, time_running) = (field(0), field(1), field(2));
    if time_running > time_enabled {
        return Err(CounterReadError::RunningExceedsEnabled {
            count,
            time_enabled,
            time_running,
        });
    }
    Ok(MiniperfCounter {
        count,
        time_enabled,
        time_running,
    })
}

fn collect_output<R: Read>(
    status: anyhow::Result<std::process::ExitStatus>,
    counters: Result<Counters<R>, CounterError>,
) -> MiniperfOutput {
    MiniperfOutput {
        raw_exit_code: status.map(|s| s.into_raw()).map_err(|e| e.to_string()),
        counters: counters
            .and_then(|c| c.collect())
            .map_err(|e| e.to_string()),
    }
}

/// First argument is an output path to write output data into. The rest is the command to execute.
pub fn main() -> anyhow::Result<()> {
    let mut args = env::args_os();
    args.next().context("No argv0")?;

    // In an ideal world, we would like this to be a pipe. Unfortunately, we can't do that, because
    // to get a pipe here, we'd have to have it not be CLOEXEC (because we need to exec *this*
    // binary). However, we're spawned by a server that creates many such processes concurrently,
    // so that means CLOEXEC must be set when creating the pipe, then unset between fork and exec.
    // To do this while retaining posix_spawn (which is quite a bit faster than fork + exec), we
    // need to dup the FD (which clears CLOEXEC), but the Rust wrapper around posix_spawn
    // (`Command`) does not expose that.
    let out = args.next().context("No output path")?;

    let counters = Counters::open();

    let status = args.next().context("No process to run").and_then(|bin| {
        // ast-grep-ignore: rust/buck2-no-command-new
        run_forwarding_sigterm(Command::new(bin).args(args)).map_err(anyhow::Error::from)
    });

    let output = collect_output(status, counters);

    // Stack allocate in the happy path.
    let mut buff = SmallVec::<[u8; MiniperfOutput::EXPECTED_SIZE]>::new();

    bincode::serde::encode_into_std_write(&output, &mut buff, bincode::config::legacy())
        .with_context(|| format!("Failed to write to `{out:?}`"))?;

    std::fs::write(&out, &buff).with_context(|| format!("Failed to write to `{out:?}`"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(values: [u64; 3]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect()
    }

    #[test]
    fn test_read_counter() {
        for (count, time_enabled, time_running) in [
            (123, 100, 50),
            (123, 100, 100),
            (0, 0, 0),
            (u64::MAX, u64::MAX, u64::MAX),
        ] {
            assert_eq!(
                read_counter(&mut encode([count, time_enabled, time_running]).as_slice()).unwrap(),
                MiniperfCounter {
                    count,
                    time_enabled,
                    time_running,
                }
            );
        }
    }

    #[test]
    fn test_read_counter_rejects_running_longer_than_enabled() {
        let error = read_counter(&mut encode([123, 100, 101]).as_slice()).unwrap_err();
        assert!(
            matches!(
                error,
                CounterReadError::RunningExceedsEnabled {
                    count: 123,
                    time_enabled: 100,
                    time_running: 101,
                }
            ),
            "Inconsistent times should be an error carrying the values read, got {error:?}"
        );
        assert_eq!(
            error.to_string(),
            "Kernel reported `time_running` (101 ns) exceeding `time_enabled` (100 ns), count 123"
        );
    }

    #[test]
    fn test_read_counter_reports_short_read() {
        let buf = encode([123, 100, 50]);
        let error = read_counter(&mut &buf[..COUNTER_READ_SIZE - 1]).unwrap_err();
        assert!(
            matches!(&error, CounterReadError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof),
            "A short read should be an I/O error, got {error:?}"
        );
    }

    #[test]
    fn test_collect_output_keeps_exit_status_when_counter_times_are_inconsistent() {
        let user = encode([10, 100, 100]);
        let kernel = encode([20, 100, 101]);
        for raw_exit_code in [0, 3 << 8, libc::SIGTERM] {
            let output = collect_output(
                Ok(std::process::ExitStatus::from_raw(raw_exit_code)),
                Ok(Counters {
                    user_counter: user.as_slice(),
                    kernel_counter: kernel.as_slice(),
                }),
            );
            let bytes = bincode::serde::encode_to_vec(&output, bincode::config::legacy()).unwrap();
            let (output, _) = bincode::serde::decode_from_slice::<MiniperfOutput, _>(
                &bytes,
                bincode::config::legacy(),
            )
            .unwrap();

            assert_eq!(
                output.raw_exit_code,
                Ok(raw_exit_code),
                "The wrapped command's exit status should survive a bad counter read"
            );
            assert_eq!(
                output.counters.err().as_deref(),
                Some(
                    "Error at collect kernel: Kernel reported `time_running` (101 ns) exceeding `time_enabled` (100 ns), count 20"
                ),
                "The counter error should name the stage and the values read"
            );
        }
    }

    #[test]
    fn test_collect_output_keeps_spawn_error_when_user_counter_times_are_inconsistent() {
        let user = encode([10, 0, 1]);
        let kernel = encode([20, 100, 100]);
        let output = collect_output(
            Err(anyhow::anyhow!("Failed to spawn command")),
            Ok(Counters {
                user_counter: user.as_slice(),
                kernel_counter: kernel.as_slice(),
            }),
        );
        assert_eq!(
            output.raw_exit_code.err().as_deref(),
            Some("Failed to spawn command")
        );
        assert_eq!(
            output.counters.err().as_deref(),
            Some(
                "Error at collect user: Kernel reported `time_running` (1 ns) exceeding `time_enabled` (0 ns), count 10"
            )
        );
    }
}
