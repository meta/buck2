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
use std::mem::MaybeUninit;
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

struct Counters {
    user_counter: perf_event::Counter,
    kernel_counter: perf_event::Counter,
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
        user_counter_builder.inherit(true).enable_on_exec(true);
        let user_counter = user_counter_builder.build().map_err(|error| CounterError {
            stage: "open user",
            error: error.into(),
        })?;

        let mut kernel_counter = Builder::new().kind(Hardware::INSTRUCTIONS);
        kernel_counter
            .include_kernel()
            .exclude_user(true)
            .inherit(true)
            .enable_on_exec(true);

        let kernel_counter = kernel_counter.build().map_err(|error| CounterError {
            stage: "open kernel",
            error: error.into(),
        })?;

        Ok(Self {
            user_counter,
            kernel_counter,
        })
    }

    fn collect(mut self) -> Result<MiniperfCounters, CounterError> {
        let user_value = self
            .user_counter
            .read_count_and_time()
            .map_err(|error| CounterError {
                stage: "collect user",
                error: error.into(),
            })?;

        let kernel_value =
            self.kernel_counter
                .read_count_and_time()
                .map_err(|error| CounterError {
                    stage: "collect kernel",
                    error: error.into(),
                })?;

        Ok(MiniperfCounters {
            user_instructions: MiniperfCounter {
                count: user_value.count,
                time_enabled: user_value.time_enabled,
                time_running: user_value.time_running,
            },
            kernel_instructions: MiniperfCounter {
                count: kernel_value.count,
                time_enabled: kernel_value.time_enabled,
                time_running: kernel_value.time_running,
            },
        })
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

    let counters = counters.and_then(|c| c.collect());

    let output = MiniperfOutput {
        raw_exit_code: status.map(|s| s.into_raw()).map_err(|e| e.to_string()),
        counters: counters.map_err(|e| e.to_string()),
    };

    // Stack allocate in the happy path.
    let mut buff = SmallVec::<[u8; MiniperfOutput::EXPECTED_SIZE]>::new();

    bincode::serde::encode_into_std_write(&output, &mut buff, bincode::config::legacy())
        .with_context(|| format!("Failed to write to `{out:?}`"))?;

    std::fs::write(&out, &buff).with_context(|| format!("Failed to write to `{out:?}`"))?;

    Ok(())
}
