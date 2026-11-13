/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use buck2::exec;
use buck2::panic;
use buck2::process_context::ClientRuntime;
use buck2::process_context::ProcessContext;
use buck2::process_context::SharedProcessContext;
use buck2_build_info::BUCK2_BUILD_INFO;
use buck2_build_info::Buck2BuildInfo;
use buck2_client_ctx::argfiles::StdinArgfile;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::restarter::RestartRequest;
use buck2_client_ctx::restarter::RestartState;
use buck2_client_ctx::restarter::Restarter;
use buck2_client_ctx::stdio;
use buck2_client_ctx::subscribers::recorder::InvocationRecorder;
use buck2_core::buck2_env;
use buck2_core::logging::LogConfigurationReloadHandle;
use buck2_core::logging::init_tracing_for_writer;
use buck2_core::logging::log_file::TracingLogFile;
use buck2_fs::working_dir::AbsWorkingDir;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;
use superconsole::Stdin;

// fbcode likes to set its own allocator in fbcode.default_allocator
// So when we set our own allocator, buck build buck2 or buck2 build buck2 often breaks.
// Making jemalloc the default only when we do a cargo build.
#[global_allocator]
#[cfg(all(
    any(target_os = "linux", target_os = "macos"),
    not(buck_build),
    not(buck2_memfrag)
))]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
#[global_allocator]
#[cfg(all(target_os = "windows", not(buck2_memfrag)))]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Opt-in fragmentation profiler, enabled with `-c buck2_dev.memfrag=true`. It wraps
// the system allocator (jemalloc in an fbcode build) rather than bringing its
// own, so it doesn't double-link jemalloc the way the tikv allocator above
// would. It stays a passthrough unless BUCK2_MEMFRAG_SHIFT is set at process
// start, so a memfrag build can still serve as a normal daemon.
#[global_allocator]
#[cfg(buck2_memfrag)]
static ALLOC: mem_frag::SamplingAlloc = mem_frag::SamplingAlloc::new();

fn init_logging() -> buck2_error::Result<Arc<dyn LogConfigurationReloadHandle>> {
    static ENV_TRACING_LOG_FILE_PATH: &str = "BUCK_LOG_TO_FILE_PATH";

    let handle = match std::env::var_os(ENV_TRACING_LOG_FILE_PATH) {
        Some(path) => {
            let path = PathBuf::from(path);
            // we set the writer to stderr first until later, when we have the logdir, set the
            // tracing log sink to that file

            fs::create_dir_all(&path)?;
            let tracing_log = path.join("tracing_log");
            let file = TracingLogFile::new(tracing_log)?;
            init_tracing_for_writer(file)
        }
        _ => init_tracing_for_writer(io::stderr),
    }?;

    #[cfg(fbcode_build)]
    {
        use buck2_event_log::should_upload_log;
        use buck2_events::sink::remote;

        if !should_upload_log()? {
            remote::disable();
        }
    }

    Ok(handle)
}

// When using a cargo build, some essential services (e.g. RE, scribe)
// fall back to slow paths that give terrible performance.
// Therefore, if we are using cargo, warn strongly.
fn check_cargo() {
    if !cfg!(fbcode_build) && !buck2_core::is_open_source() {
        eprintln!("=====================================================================");
        eprintln!("WARNING: You are using Buck v2 compiled with `cargo`, not `buck`.");
        eprintln!("         Some operations may go slower and logging may be impaired.");
        eprintln!("=====================================================================");
        eprintln!();
    }
}

fn print_retry() -> buck2_error::Result<()> {
    buck2_client_ctx::eprintln!("============================================================")?;
    buck2_client_ctx::eprintln!("|| Buck2 has detected that it needs to restart to proceed ||")?;
    buck2_client_ctx::eprintln!("|| Your command will now restart.                         ||")?;
    buck2_client_ctx::eprintln!("============================================================")?;
    buck2_client_ctx::eprintln!()?;
    Ok(())
}

/// Makes one attempt at the command. Returns a `RestartRequest` if it is to be restarted.
fn exec_with_logging(
    trace_id: TraceId,
    start_time: SystemTime,
    restarted_trace_id: Option<TraceId>,
    restart_state: RestartState,
    shared: buck2_error::Result<SharedProcessContext>,
    runtime: &mut ClientRuntime,
) -> (
    Option<SharedProcessContext>,
    ExitResult,
    Option<RestartRequest>,
) {
    let args = std::env::args().collect::<Vec<String>>();
    let recorder = InvocationRecorder::new(trace_id.dupe(), restarted_trace_id, start_time, args);
    let mut events_ctx = EventsCtx::new(Some(recorder), vec![]);
    events_ctx.restart_state = restart_state;
    let (shared, res) = match shared {
        Ok(mut shared) => {
            let res = exec(ProcessContext {
                trace_id: trace_id.dupe(),
                events_ctx: &mut events_ctx,
                shared: &mut shared,
                runtime,
                start_time,
            });
            (Some(shared), res)
        }
        Err(e) => (None, e.into()),
    };
    let (res, restart_request) = match runtime.get_or_init() {
        Ok(runtime) => events_ctx.finalize_events(trace_id, res, runtime),
        Err(e) => (e.into(), None),
    };
    (shared, res, restart_request)
}

