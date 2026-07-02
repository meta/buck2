/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::ffi::OsString;
use std::fmt::Display;
use std::io;
use std::io::Write;
use std::ops::FromResidual;
use std::process::Command;

use buck2_data::ErrorReport;
use buck2_error::ErrorTag;
use buck2_error::ExitCode;
use buck2_error::classify::ErrorLike;
use buck2_error::classify::ErrorTagExtra;
use buck2_error::classify::best_error;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_wrapper_common::invocation_id::TraceId;

#[derive(Debug)]
pub struct ExecArgs {
    prog: OsString,
    argv: Vec<OsString>,
    chdir: Option<AbsPathBuf>,
    environment: ExecEnvironment,
}

/// Environment changes applied only to the process launched by [`ExitResult::exec`].
#[derive(Debug, Default)]
pub struct ExecEnvironment {
    /// Variables to set or replace.
    pub set: Vec<(String, String)>,
    /// Variables to remove from the inherited environment.
    pub remove: Vec<String>,
}

/// ExitResult represents the outcome of a process execution where we care to return a specific
/// exit code. This is designed to be used as the return value from `main()`.
///
/// We can easily turn a buck2_error::Result (or buck2_error::Error, or even a message) into a ExitResult,
/// but the reverse is not possible: once created, the only useful thing we can with a
/// ExitResult is propagate it.
#[must_use]
#[derive(Debug)]
pub struct ExitResult {
    variant: ExitResultVariant,

    /// Some stdout output that should be emitted prior to exiting. This allows commands to buffer
    /// their final output and choose not to send it if we opt to restart the command.
    stdout: Vec<u8>,

    // Errors that should have been written to the console already.
    // These should be written to the command report, but won't be written to the console before exiting via `report()`
    emitted_errors: Vec<ErrorReport>,
}

#[derive(Debug)]
enum ExitResultVariant {
    /// We finished successfully, return the specific exit code.
    Status(ExitCode),
    /// Instead of terminating normally, `exec` (or spawn on Windows)
    /// a new process with the given name and argv.
    /// This is used to implement `buck2 run`.
    Exec(ExecArgs),
    /// We failed (i.e. due to a Buck internal error).
    /// At this time, when execution does fail, we print out the error message to stderr.
    StatusWithErr(ExitCode, buck2_error::Error),
}

impl ExitResult {
    pub fn success() -> Self {
        Self::status(ExitCode::Success)
    }

    pub fn is_success(&self) -> bool {
        matches!(self.variant, ExitResultVariant::Status(ExitCode::Success))
    }

    /// Return the stored error that hasn't been shown to the user yet, if there is one.
    pub fn get_error(&self) -> Option<buck2_error::Error> {
        if let ExitResultVariant::StatusWithErr(_, err) = &self.variant {
            Some(err.clone())
        } else {
            None
        }
    }

    /// Get all errors, emitted or not, for the command report and invocation record.
    pub fn get_all_errors(&self) -> Vec<ErrorReport> {
        let mut errors = self.emitted_errors.clone();
        if let ExitResultVariant::StatusWithErr(_, e) = &self.variant {
            errors.push(e.into());
        }
        errors
    }

    /// Only use for commands that did not fail, otherwise return an error
    fn status(status: ExitCode) -> Self {
        Self {
            variant: ExitResultVariant::Status(status),
            stdout: Vec::new(),
            emitted_errors: Vec::new(),
        }
    }

    pub fn status_with_emitted_errors(status: ExitCode, emitted_errors: Vec<ErrorReport>) -> Self {
        Self {
            variant: ExitResultVariant::Status(status),
            stdout: Vec::new(),
            emitted_errors,
        }
    }

    pub fn exec(
        prog: OsString,
        argv: Vec<OsString>,
        chdir: Option<AbsPathBuf>,
        environment: ExecEnvironment,
    ) -> Self {
        Self {
            variant: ExitResultVariant::Exec(ExecArgs {
                prog,
                argv,
                chdir,
                environment,
            }),
            stdout: Vec::new(),
            emitted_errors: Vec::new(),
        }
    }

