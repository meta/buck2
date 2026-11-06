/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_common::argv::Argv;
use buck2_common::argv::SanitizedArgv;
use buck2_log_common::chrome_trace::ChromeTraceCommand;

use crate::allocative::AllocativeCommand;
use crate::allocator_stats::AllocatorStatsCommand;
use crate::anon_targets::AnonTargetsCommand;
use crate::crash::CrashCommand;
use crate::daemon_dir::DaemonDirCommand;
use crate::dice_dump::DiceDumpCommand;
use crate::eval::EvalCommand;
use crate::exe::ExeCommand;
use crate::file_status::FileStatusCommand;
use crate::flush_bes::FlushBesCommand;
use crate::flush_dep_files::FlushDepFilesCommand;
use crate::flush_pgo_profile::FlushPgoProfileCommand;
use crate::heap_dump::HeapDumpCommand;
use crate::hydration::HydrationCommand;
use crate::internal_version::InternalVersionCommand;
use crate::log_perf::LogPerfCommand;
use crate::paranoid::ParanoidCommand;
use crate::persist_event_logs::PersistEventLogsCommand;
use crate::recent_alloc_dump::RecentAllocDumpCommand;
use crate::set_log_filter::SetLogFilterCommand;
use crate::tenants::TenantsCommand;
use crate::thread_dump::ThreadDumpCommand;
use crate::trace_io::TraceIoCommand;
use crate::upload_re_logs::UploadReLogsCommand;

mod allocative;
mod allocator_stats;
mod anon_targets;
mod crash;
mod daemon_dir;
mod dice_dump;
mod eval;
mod exe;
mod file_status;
mod flush_bes;
mod flush_dep_files;
mod flush_pgo_profile;
mod heap_dump;
mod hydration;
mod internal_version;
mod log_perf;
mod paranoid;
mod persist_event_logs;
mod recent_alloc_dump;
mod set_log_filter;
mod tenants;
mod thread_dump;
mod trace_io;
mod upload_re_logs;

#[derive(Debug, clap::Parser)]
#[clap(about = "Hidden debug commands useful for testing buck2")]
pub enum DebugCommand {
    /// Deliberately crashes the Buck daemon, for testing purposes.
    Crash(CrashCommand),
    HeapDump(HeapDumpCommand),
    /// Dumps jemalloc's recent-allocations ring (individual allocations with
    /// sizes, alloc/free stacks, and lifetimes) to a JSON file.
    RecentAllocDump(RecentAllocDumpCommand),
    /// Dumps allocator stat
    AllocatorStats(AllocatorStatsCommand),
    /// Dump the DICE graph to a file and saves it to disk.
    DiceDump(DiceDumpCommand),
    /// Prints the hash of the buck2 binary
    InternalVersion(InternalVersionCommand),
    /// Renders an event-log to a Chrome trace file for inspection with a browser.
    ChromeTrace(ChromeTraceCommand),
    /// Flushes all dep files known to Buck2.
    FlushDepFiles(FlushDepFilesCommand),
    /// Closes the BES streams of finished commands; exits non-zero unless the server
    /// acknowledged every event within `--timeout`.
    FlushBes(FlushBesCommand),
    /// Flush PGO profile data from the daemon to disk.
    FlushPgoProfile(FlushPgoProfileCommand),
    // Upload RE logs given an RE session ID
    UploadReLogs(UploadReLogsCommand),
    /// Validates that Buck2 and disk agree on the state of files.
    FileStatus(FileStatusCommand),
    /// Prints buck2 daemon directory (`~/.buckd/xxx`).
    DaemonDir(DaemonDirCommand),
    /// Prints buck2 executable (this executable) path.
    Exe(ExeCommand),
    Allocative(AllocativeCommand),
    SetLogFilter(SetLogFilterCommand),
    /// Make sense of log perf
    LogPerf(LogPerfCommand),
    /// Interact with I/O tracing of the daemon.
    TraceIo(TraceIoCommand),
    #[doc(hidden)]
    PersistEventLogs(PersistEventLogsCommand),
    #[clap(subcommand)]
    Paranoid(ParanoidCommand),
    Eval(EvalCommand),
    /// List the anon targets requested while analyzing the deps closure of the given
    /// targets.
    AnonTargets(AnonTargetsCommand),
    ThreadDump(ThreadDumpCommand),
    /// List tenants currently registered in the daemon.
    Tenants(TenantsCommand),
    /// Control DICE node value page-out / page-in.
    #[clap(subcommand)]
    Hydration(HydrationCommand),
}

impl DebugCommand {
    pub fn exec(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let matches = matches.unwrap_subcommand();
        match self {
            DebugCommand::DiceDump(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::Crash(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::HeapDump(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::RecentAllocDump(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::AllocatorStats(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::InternalVersion(cmd) => cmd.exec(matches, ctx),
            DebugCommand::ChromeTrace(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::FlushDepFiles(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::FlushBes(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::FlushPgoProfile(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::UploadReLogs(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::DaemonDir(cmd) => cmd.exec(matches, ctx),
            DebugCommand::Exe(cmd) => cmd.exec(matches, ctx),
            DebugCommand::Allocative(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::SetLogFilter(cmd) => cmd.exec(matches, ctx),
            DebugCommand::FileStatus(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::LogPerf(cmd) => cmd.exec(matches, ctx),
            DebugCommand::TraceIo(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::PersistEventLogs(cmd) => cmd.exec(matches, ctx, events_ctx),
            DebugCommand::Paranoid(cmd) => cmd.exec(matches, ctx),
            DebugCommand::Eval(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::AnonTargets(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::ThreadDump(cmd) => cmd.exec(matches, ctx),
            DebugCommand::Tenants(cmd) => ctx.exec(cmd, matches, events_ctx),
            DebugCommand::Hydration(cmd) => ctx.exec(cmd, matches, events_ctx),
        }
    }

    pub fn sanitize_argv(&self, argv: Argv) -> SanitizedArgv {
        argv.no_need_to_sanitize()
    }
}