// As this main() is used as the entry point for the `buck daemon` command,
// it must be single-threaded. Commands that want to be multi-threaded/async
// will start up their own tokio runtime.
fn main() -> ! {
    // Enable sampling early so the side table exists before the daemon forks
    // (fork copies the heap). The dump watcher is started later, post-fork, in
    // buck2_daemon — threads do not survive the daemonizing fork.
    #[cfg(buck2_memfrag)]
    mem_frag::init();

    buck2_core::client_only::CLIENT_ONLY_VAL.init(cfg!(client_only));
    #[cfg(not(client_only))]
    {
        buck2_analysis::init_late_bindings();
        buck2_anon_target::init_late_bindings();
        buck2_action_impl::init_late_bindings();
        buck2_cmd_audit_server::init_late_bindings();
        buck2_build_api::init_late_bindings();
        buck2_cmd_docs_server::init_late_bindings();
        buck2_cmd_lsp_server::init_late_bindings();
        buck2_external_cells::init_late_bindings();
        buck2_transition::init_late_bindings();
        buck2_build_signals_impl::init_late_bindings();
        buck2_bxl::init_late_bindings();
        buck2_cfg_constructor::init_late_bindings();
        buck2_configured::init_late_bindings();
        buck2_query_impls::init_late_bindings();
        buck2_interpreter_for_build::init_late_bindings();
        buck2_server_commands::init_late_bindings();
        buck2_cmd_targets_server::init_late_bindings();
        buck2_cmd_query_server::init_late_bindings();
        buck2_cmd_starlark_server::init_late_bindings();
        buck2_test::init_late_bindings();
        buck2_validation::init_late_bindings();
        buck2_events::init_late_bindings();
    }
    BUCK2_BUILD_INFO.init(Buck2BuildInfo {
        revision: std::option_env!("BUCK2_SET_EXPLICIT_VERSION"),
        win_internal_version: std::option_env!("BUCK2_WIN_INTERNAL_VERSION"),
        release_timestamp: std::option_env!("BUCK2_RELEASE_TIMESTAMP"),
    });

    // Set up crypto impl once per process
    buck2_certs::certs::setup_cryptography_or_fail();

    fn init_shared_context() -> buck2_error::Result<SharedProcessContext> {
        panic::initialize()?;
        check_cargo();

        // Log the start timestamp
        tracing::debug!("Client initialized logging");

        let stdin_buffer_size = buck2_env!(
            "BUCK2_TEST_STDIN_BUFFER_SIZE",
            type=usize,
            applicability=testing,
        )?
        .unwrap_or(8192);

        Ok(SharedProcessContext {
            log_reload_handle: init_logging()?,
            stdin: Stdin::new(stdin_buffer_size),
            working_dir: AbsWorkingDir::current_dir()?,
            args: std::env::args().collect::<Vec<String>>(),
            stdin_argfile: StdinArgfile::default(),
        })
    }

    fn main_with_result() -> ExitResult {
        let start_time = SystemTime::now();
        let first_trace_id = TraceId::from_env_or_new()?;
        let restarter = Restarter::try_new()?;
        let mut runtime = ClientRuntime::new();
        let shared = init_shared_context();
        let (shared, res, restart_request) = exec_with_logging(
            first_trace_id.dupe(),
            start_time,
            None,
            RestartState::Watching(restarter),
            shared,
            &mut runtime,
        );

        if let Some(shared) = shared {
            maybe_restart(first_trace_id, res, restart_request, shared, &mut runtime)
        } else {
            res
        }
    }

    /// Restarts the command if its first attempt asks for it and wrote nothing to stdout.
    fn maybe_restart(
        first_trace_id: TraceId,
        initial_result: ExitResult,
        restart_request: Option<RestartRequest>,
        shared: SharedProcessContext,
        runtime: &mut ClientRuntime,
    ) -> ExitResult {
        let restart_start_time = SystemTime::now();

        let Some(restart_request) = restart_request else {
            tracing::debug!("No restart was requested");
            return initial_result;
        };

        if stdio::has_written_to_stdout() {
            tracing::debug!("Cannot restart: wrote to stdout");
            return initial_result;
        }

        if print_retry().is_err() {
            tracing::debug!("Cannot restart: warning message cannot be printed");
            return initial_result;
        }

        let (_, res, _) = exec_with_logging(
            TraceId::new(),
            restart_start_time,
            Some(first_trace_id),
            RestartState::Restarted(restart_request),
            Ok(shared),
            runtime,
        );
        res
    }

    main_with_result().report()
}