    pub fn bail(msg: impl Display) -> Self {
        Self::err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Bail,
            "Command failed: {}",
            msg
        ))
    }

    pub fn signal_interrupt() -> Self {
        Self::status(ExitCode::SignalInterrupt)
    }

    pub fn timeout() -> Self {
        Self::status(ExitCode::Timeout)
    }

    pub fn err(err: buck2_error::Error) -> Self {
        Self {
            variant: ExitResultVariant::StatusWithErr(err.exit_code(), err),
            stdout: Vec::new(),
            emitted_errors: Vec::new(),
        }
    }

    pub fn err_with_exit_code(err: buck2_error::Error, exit_code: ExitCode) -> Self {
        Self {
            variant: ExitResultVariant::StatusWithErr(exit_code, err),
            stdout: Vec::new(),
            emitted_errors: Vec::new(),
        }
    }

    /// Return this ExitStatus or call a function to produce a new one.
    pub fn or_else(self, f: impl FnOnce(Self) -> Self) -> Self {
        if matches!(self.variant, ExitResultVariant::Status(ExitCode::Success)) {
            return self;
        }

        f(self)
    }

    pub fn with_stdout(mut self, stdout: Vec<u8>) -> Self {
        self.stdout.extend(stdout);
        self
    }

    pub fn report(self) -> ! {
        match crate::stdio::print_bytes(&self.stdout) {
            Ok(()) => self.variant.report(),
            Err(e) => Self::err(e).variant.report(),
        }
    }

    pub fn from_command_result_errors(errors: Vec<ErrorReport>) -> Self {
        fn status_with_error_report(status: ExitCode, errors: Vec<ErrorReport>) -> ExitResult {
            ExitResult {
                variant: ExitResultVariant::Status(status),
                stdout: Vec::new(),
                emitted_errors: errors,
            }
        }
        let exit_code = best_error(&errors)
            .and_then(|e| e.best_tag())
            .map_or(ExitCode::UnknownFailure, |t| t.exit_code());
        status_with_error_report(exit_code, errors)
    }

    /// Buck2 supports being built as both a "full" binary as well as a "client-only" binary.
    ///
    /// However, some commands (eg `--no-buckd`) are not supported in the client-only binary, and so
    /// when these commands are run, we have to retry them with the full build.
    ///
    /// This function is called in those cases. It returns `Some` only for client-only builds.
    pub fn retry_command_with_full_binary() -> buck2_error::Result<Option<Self>> {
        if buck2_core::client_only::is_client_only()? {
            let exe = crate::daemon::client::connect::get_daemon_exe()?;
            Ok(Some(ExitResult::exec(
                exe.into_os_string(),
                std::env::args_os().collect(),
                None,
                ExecEnvironment::default(),
            )))
        } else {
            Ok(None)
        }
    }

    /// Return the exit code to be returned unless a run command will be exec'd, in which case we don't know what the final exit code will be.
    pub fn exit_code(&self) -> Option<ExitCode> {
        if let ExitResultVariant::Status(exit_code)
        | ExitResultVariant::StatusWithErr(exit_code, _) = self.variant
        {
            Some(exit_code)
        } else {
            None
        }
    }

    /// Return the name of the exit code for logging, or the reason there is no exit code (EXEC).
    pub fn name(&self) -> &'static str {
        match self.variant {
            ExitResultVariant::Status(exit_code)
            | ExitResultVariant::StatusWithErr(exit_code, _) => exit_code.name(),
            ExitResultVariant::Exec(_) => "EXEC",
        }
    }

    pub fn write_command_report(
        &self,
        trace_id: TraceId,
        buck_log_dir: Option<AbsNormPathBuf>,
        command_report_path: Option<AbsPathBuf>,
        finalizing_error_messages: Vec<String>,
    ) -> buck2_error::Result<()> {
        let (path, copy_path) = if let Some(buck_log_dir) = buck_log_dir {
            let dir = buck_log_dir.join(ForwardRelativePath::new(&trace_id.to_string())?);
            fs_util::create_dir_all(&dir)?;
            // this path is used by the buck wrapper, don't change without updating the wrapper.
            let path = dir.join(ForwardRelativePath::new("command_report.json")?);
            (path.into_abs_path_buf(), command_report_path)
        } else if let Some(command_report_path) = command_report_path {
            // If buck_log_dir is not set, we are running outside a repo, write to command_report_path if present.
            (command_report_path, None)
        } else {
            // No buck_log_dir, no command_report_path, do nothing.
            return Ok(());
        };
        let file = fs_util::create_file(&path).categorize_internal()?;
        let mut file = std::io::BufWriter::new(file);

        let error_messages = self
            .get_all_errors()
            .iter()
            .map(|e| e.message.clone())
            .collect();

        if let Some(exit_code) = self.exit_code() {
            serde_json::to_writer_pretty(
                &mut file,
                &buck2_data::CommandReport {
                    trace_id: trace_id.to_string(),
                    exit_code: exit_code.exit_code(),
                    error_messages,
                    finalizing_error_messages,
                },
            )?;

            if let Some(report_path) = copy_path {
                if let Some(parent) = report_path.parent() {
                    fs_util::create_dir_all(parent)?;
                }
                // buck wrapper depends on command report being written.
                file.flush()?;
                // input path from --command-report-path
                fs_util::copy(path, report_path).categorize_input()?;
            }
        }

        Ok(())
    }
}

/// We can produce a ExitResult from a `buck2_error::Result` for convenience.
impl From<buck2_error::Result<()>> for ExitResult {
    fn from(e: buck2_error::Result<()>) -> Self {
        match e {
            Ok(()) => Self::success(),
            Err(e) => Self::err(e),
        }
    }
}

impl From<buck2_error::Result<ExitCode>> for ExitResult {
    fn from(e: buck2_error::Result<ExitCode>) -> Self {
        match e {
            Ok(code) => Self::status(code),
            Err(e) => Self::err(e),
        }
    }
}

impl From<buck2_error::Error> for ExitResult {
    fn from(e: buck2_error::Error) -> Self {
        Self::err(e)
    }
}

impl FromResidual<buck2_error::Error> for ExitResult {
    #[track_caller]
    fn from_residual(residual: buck2_error::Error) -> ExitResult {
        Self::err(residual)
    }
}

impl<E: Into<::buck2_error::Error>> FromResidual<Result<Infallible, E>> for ExitResult {
    #[track_caller]
    fn from_residual(residual: Result<Infallible, E>) -> ExitResult {
        match residual {
            // E -> buck2_error::Error -> ExitResult
            Err(e) => Self::err(e.into()),
        }
    }
}

/// Implementing Termination lets us set the exit code for the process.
impl ExitResultVariant {
    pub fn report(self) -> ! {
        // Log the exit timestamp
        tracing::debug!("Client exiting");

        // Drain any buffered OTLP spans before we leave. Every variant below either calls
        // `libc::_exit` or `exec`s a new process image -- neither runs destructors, so the batch
        // exporter would otherwise drop its final spans on the floor.
        // See https://github.com/open-telemetry/opentelemetry-rust/issues/1961.
        buck2_core::logging::otel::shutdown();
        // NOTE: We use writeln instead of println so we don't panic if stderr is closed. This
        // ensures we get the desired exit code printed instead of potentially a panic.
        let mut exit_code = match self {
            Self::Status(v) => v,
            Self::Exec(args) => {
                // Terminate by exec-ing a new process - usually because of `buck2 run`.
                //
                // execv does not return.
                execv(args)
            }
            Self::StatusWithErr(exit_code, e) => {
                tracing::debug!("Exiting with {:?} ({:?})", exit_code, e);

                match exit_code {
                    ExitCode::SignalInterrupt | ExitCode::ClientIoBrokenPipe => {
                        // No logging for those.
                    }
                    _ => {
                        let _ignored = writeln!(io::stderr().lock(), "Command failed: {e:?}");
                    }
                }

                exit_code
            }
        };

        // Global destructors in C++ dependencies destroy global state,
        // while running background threads rely on this state.
        // So the result is non-reproducible crash of the buck2 client.
        // https://fburl.com/7u7kizm7
        // So let's disable global destructors.
        // Global destructors are hard (if even possible) to do safely anyway.

        if io::stdout().flush().is_err() {
            exit_code = ExitCode::SignalInterrupt;
        }

        // Stderr should be autoflushed, but just in case...
        if io::stderr().flush().is_err() {
            exit_code = ExitCode::SignalInterrupt;
        }

        unsafe { libc::_exit(exit_code.exit_code() as libc::c_int) }
    }
}

/// A wrapper around an `io::Error` which indicates that the error came from "client IO".
///
/// We use this to inform the exit code generation
#[derive(buck2_error::Error, derivative::Derivative)]
#[derivative(Debug = "transparent")]
#[error(transparent)]
pub enum ClientIoError {
    /// A broken pipe when writing to stdout is expected if stdout is closed before the command finishes.
    /// An easy way to trigger this is `buck2 audit config | head`
    #[buck2(tag = IoClientBrokenPipe)]
    #[buck2(environment)]
    BrokenPipe(io::Error),
    #[buck2(tier0)]
    OtherIo(io::Error),
    #[buck2(tier0)]
    Other(buck2_error::Error),
}

impl From<buck2_error::Error> for ClientIoError {
    fn from(error: buck2_error::Error) -> Self {
        ClientIoError::Other(error)
    }
}

impl From<csv::Error> for ClientIoError {
    fn from(error: csv::Error) -> Self {
        match error.kind() {
            csv::ErrorKind::Io(_) => {}
            _ => return ClientIoError::Other(from_any_with_tag(error, ErrorTag::CsvParse)),
        }
        match error.into_kind() {
            csv::ErrorKind::Io(io_error) => ClientIoError::from(io_error),
            // Can't clone the error or move kind and create a new csv::Error.
            _ => unreachable!("csv::Error must be an io::Error"),
        }
    }
}

impl From<serde_json::Error> for ClientIoError {
    fn from(error: serde_json::Error) -> Self {
        let error: io::Error = error.into();
        ClientIoError::from(error)
    }
}

impl From<io::Error> for ClientIoError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::BrokenPipe {
            ClientIoError::BrokenPipe(error)
        } else {
            ClientIoError::OtherIo(error)
        }
    }
}

#[cfg(windows)]
fn do_exec(command: &mut Command) -> buck2_error::Error {
    let status = match command.status() {
        Ok(status) => status,
        Err(e) => return e.into(),
    };
    let code = status.code().unwrap_or(1);
    unsafe { libc::_exit(code as libc::c_int) }
}

#[cfg(unix)]
fn do_exec(command: &mut Command) -> buck2_error::Error {
    use std::os::unix::process::CommandExt;

    command.exec().into()
}

fn apply_exec_environment(command: &mut Command, environment: ExecEnvironment) {
    for key in environment.remove {
        command.env_remove(key);
    }
    for (key, value) in environment.set {
        command.env(key, value);
    }
}

/// Invokes the given program with the given argv and replaces the program image with the new program.
/// Does not return.
fn execv(args: ExecArgs) -> ! {
    // ast-grep-ignore: rust/buck2-no-command-new
    let mut command = Command::new(&args.prog);
    command.args(&args.argv[1..]);
    if let Some(dir) = args.chdir {
        // Note here we break `cwd::cwd_will_not_change` promise.
        // This is OK because we don't return from this function
        // (otherwise this would be a really bad idea, even without the promise).
        command.current_dir(dir);
    }
    apply_exec_environment(&mut command, args.environment);
    let err = do_exec(&mut command).context(format!(
        "Failed to execute target process, running {:?} {:?}",
        args.prog, args.argv
    ));
    ExitResult::err(err).report()
}

#[cfg(test)]
mod tests {
    use buck2_util::process::background_command;

    use super::*;

    #[test]
    fn exec_environment_applies_additions_and_removals() {
        let mut command = background_command("program");
        apply_exec_environment(
            &mut command,
            ExecEnvironment {
                set: vec![("SET_ME".to_owned(), "value".to_owned())],
                remove: vec!["REMOVE_ME".to_owned()],
            },
        );

        let environment: Vec<_> = command.get_envs().collect();
        assert!(environment.contains(&(
            std::ffi::OsStr::new("SET_ME"),
            Some(std::ffi::OsStr::new("value"))
        )));
        assert!(environment.contains(&(std::ffi::OsStr::new("REMOVE_ME"), None)));
    }
}
