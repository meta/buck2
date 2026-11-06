/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp::max;
use std::cmp::min;
use std::io::Write;
use std::ops::Sub;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_action_parallelism::ActionInterval;
use buck2_cli_proto::command_result;
use buck2_common::build_count::BuildCount;
use buck2_common::build_count::BuildCountManager;
use buck2_common::convert::ProstDurationExt;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_core::buck2_env;
use buck2_core::soft_error;
use buck2_core::soft_error_or_panic;
use buck2_core::warn_user_or_panic;
use buck2_data::ErrorReport;
use buck2_data::FileWatcherProvider;
use buck2_data::FileWatcherStart;
use buck2_data::InvocationOutcome;
use buck2_data::ProcessedErrorReport;
use buck2_data::SchedulingMode;
use buck2_data::SoftError;
use buck2_data::SystemInfo;
use buck2_data::TargetCfg;
use buck2_data::error::ErrorTag;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ExitCode;
use buck2_error::Tier;
use buck2_error::buck2_error;
use buck2_error::classify::ERROR_TAG_UNCLASSIFIED;
use buck2_error::classify::ErrorLike;
use buck2_error::classify::source_area;
// Used by the Scribe path, which only an fbcode build has, and by the tests.
#[cfg_attr(not(any(fbcode_build, test)), allow(unused_imports))]
use buck2_error::internal_error;
use buck2_error::source_location::SourceLocation;
use buck2_event_log::ttl::manifold_event_log_ttl;
use buck2_event_observer::action_stats;
use buck2_event_observer::cache_hit_rate::total_cache_hit_rate;
use buck2_event_observer::last_command_execution_kind;
use buck2_event_observer::last_command_execution_kind::LastCommandExecutionKind;
use buck2_event_observer::last_command_execution_kind::get_last_command_execution_time;
use buck2_events::BuckEvent;
use buck2_events::daemon_id::DaemonId;
use buck2_events::metadata;
use buck2_events::sink::remote::RemoteEventConfig;
#[cfg(fbcode_build)]
use buck2_events::sink::remote::new_remote_event_sink_if_enabled;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_hash::IntentionallyStdHashMap;
use buck2_util::network_speed_average::NetworkSpeedAverage;
use buck2_util::sliding_window::SlidingWindow;
use buck2_wrapper_common::BUCK_WRAPPER_START_TIME_ENV_VAR;
use buck2_wrapper_common::invocation_id::TraceId;
use console::strip_ansi_codes;
use dupe::Dupe;
use itertools::Itertools;
use termwiz::istty::IsTty;
use tokio::sync::mpsc::Receiver;
use variant_name::VariantName;

use crate::agent_invocation_info::AgentInvocationInfo;
use crate::client_ctx::ClientCommandContext;
use crate::client_metadata::ClientMetadata;
use crate::common::CommonBuildConfigurationOptions;
use crate::common::CommonEventLogOptions;
use crate::common::PreemptibleWhen;
use crate::console_interaction_stream::ConsoleInteraction;
use crate::exit_result::ExitResult;
use crate::remote_sink_config;
use crate::subscribers::classify_server_stderr::classify_server_stderr;
use crate::subscribers::observer::ErrorObserver;
use crate::subscribers::subscriber::EventSubscriber;
use crate::subscribers::system_warning::check_download_speed;
use crate::subscribers::system_warning::check_memory_pressure;
use crate::subscribers::system_warning::check_remaining_disk_space;
use crate::subscribers::system_warning::is_disk_near_full;

pub fn process_memory(snapshot: &buck2_data::Snapshot) -> Option<u64> {
    // buck2_rss is the resident set size observed by daemon (exluding subprocesses).
    // On MacOS buck2_rss is not stored and also RSS in general is not a reliable indicator due to swapping which moves pages from resident set to disk.
    // Hence, we take max of buck2_rss and malloc_bytes_active (coming from jemalloc and is available on Macs as well).
    snapshot
        .malloc_bytes_active
        .into_iter()
        .chain(snapshot.buck2_rss)
        .max()
}

const MEMORY_PRESSURE_TAG: &str = "memory_pressure_warning";

const SYSTEM_LOAD1_WINDOW: Duration = Duration::from_secs(60);
const SYSTEM_LOAD5_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Soft errors the client itself reports, before or outside of any event stream. The daemon's
/// arrive as events; these are added to the record when it is written.
static CLIENT_SOFT_ERRORS: Mutex<Vec<SoftError>> = Mutex::new(Vec::new());

pub fn note_client_soft_error(soft_error: SoftError) {
    CLIENT_SOFT_ERRORS.lock().unwrap().push(soft_error);
}

pub struct InvocationRecorder {
    write_to_path: Option<AbsPathBuf>,
    command_name: Option<&'static str>,
    cli_args: Vec<String>,
    representative_config_flags: Vec<String>,
    isolation_dir: Option<String>,
    start_time: SystemTime,
    build_count_manager: Option<BuildCountManager>,
    trace_id: TraceId,
    command_end: Option<buck2_data::CommandEnd>,
    command_duration: Option<prost_types::Duration>,
    re_session_id: Option<String>,
    re_experiment_name: Option<String>,
    persistent_cache_mode: Option<String>,
    critical_path_duration: Option<Duration>,
    critical_path_page_in: Option<Duration>,
    tags: Vec<String>,
    run_local_count: u64,
    /// Dep-file cache work for this command, summed from its own events.
    dep_file_db_probes: u64,
    dep_file_db_probe_duration_us: u64,
    dep_file_db_fetches: u64,
    dep_file_db_fetch_duration_us: u64,
    dep_file_db_hits: u64,
    dep_file_db_writes_queued: u64,
    run_remote_count: u64,
    run_action_cache_count: u64,
    run_remote_dep_file_cache_count: u64,
    run_remote_served_from_cache_count: u64,
    run_remote_deduplicated_count: u64,
    run_skipped_count: u64,
    run_fallback_count: u64,
    run_fallback_re_queue_count: u64,
    run_local_only_count: u64,
    local_actions_executed_via_worker: u64,
    first_snapshot: Option<buck2_data::Snapshot>,
    last_snapshot: Option<buck2_data::Snapshot>,
    min_attempted_build_count_since_rebase: u64,
    min_build_count_since_rebase: u64,
    cache_upload_count: u64,
    cache_upload_attempt_count: u64,
    re_action_cache_query_error_count: u64,
    dep_file_upload_count: u64,
    dep_file_upload_attempt_count: u64,
    parsed_target_patterns: Option<buck2_data::ParsedTargetPatterns>,
    filesystem: Option<String>,
    watchman_version: Option<String>,
    eden_version: Option<String>,
    test_info: Option<String>,
    eligible_for_full_hybrid: bool,
    max_event_client_delay: Option<Duration>,
    max_malloc_bytes_active: Option<u64>,
    max_malloc_bytes_allocated: Option<u64>,
    run_command_failure_count: u64,
    event_count: u64,
    time_to_first_action_execution: Option<Duration>,
    materialization_output_size: u64,
    initial_materializer_entries_from_sqlite: Option<u64>,
    time_to_command_start: Option<Duration>,
    time_to_command_critical_section: Option<Duration>,
    time_to_first_analysis: Option<Duration>,
    time_to_load_first_build_file: Option<Duration>,
    time_to_first_command_execution_start: Option<Duration>,
    time_to_first_test_discovery: Option<Duration>,
    time_to_first_test_run: Option<Duration>,
    // We want to track the time to first arrival of each test result type
    // to better understand the user-experience around test execution
    time_to_first_pass_test_result: Option<Duration>,
    time_to_first_fail_test_result: Option<Duration>,
    time_to_first_skip_test_result: Option<Duration>,
    time_to_first_timeout_test_result: Option<Duration>,
    time_to_first_fatal_test_result: Option<Duration>,
    time_to_first_unknown_test_result: Option<Duration>,
    time_to_first_infra_failure_test_result: Option<Duration>,

    system_info: SystemInfo,
    paging_summary: Option<buck2_data::PagingSummary>,
    file_watcher_stats: Option<buck2_data::FileWatcherStats>,
    file_watcher_duration: Option<Duration>,
    time_to_last_action_execution_end: Option<Duration>,
    initial_sink_success_count: Option<u64>,
    initial_sink_failure_count: Option<u64>,
    initial_sink_dropped_count: Option<u64>,
    initial_sink_bytes_written: Option<u64>,
    sink_max_buffer_depth: u64,
    soft_error_categories: BuckMutSet<SoftError>,
    concurrent_command_blocking_duration: Option<Duration>,
    metadata: IntentionallyStdHashMap<String, String>,
    analysis_count: u64,
    load_count: u64,
    daemon_in_memory_state_is_corrupted: bool,
    daemon_materializer_state_is_corrupted: bool,
    enable_restarter: bool,
    restarted_trace_id: Option<TraceId>,
    preemptible: Option<PreemptibleWhen>,
    has_command_result: bool,
    has_end_of_stream: bool,
    compressed_event_log_size_bytes: Option<Arc<AtomicU64>>,
    remote_sink_config: RemoteEventConfig,
    critical_path_backend: Option<String>,
    bxl_ensure_artifacts_duration: Option<prost_types::Duration>,
    install_duration: Option<prost_types::Duration>,
    install_device_metadata: Vec<buck2_data::DeviceMetadata>,
    /// Daemon timestamp of `CommandStart`, the origin of `command_duration`.
    command_start_timestamp: Option<SystemTime>,
    build_phase_end: Option<(SystemTime, buck2_data::BuildStatus)>,
    installer_log_url: Option<String>,
    installer_name: Option<String>,
    initial_re_upload_bytes: Option<u64>,
    initial_re_download_bytes: Option<u64>,
    initial_zdb_download_queries: Option<u64>,
    initial_zdb_download_bytes: Option<u64>,
    initial_zdb_upload_queries: Option<u64>,
    initial_zdb_upload_bytes: Option<u64>,
    initial_zgateway_download_queries: Option<u64>,
    initial_zgateway_download_bytes: Option<u64>,
    initial_zgateway_upload_queries: Option<u64>,
    initial_zgateway_upload_bytes: Option<u64>,
    initial_manifold_download_queries: Option<u64>,
    initial_manifold_download_bytes: Option<u64>,
    initial_manifold_upload_queries: Option<u64>,
    initial_manifold_upload_bytes: Option<u64>,
    initial_hedwig_download_queries: Option<u64>,
    initial_hedwig_download_bytes: Option<u64>,
    initial_hedwig_upload_queries: Option<u64>,
    initial_hedwig_upload_bytes: Option<u64>,
    concurrent_command_ids: BuckMutSet<String>,
    re_exceeded_quotas: BuckMutSet<String>,
    daemon_connection_failure: bool,
    /// Daemon started by this command.
    daemon_was_started: Option<buck2_data::DaemonWasStartedReason>,
    should_restart: bool,
    settings_hash: Option<String>,
    client_metadata: Vec<buck2_data::ClientMetadata>,
    agent_invocation_info: AgentInvocationInfo,
    command_errors: Vec<ErrorReport>,
    exit_code: Option<u32>,
    exit_result_name: Option<String>,
    outcome: Option<InvocationOutcome>,
    /// To append to gRPC errors.
    server_stderr: String,
    target_rule_type_names: Vec<String>,
    re_max_download_speeds: Vec<SlidingWindow>,
    re_max_upload_speeds: Vec<SlidingWindow>,
    re_avg_download_speed: NetworkSpeedAverage,
    re_avg_upload_speed: NetworkSpeedAverage,
    peak_process_memory_bytes: Option<u64>,
    has_new_buckconfigs: bool,
    peak_used_disk_space_bytes: Option<u64>,
    peak_normalized_system_load1: Option<f64>,
    peak_normalized_system_load5: Option<f64>,
    active_networks_kinds: BuckMutSet<i32>,
    target_cfg: Option<TargetCfg>,
    hg_revision: Option<String>,
    git_revision: Option<String>,
    has_local_changes: Option<bool>,
    version_control_errors: Vec<String>,
    concurrent_commands: bool,
    initial_local_cache_hits_files: Option<i64>,
    initial_local_cache_hits_bytes: Option<i64>,
    initial_local_cache_misses_files: Option<i64>,
    initial_local_cache_misses_bytes: Option<i64>,
    materialization_files: u64,
    previous_uuid_with_mismatched_config: Option<String>,
    file_watcher: Option<String>,
    health_check_tags_receiver: Option<Receiver<Vec<String>>>,
    health_check_tags: BuckMutSet<String>,
    exec_time_ms: u64,
    initial_local_cache_hits_files_from_memory_cache: Option<i64>,
    initial_local_cache_hits_files_from_filesystem_cache: Option<i64>,
    initial_local_cache_lookups: Option<i64>,
    initial_local_cache_lookup_latency_microseconds: Option<i64>,
    max_dice_in_progress_keys: u64,
    max_dice_compute_keys: u64,
    current_in_progress_actions: u64,
    max_in_progress_actions: u64,
    current_in_progress_local_actions: u64,
    max_in_progress_local_actions: u64,
    current_in_progress_remote_actions: u64,
    max_in_progress_remote_actions: u64,
    current_in_progress_remote_uploads: u64,
    max_in_progress_remote_uploads: u64,
    // Execution-only intervals of each executed action, used to compute the
    // action-concurrency distribution emitted on the InvocationRecord.
    action_intervals: Vec<ActionInterval>,
    // Track executor stage types by span ID to know which counter to decrement on end
    executor_stages_by_span: BuckMutMap<u64, ExecutorStageType>,
    // Track maximum buck2 daemon anon memory usage
    memory_max_anon_allprocs: Option<u64>,
    // Track maximum buck2 forkserver anon memory usage
    memory_max_anon_forkserver_actions: Option<u64>,
    // Track maximum total buck2 daemon memory usage (anon+file+kernel)
    memory_max_total_allprocs: Option<u64>,
    // Track maximum total buck2 forkserver memory usage (anon+file+kernel)
    memory_max_total_forkserver_actions: Option<u64>,
    // Track peak allprocs swap usage (bytes)
    memory_max_swap_bytes_allprocs: Option<u64>,
    // Track peak allprocs memory pressure (PSI full avg10 %)
    memory_max_pressure_10s_avg_allprocs: Option<f64>,
    // Track peak allprocs memory pressure (PSI full avg60 %)
    memory_max_pressure_60s_avg_allprocs: Option<f64>,
    // CommandOptions data
    command_options: Option<buck2_data::CommandOptions>,
    // Initial IO counters captured at invocation start
    initial_io_copy_count: Option<u32>,
    initial_io_symlink_count: Option<u32>,
    initial_io_hardlink_count: Option<u32>,
    initial_io_mkdir_count: Option<u32>,
    initial_io_readdir_count: Option<u32>,
    initial_io_readdir_eden_count: Option<u32>,
    initial_io_rmdir_count: Option<u32>,
    initial_io_rmdir_all_count: Option<u32>,
    initial_io_stat_count: Option<u32>,
    initial_io_stat_eden_count: Option<u32>,
    initial_io_chmod_count: Option<u32>,
    initial_io_readlink_count: Option<u32>,
    initial_io_remove_count: Option<u32>,
    initial_io_rename_count: Option<u32>,
    initial_io_read_count: Option<u32>,
    initial_io_write_count: Option<u32>,
    initial_io_canonicalize_count: Option<u32>,
    initial_io_eden_settle_count: Option<u32>,
    repo_path: Option<String>,
}

#[derive(Clone, Debug)]
enum ExecutorStageType {
    LocalAction,
    RemoteAction,
    RemoteUpload,
}

impl InvocationRecorder {
    pub fn new(
        trace_id: TraceId,
        restarted_trace_id: Option<TraceId>,
        start_time: SystemTime,
        args: Vec<String>,
    ) -> Self {
        Self {
            write_to_path: None,
            command_name: None,
            cli_args: args,
            representative_config_flags: Vec::new(),
            isolation_dir: None,
            start_time,
            build_count_manager: None,
            trace_id,
            command_end: None,
            command_duration: None,
            re_session_id: None,
            re_experiment_name: None,
            persistent_cache_mode: None,
            critical_path_duration: None,
            critical_path_page_in: None,
            tags: vec![],
            run_local_count: 0,
            dep_file_db_probes: 0,
            dep_file_db_probe_duration_us: 0,
            dep_file_db_fetches: 0,
            dep_file_db_fetch_duration_us: 0,
            dep_file_db_hits: 0,
            dep_file_db_writes_queued: 0,
            run_remote_count: 0,
            run_action_cache_count: 0,
            run_remote_dep_file_cache_count: 0,
            run_remote_served_from_cache_count: 0,
            run_remote_deduplicated_count: 0,
            run_skipped_count: 0,
            run_fallback_count: 0,
            run_fallback_re_queue_count: 0,
            run_local_only_count: 0,
            local_actions_executed_via_worker: 0,
            first_snapshot: None,
            last_snapshot: None,
            min_attempted_build_count_since_rebase: 0,
            min_build_count_since_rebase: 0,
            cache_upload_count: 0,
            cache_upload_attempt_count: 0,
            re_action_cache_query_error_count: 0,
            dep_file_upload_count: 0,
            dep_file_upload_attempt_count: 0,
            parsed_target_patterns: None,
            filesystem: None,
            watchman_version: None,
            eden_version: None,
            test_info: None,
            eligible_for_full_hybrid: false,
            max_event_client_delay: None,
            max_malloc_bytes_active: None,
            max_malloc_bytes_allocated: None,
            run_command_failure_count: 0,
            event_count: 0,
            time_to_first_action_execution: None,
            materialization_output_size: 0,
            initial_materializer_entries_from_sqlite: None,
            time_to_command_start: None,
            time_to_command_critical_section: None,
            time_to_first_analysis: None,
            time_to_load_first_build_file: None,
            time_to_first_command_execution_start: None,
            time_to_first_test_discovery: None,
            time_to_first_test_run: None,
            time_to_first_pass_test_result: None,
            time_to_first_fail_test_result: None,
            time_to_first_fatal_test_result: None,
            time_to_first_timeout_test_result: None,
            time_to_first_skip_test_result: None,
            time_to_first_infra_failure_test_result: None,
            time_to_first_unknown_test_result: None,
            system_info: SystemInfo::default(),
            paging_summary: None,
            file_watcher_stats: None,
            file_watcher_duration: None,
            time_to_last_action_execution_end: None,
            initial_sink_success_count: None,
            initial_sink_failure_count: None,
            initial_sink_dropped_count: None,
            initial_sink_bytes_written: None,
            sink_max_buffer_depth: 0,
            soft_error_categories: BuckMutSet::default(),
            concurrent_command_blocking_duration: None,
            // Use a null daemon_id here initially - if we later get metadata back from the daemon,
            // we'll overwrite this then
            metadata: buck2_events::metadata::collect(&DaemonId::null()),
            analysis_count: 0,
            load_count: 0,
            daemon_in_memory_state_is_corrupted: false,
            daemon_materializer_state_is_corrupted: false,
            enable_restarter: false,
            restarted_trace_id,
            preemptible: None,
            has_command_result: false,
            has_end_of_stream: false,
            compressed_event_log_size_bytes: None,
            remote_sink_config: RemoteEventConfig {
                buffer_size: 1,
                retry_backoff: Duration::from_millis(500),
                retry_attempts: 5,
                #[cfg(not(fbcode_build))]
                retry_window: Duration::from_secs(
                    buck2_events::sink::remote::DEFAULT_RETRY_WINDOW_SECS,
                ),
                #[cfg(not(fbcode_build))]
                replay_spill_dir: None,
                #[cfg(not(fbcode_build))]
                replay_spill_max_bytes: buck2_events::sink::remote::DEFAULT_REPLAY_SPILL_MAX_BYTES,
                message_batch_size: None,
                #[cfg(fbcode_build)]
                thrift_timeout: Duration::from_secs(2),
                #[cfg(not(fbcode_build))]
                grpc_timeout: Duration::from_secs(10),
                #[cfg(not(fbcode_build))]
                bes_backend: None,
                #[cfg(not(fbcode_build))]
                bes_headers: Vec::new(),
                bes_tls: Default::default(),
                #[cfg(not(fbcode_build))]
                build_metadata: Vec::new(),
                #[cfg(not(fbcode_build))]
                event_format: Default::default(),
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload: true,
                #[cfg(not(fbcode_build))]
                upload_successful_action_events: true,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_backend: None,
                #[cfg(not(fbcode_build))]
                re_client_cas_address: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_instance_name: None,
                #[cfg(not(fbcode_build))]
                re_client_instance_name: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_uri_authority: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_max_bytes: 10 * 1024 * 1024,
                #[cfg(not(fbcode_build))]
                channel: Default::default(),
            },
            critical_path_backend: None,
            bxl_ensure_artifacts_duration: None,
            install_duration: None,
            install_device_metadata: Vec::new(),
            command_start_timestamp: None,
            build_phase_end: None,
            installer_log_url: None,
            installer_name: None,
            initial_re_upload_bytes: None,
            initial_re_download_bytes: None,
            initial_zdb_download_queries: None,
            initial_zdb_download_bytes: None,
            initial_zdb_upload_queries: None,
            initial_zdb_upload_bytes: None,
            initial_zgateway_download_queries: None,
            initial_zgateway_download_bytes: None,
            initial_zgateway_upload_queries: None,
            initial_zgateway_upload_bytes: None,
            initial_manifold_download_queries: None,
            initial_manifold_download_bytes: None,
            initial_manifold_upload_queries: None,
            initial_manifold_upload_bytes: None,
            initial_hedwig_download_queries: None,
            initial_hedwig_download_bytes: None,
            initial_hedwig_upload_queries: None,
            initial_hedwig_upload_bytes: None,
            concurrent_command_ids: BuckMutSet::default(),
            re_exceeded_quotas: BuckMutSet::default(),
            daemon_connection_failure: false,
            daemon_was_started: None,
            settings_hash: None,
            should_restart: false,
            client_metadata: Vec::new(),
            agent_invocation_info: AgentInvocationInfo::default(),
            command_errors: Vec::new(),
            exit_code: None,
            exit_result_name: None,
            outcome: None,
            server_stderr: String::new(),
            target_rule_type_names: Vec::new(),
            re_max_download_speeds: vec![
                SlidingWindow::new(Duration::from_secs(1)),
                SlidingWindow::new(Duration::from_secs(5)),
                SlidingWindow::new(Duration::from_secs(10)),
            ],
            re_max_upload_speeds: vec![
                SlidingWindow::new(Duration::from_secs(1)),
                SlidingWindow::new(Duration::from_secs(5)),
                SlidingWindow::new(Duration::from_secs(10)),
            ],
            re_avg_download_speed: NetworkSpeedAverage::default(),
            re_avg_upload_speed: NetworkSpeedAverage::default(),
            peak_process_memory_bytes: None,
            has_new_buckconfigs: false,
            peak_used_disk_space_bytes: None,
            peak_normalized_system_load1: None,
            peak_normalized_system_load5: None,
            active_networks_kinds: BuckMutSet::default(),
            target_cfg: None,
            hg_revision: None,
            git_revision: None,
            has_local_changes: None,
            version_control_errors: Vec::new(),
            concurrent_commands: false,
            initial_local_cache_hits_files: None,
            initial_local_cache_hits_bytes: None,
            initial_local_cache_misses_files: None,
            initial_local_cache_misses_bytes: None,
            materialization_files: 0,
            previous_uuid_with_mismatched_config: None,
            file_watcher: None,
            health_check_tags_receiver: None,
            health_check_tags: BuckMutSet::default(),
            exec_time_ms: 0,
            initial_local_cache_hits_files_from_memory_cache: None,
            initial_local_cache_hits_files_from_filesystem_cache: None,
            initial_local_cache_lookups: None,
            initial_local_cache_lookup_latency_microseconds: None,
            max_dice_in_progress_keys: 0,
            max_dice_compute_keys: 0,
            current_in_progress_actions: 0,
            max_in_progress_actions: 0,
            action_intervals: Vec::new(),
            current_in_progress_local_actions: 0,
            max_in_progress_local_actions: 0,
            current_in_progress_remote_actions: 0,
            max_in_progress_remote_actions: 0,
            current_in_progress_remote_uploads: 0,
            max_in_progress_remote_uploads: 0,
            executor_stages_by_span: BuckMutMap::default(),
            memory_max_anon_allprocs: None,
            memory_max_anon_forkserver_actions: None,
            memory_max_total_allprocs: None,
            memory_max_total_forkserver_actions: None,
            memory_max_swap_bytes_allprocs: None,
            memory_max_pressure_10s_avg_allprocs: None,
            memory_max_pressure_60s_avg_allprocs: None,
            command_options: None,
            initial_io_copy_count: None,
            initial_io_symlink_count: None,
            initial_io_hardlink_count: None,
            initial_io_mkdir_count: None,
            initial_io_readdir_count: None,
            initial_io_readdir_eden_count: None,
            initial_io_rmdir_count: None,
            initial_io_rmdir_all_count: None,
            initial_io_stat_count: None,
            initial_io_stat_eden_count: None,
            initial_io_chmod_count: None,
            initial_io_readlink_count: None,
            initial_io_remove_count: None,
            initial_io_rename_count: None,
            initial_io_read_count: None,
            initial_io_write_count: None,
            initial_io_canonicalize_count: None,
            initial_io_eden_settle_count: None,
            repo_path: None,
        }
    }

    pub fn update_for_client_ctx(
        &mut self,
        ctx: &ClientCommandContext<'_>,
        command_name: &'static str,
    ) {
        self.isolation_dir = Some(ctx.isolation.to_string());
        self.client_metadata = ctx
            .client_metadata
            .iter()
            .map(ClientMetadata::to_proto)
            .collect();

        if let Some(client_id_from_client_metadata) = ctx
            .client_metadata
            .iter()
            .find(|m| m.key == "id")
            .map(|m| m.value.clone())
        {
            self.metadata.insert(
                "client".to_owned(),
                client_id_from_client_metadata.to_owned(),
            );
        }

        self.agent_invocation_info = ctx.agent_invocation_info().clone();

        // Outside a repository there are no settings to resolve; a settings
        // file that fails to parse fails the command itself later.
        self.settings_hash = ctx
            .buck_settings()
            .ok()
            .map(|settings| settings.snapshot().hash);

        self.command_name = Some(command_name);
    }

    pub(crate) fn update_for_command(
        &mut self,
        ctx: &ClientCommandContext<'_>,
        event_log_opts: &CommonEventLogOptions,
        sanitized_argv: Vec<String>,
        build_config_opts: Option<&CommonBuildConfigurationOptions>,
        representative_config_flags: Vec<String>,
        log_size_counter_bytes: Option<Arc<AtomicU64>>,
        health_check_tags_receiver: Option<Receiver<Vec<String>>>,
        paths: Option<&InvocationPaths>,
    ) {
        let write_to_path = event_log_opts
            .unstable_write_invocation_record
            .as_ref()
            .map(|path| path.resolve(&ctx.working_dir));

        let filesystem;
        #[cfg(fbcode_build)]
        {
            let is_eden = paths.is_some_and(|paths| {
                let root = std::path::Path::to_owned(paths.project_root().root().to_buf().as_ref());
                detect_eden::is_eden(root).unwrap_or(false)
            });
            if is_eden {
                filesystem = "eden".to_owned();
            } else {
                filesystem = "default".to_owned();
            }
        }
        #[cfg(not(fbcode_build))]
        {
            filesystem = "default".to_owned();
        }
        let build_count = paths.and_then(|p| match BuildCountManager::new(p.build_count_dir()) {
            Ok(manager) => Some(manager),
            Err(e) => {
                soft_error_or_panic!("build_count_init_failed", e);
                None
            }
        });

        self.cli_args = sanitized_argv;
        self.representative_config_flags = representative_config_flags;
        self.write_to_path = write_to_path;
        self.build_count_manager = build_count;
        self.filesystem = Some(filesystem);
        self.compressed_event_log_size_bytes = log_size_counter_bytes;
        self.remote_sink_config = remote_sink_config::with_buckconfig_overrides(
            paths,
            RemoteEventConfig {
                buffer_size: 1,
                retry_backoff: Duration::from_millis(500),
                retry_attempts: 5,
                #[cfg(not(fbcode_build))]
                retry_window: Duration::from_secs(
                    buck2_events::sink::remote::DEFAULT_RETRY_WINDOW_SECS,
                ),
                #[cfg(not(fbcode_build))]
                replay_spill_dir: None,
                #[cfg(not(fbcode_build))]
                replay_spill_max_bytes: buck2_events::sink::remote::DEFAULT_REPLAY_SPILL_MAX_BYTES,
                message_batch_size: None,
                #[cfg(fbcode_build)]
                thrift_timeout: Duration::from_secs(2),
                #[cfg(not(fbcode_build))]
                grpc_timeout: Duration::from_secs(10),
                #[cfg(not(fbcode_build))]
                bes_backend: None,
                #[cfg(not(fbcode_build))]
                bes_headers: Vec::new(),
                bes_tls: Default::default(),
                #[cfg(not(fbcode_build))]
                build_metadata: Vec::new(),
                #[cfg(not(fbcode_build))]
                event_format: Default::default(),
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload: true,
                #[cfg(not(fbcode_build))]
                upload_successful_action_events: true,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_backend: None,
                #[cfg(not(fbcode_build))]
                re_client_cas_address: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_instance_name: None,
                #[cfg(not(fbcode_build))]
                re_client_instance_name: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_uri_authority: None,
                #[cfg(not(fbcode_build))]
                bazel_artifact_upload_max_bytes: 10 * 1024 * 1024,
                #[cfg(not(fbcode_build))]
                channel: Default::default(),
            },
        );
        self.health_check_tags_receiver = health_check_tags_receiver;
        self.preemptible = build_config_opts.and_then(|opts| opts.preemptible);
        self.repo_path = paths.map(|p| p.project_root().root().to_string());
    }

    async fn build_count(
        &mut self,
        is_success: bool,
        command_name: &str,
    ) -> buck2_error::Result<Option<BuildCount>> {
        if let Some(stats) = &self.file_watcher_stats {
            if let Some(merge_base) = &stats.branched_from_revision {
                match &self.parsed_target_patterns {
                    None => {
                        if is_success {
                            return Err(buck2_error!(
                                ErrorTag::InvalidEvent,
                                "successful {} commands should have resolved target patterns",
                                command_name
                            ));
                        }
                        // fallthrough to 0 below
                    }
                    Some(v) => {
                        return if let Some(build_count) = &self.build_count_manager {
                            Some(
                                build_count
                                    .increment(merge_base, v, is_success)
                                    .await
                                    .buck_error_context("Error recording build count"),
                            )
                            .transpose()
                        } else {
                            Ok(None)
                        };
                    }
                };
            }
        }

        Ok(Default::default())
    }

    fn outcome(&self, exit_result: &ExitResult) -> InvocationOutcome {
        let has_errors = !exit_result.get_all_errors().is_empty();

        // Could be replaced with an error tag check.
        let crashed =
            self.daemon_connection_failure || (self.has_end_of_stream && !self.has_command_result);

        match (exit_result.exit_code(), has_errors) {
            // Only report success if no errors are reported.
            (Some(ExitCode::Success), false) => InvocationOutcome::Success,
            // Should not have returned success.
            (Some(ExitCode::Success), true) => InvocationOutcome::Unknown,
            // Ignore errors if the command was cancelled.
            (Some(ExitCode::SignalInterrupt) | Some(ExitCode::ClientIoBrokenPipe), _) => {
                InvocationOutcome::Cancelled
            }
            // Remaining exit codes indicate failed commands, these should always have errors.
            (Some(_), true) => match crashed {
                true => InvocationOutcome::Crashed,
                false => InvocationOutcome::Failed,
            },
            // Error should have been reported.
            (Some(_), false) => InvocationOutcome::Unknown,
            // No exit code means a run command succeeded in calling exec (result of exec is unknown but buck succeeded).
            // This should probably be a separate outcome.
            (None, false) => InvocationOutcome::Success,
            // Exec should not have been called if there were errors in buck.
            (None, true) => InvocationOutcome::Unknown,
        }
    }

    fn finalize_errors(&mut self) -> Vec<ProcessedErrorReport> {
        let mut errors = Vec::new();
        let disk_near_full = match (
            self.last_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.used_disk_space_bytes),
            self.system_info.total_disk_space_bytes,
        ) {
            (Some(used), Some(total)) => is_disk_near_full(total.saturating_sub(used), total),
            _ => false,
        };
        for error in self.command_errors.drain(..) {
            // FIXME this error should be updated at the source if possible, and not only in the invocation record.
            let error: ErrorReport = if error.tags.contains(&(ErrorTag::ClientGrpc as i32)) {
                let error: buck2_error::Error = error.into();
                // Add stderr to GRPC connection errors if available
                let error = classify_server_stderr(error, &self.server_stderr, disk_near_full);
                let error = if self.server_stderr.is_empty() {
                    let error = error.context("buckd stderr is empty");
                    // Likely buckd received SIGKILL, may be due to memory pressure
                    if self.tags.iter().any(|s| s == MEMORY_PRESSURE_TAG) {
                        error
                            .context("memory pressure detected")
                            .tag([ErrorTag::ServerMemoryPressure])
                    } else {
                        error
                    }
                } else if error.has_tag(ErrorTag::ServerSigterm) {
                    error.context("buckd killed by SIGTERM")
                } else {
                    // Scribe sink truncates messages, but here we can do it better:
                    // - truncate even if total message is not large enough
                    // - truncate stderr, but keep the error message
                    let server_stderr = truncate_stderr(&self.server_stderr);
                    error.context(format!("buckd stderr:\n{server_stderr}"))
                };
                (&error).into()
            } else {
                error
            };
            errors.push(error);
        }
        errors.sort_by_key(|e| e.error_rank());
        errors
            .into_iter()
            .map(process_error_report)
            .collect::<Vec<_>>()
    }

    fn create_record_event(&mut self) -> BuckEvent {
        self.soft_error_categories
            .extend(std::mem::take(&mut *CLIENT_SOFT_ERRORS.lock().unwrap()));
        let mut sink_success_count = None;
        let mut sink_failure_count = None;
        let mut sink_dropped_count = None;
        let mut sink_bytes_written = None;
        let mut re_upload_bytes = None;
        let mut re_download_bytes = None;

        let mut zdb_download_queries = None;
        let mut zdb_download_bytes = None;
        let mut zdb_upload_queries = None;
        let mut zdb_upload_bytes = None;

        let mut zgateway_download_queries = None;
        let mut zgateway_download_bytes = None;
        let mut zgateway_upload_queries = None;
        let mut zgateway_upload_bytes = None;

        let mut manifold_download_queries = None;
        let mut manifold_download_bytes = None;
        let mut manifold_upload_queries = None;
        let mut manifold_upload_bytes = None;

        let mut hedwig_download_queries = None;
        let mut hedwig_download_bytes = None;
        let mut hedwig_upload_queries = None;
        let mut hedwig_upload_bytes = None;

        let mut local_cache_hits_files = None;
        let mut local_cache_hits_bytes = None;
        let mut local_cache_misses_files = None;
        let mut local_cache_misses_bytes = None;

        let mut local_cache_hits_files_from_memory_cache = None;
        let mut local_cache_hits_files_from_filesystem_cache = None;
        let mut local_cache_lookups = None;
        let mut local_cache_lookup_latency_microseconds = None;

        let mut io_copy_count = None;
        let mut io_symlink_count = None;
        let mut io_hardlink_count = None;
        let mut io_mkdir_count = None;
        let mut io_readdir_count = None;
        let mut io_readdir_eden_count = None;
        let mut io_rmdir_count = None;
        let mut io_rmdir_all_count = None;
        let mut io_stat_count = None;
        let mut io_stat_eden_count = None;
        let mut io_chmod_count = None;
        let mut io_readlink_count = None;
        let mut io_remove_count = None;
        let mut io_rename_count = None;
        let mut io_read_count = None;
        let mut io_write_count = None;
        let mut io_canonicalize_count = None;
        let mut io_eden_settle_count = None;

        let mut page_in_count = None;
        let mut page_in_fetch_us = None;
        let mut page_in_deser_us = None;
        let mut page_in_bytes = None;
        let mut page_in_by_key_type = IntentionallyStdHashMap::new();

        // Already a per-command delta from the daemon; sum across key types for
        // the aggregate scalars.
        if let Some(paging_summary) = &self.paging_summary
            && !paging_summary.dice_page_in_by_key_type.is_empty()
        {
            page_in_by_key_type = paging_summary.dice_page_in_by_key_type.clone();
            page_in_count = Some(page_in_by_key_type.values().map(|s| s.count).sum());
            page_in_fetch_us = Some(page_in_by_key_type.values().map(|s| s.fetch_us).sum());
            page_in_deser_us = Some(page_in_by_key_type.values().map(|s| s.deser_us).sum());
            page_in_bytes = Some(page_in_by_key_type.values().map(|s| s.bytes).sum());
        }

        if let Some(snapshot) = &self.last_snapshot {
            sink_success_count =
                calculate_diff_if_some(&snapshot.sink_successes, &self.initial_sink_success_count);
            sink_failure_count =
                calculate_diff_if_some(&snapshot.sink_failures, &self.initial_sink_failure_count);
            sink_dropped_count =
                calculate_diff_if_some(&snapshot.sink_dropped, &self.initial_sink_dropped_count);
            sink_bytes_written = calculate_diff_if_some(
                &snapshot.sink_bytes_written,
                &self.initial_sink_bytes_written,
            );
            re_upload_bytes = calculate_diff_if_some(
                &Some(snapshot.re_upload_bytes),
                &self.initial_re_upload_bytes,
            );
            re_download_bytes = calculate_diff_if_some(
                &Some(snapshot.re_download_bytes),
                &self.initial_re_download_bytes,
            );
            zdb_download_queries = calculate_diff_if_some(
                &Some(snapshot.zdb_download_queries),
                &self.initial_zdb_download_queries,
            );
            zdb_download_bytes = calculate_diff_if_some(
                &Some(snapshot.zdb_download_bytes),
                &self.initial_zdb_download_bytes,
            );
            zdb_upload_queries = calculate_diff_if_some(
                &Some(snapshot.zdb_upload_queries),
                &self.initial_zdb_upload_queries,
            );
            zdb_upload_bytes = calculate_diff_if_some(
                &Some(snapshot.zdb_upload_bytes),
                &self.initial_zdb_upload_bytes,
            );
            zgateway_download_queries = calculate_diff_if_some(
                &Some(snapshot.zgateway_download_queries),
                &self.initial_zgateway_download_queries,
            );
            zgateway_download_bytes = calculate_diff_if_some(
                &Some(snapshot.zgateway_download_bytes),
                &self.initial_zgateway_download_bytes,
            );
            zgateway_upload_queries = calculate_diff_if_some(
                &Some(snapshot.zgateway_upload_queries),
                &self.initial_zgateway_upload_queries,
            );
            zgateway_upload_bytes = calculate_diff_if_some(
                &Some(snapshot.zgateway_upload_bytes),
                &self.initial_zgateway_upload_bytes,
            );
            manifold_download_queries = calculate_diff_if_some(
                &Some(snapshot.manifold_download_queries),
                &self.initial_manifold_download_queries,
            );
            manifold_download_bytes = calculate_diff_if_some(
                &Some(snapshot.manifold_download_bytes),
                &self.initial_manifold_download_bytes,
            );
            manifold_upload_queries = calculate_diff_if_some(
                &Some(snapshot.manifold_upload_queries),
                &self.initial_manifold_upload_queries,
            );
            manifold_upload_bytes = calculate_diff_if_some(
                &Some(snapshot.manifold_upload_bytes),
                &self.initial_manifold_upload_bytes,
            );
            hedwig_download_queries = calculate_diff_if_some(
                &Some(snapshot.hedwig_download_queries),
                &self.initial_hedwig_download_queries,
            );
            hedwig_download_bytes = calculate_diff_if_some(
                &Some(snapshot.hedwig_download_bytes),
                &self.initial_hedwig_download_bytes,
            );
            hedwig_upload_queries = calculate_diff_if_some(
                &Some(snapshot.hedwig_upload_queries),
                &self.initial_hedwig_upload_queries,
            );
            hedwig_upload_bytes = calculate_diff_if_some(
                &Some(snapshot.hedwig_upload_bytes),
                &self.initial_hedwig_upload_bytes,
            );

            local_cache_hits_files = calculate_diff_if_some(
                &Some(snapshot.local_cache_hits_files),
                &self.initial_local_cache_hits_files,
            );

            local_cache_hits_bytes = calculate_diff_if_some(
                &Some(snapshot.local_cache_hits_bytes),
                &self.initial_local_cache_hits_bytes,
            );

            local_cache_misses_files = calculate_diff_if_some(
                &Some(snapshot.local_cache_misses_files),
                &self.initial_local_cache_misses_files,
            );

            local_cache_misses_bytes = calculate_diff_if_some(
                &Some(snapshot.local_cache_misses_bytes),
                &self.initial_local_cache_misses_bytes,
            );

            local_cache_hits_files_from_memory_cache = calculate_diff_if_some(
                &Some(snapshot.local_cache_hits_files_from_memory_cache),
                &self.initial_local_cache_hits_files_from_memory_cache,
            );

            local_cache_hits_files_from_filesystem_cache = calculate_diff_if_some(
                &Some(snapshot.local_cache_hits_files_from_filesystem_cache),
                &self.initial_local_cache_hits_files_from_filesystem_cache,
            );

            local_cache_lookups = calculate_diff_if_some(
                &Some(snapshot.local_cache_lookups),
                &self.initial_local_cache_lookups,
            );

            local_cache_lookup_latency_microseconds = calculate_diff_if_some(
                &Some(snapshot.local_cache_lookup_latency_microseconds),
                &self.initial_local_cache_lookup_latency_microseconds,
            );

            io_copy_count =
                calculate_diff_if_some(&snapshot.io_copy_count, &self.initial_io_copy_count);
            io_symlink_count =
                calculate_diff_if_some(&snapshot.io_symlink_count, &self.initial_io_symlink_count);
            io_hardlink_count = calculate_diff_if_some(
                &snapshot.io_hardlink_count,
                &self.initial_io_hardlink_count,
            );
            io_mkdir_count =
                calculate_diff_if_some(&snapshot.io_mkdir_count, &self.initial_io_mkdir_count);
            io_readdir_count =
                calculate_diff_if_some(&snapshot.io_readdir_count, &self.initial_io_readdir_count);
            io_readdir_eden_count = calculate_diff_if_some(
                &snapshot.io_readdir_eden_count,
                &self.initial_io_readdir_eden_count,
            );
            io_rmdir_count =
                calculate_diff_if_some(&snapshot.io_rmdir_count, &self.initial_io_rmdir_count);
            io_rmdir_all_count = calculate_diff_if_some(
                &snapshot.io_rmdir_all_count,
                &self.initial_io_rmdir_all_count,
            );
            io_stat_count =
                calculate_diff_if_some(&snapshot.io_stat_count, &self.initial_io_stat_count);
            io_stat_eden_count = calculate_diff_if_some(
                &snapshot.io_stat_eden_count,
                &self.initial_io_stat_eden_count,
            );
            io_chmod_count =
                calculate_diff_if_some(&snapshot.io_chmod_count, &self.initial_io_chmod_count);
            io_readlink_count = calculate_diff_if_some(
                &snapshot.io_readlink_count,
                &self.initial_io_readlink_count,
            );
            io_remove_count =
                calculate_diff_if_some(&snapshot.io_remove_count, &self.initial_io_remove_count);
            io_rename_count =
                calculate_diff_if_some(&snapshot.io_rename_count, &self.initial_io_rename_count);
            io_read_count =
                calculate_diff_if_some(&snapshot.io_read_count, &self.initial_io_read_count);
            io_write_count =
                calculate_diff_if_some(&snapshot.io_write_count, &self.initial_io_write_count);
            io_canonicalize_count = calculate_diff_if_some(
                &snapshot.io_canonicalize_count,
                &self.initial_io_canonicalize_count,
            );
            io_eden_settle_count = calculate_diff_if_some(
                &snapshot.io_eden_settle_count,
                &self.initial_io_eden_settle_count,
            );

            // We show memory/disk warnings in the console but we can't emit a tag event there due to having no access to dispatcher.
            // Also, it suffices to only emit a single tag per invocation, not one tag each time memory pressure is exceeded.
            // We can't just rely on the last snapshot here instead we use the peak memory/disk usage to check if we ever reported a warning.
            if let Some(mem) = self.peak_process_memory_bytes
                && check_memory_pressure(mem, &self.system_info).is_some()
            {
                self.tags.push(MEMORY_PRESSURE_TAG.to_owned());
            }
            if let Some(bytes) = self.peak_used_disk_space_bytes
                && check_remaining_disk_space(bytes, &self.system_info).is_some()
            {
                self.tags.push("low_disk_space".to_owned());
            }
            if check_download_speed(
                &self.first_snapshot,
                self.last_snapshot.as_ref(),
                &self.system_info,
                self.re_avg_download_speed.avg_per_second(),
                self.concurrent_commands,
            ) {
                self.tags.push("slow_network_speed_ui_only".to_owned());
            }
            self.try_read_health_check_tags(); // Empty the queue so far.
            self.tags.extend(self.health_check_tags.iter().cloned());
        }

        let mut metadata = Self::default_metadata();
        metadata.strings.extend(std::mem::take(&mut self.metadata));

        let preemptible = self.preemptible.take().map_or("UNSPECIFIED", |p| match p {
            PreemptibleWhen::Never => "NEVER",
            PreemptibleWhen::Always => "ALWAYS",
            PreemptibleWhen::OnDifferentState => "ON_DIFFERENT_STATE",
        });

        let errors = self.finalize_errors();

        let action_parallelism = buck2_action_parallelism::compute(
            &std::mem::take(&mut self.action_intervals),
            &buck2_action_parallelism::PERCENTILES,
        );

        let agent_invocation_info = std::mem::take(&mut self.agent_invocation_info);
        let agent_context = agent_invocation_info.agent_context().to_proto();
        let record = buck2_data::InvocationRecord {
            command_name: Some(self.command_name.unwrap_or("unknown").to_owned()),
            command_end: self.command_end.take(),
            command_duration: self.command_duration.take(),
            client_walltime: duration_since(SystemTime::now(), self.start_time)
                .try_into()
                .ok(),
            wrapper_start_time: buck2_env!(BUCK_WRAPPER_START_TIME_ENV_VAR, type=u64)
                .ok()
                .flatten()
                .or_else(|| {
                    self.start_time
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .ok()
                        .and_then(duration_as_millis)
                }),
            re_session_id: self.re_session_id.take().unwrap_or_default(),
            re_experiment_name: self.re_experiment_name.take().unwrap_or_default(),
            persistent_cache_mode: self.persistent_cache_mode.clone(),
            cli_args: self.cli_args.clone(),
            representative_config_flags: self.representative_config_flags.clone(),
            critical_path_duration: self.critical_path_duration.and_then(|x| x.try_into().ok()),
            critical_path_page_in: self.critical_path_page_in.and_then(|x| x.try_into().ok()),
            metadata: Some(metadata),
            tags: std::mem::take(&mut self.tags),
            run_local_count: self.run_local_count,
            run_remote_count: self.run_remote_count,
            run_action_cache_count: self.run_action_cache_count,
            run_remote_dep_file_cache_count: self.run_remote_dep_file_cache_count,
            run_remote_served_from_cache_count: self.run_remote_served_from_cache_count,
            run_remote_deduplicated_count: self.run_remote_deduplicated_count,
            cache_hit_rate: total_cache_hit_rate(
                self.run_local_count,
                self.run_remote_count,
                self.run_action_cache_count + self.run_remote_served_from_cache_count,
                self.run_remote_dep_file_cache_count,
            ) as f32,
            run_skipped_count: self.run_skipped_count,
            run_fallback_count: Some(self.run_fallback_count),
            run_fallback_re_queue_count: Some(self.run_fallback_re_queue_count),
            run_local_only_count: Some(self.run_local_only_count),
            local_actions_executed_via_worker: Some(self.local_actions_executed_via_worker),
            first_snapshot: self.first_snapshot.take(),
            last_snapshot: self.last_snapshot.take(),
            min_attempted_build_count_since_rebase: self.min_attempted_build_count_since_rebase,
            min_build_count_since_rebase: self.min_build_count_since_rebase,
            cache_upload_count: self.cache_upload_count,
            cache_upload_attempt_count: self.cache_upload_attempt_count,
            re_action_cache_query_error_count: Some(self.re_action_cache_query_error_count),
            dep_file_upload_count: self.dep_file_upload_count,
            dep_file_upload_attempt_count: self.dep_file_upload_attempt_count,
            parsed_target_patterns: self.parsed_target_patterns.take(),
            filesystem: self.filesystem.take().unwrap_or("default".to_owned()),
            watchman_version: self.watchman_version.take(),
            eden_version: self.eden_version.take(),
            test_info: self.test_info.take(),
            eligible_for_full_hybrid: Some(self.eligible_for_full_hybrid),
            max_event_client_delay_ms: self.max_event_client_delay.and_then(duration_as_millis),
            max_malloc_bytes_active: self.max_malloc_bytes_active.take(),
            max_malloc_bytes_allocated: self.max_malloc_bytes_allocated.take(),
            run_command_failure_count: Some(self.run_command_failure_count),
            event_count: Some(self.event_count),
            time_to_first_action_execution_ms: self
                .time_to_first_action_execution
                .and_then(duration_as_millis),
            materialization_output_size: Some(self.materialization_output_size),
            initial_materializer_entries_from_sqlite: self.initial_materializer_entries_from_sqlite,
            time_to_command_start_ms: self.time_to_command_start.and_then(duration_as_millis),
            time_to_command_critical_section_ms: self
                .time_to_command_critical_section
                .and_then(duration_as_millis),
            time_to_first_analysis_ms: self.time_to_first_analysis.and_then(duration_as_millis),
            time_to_load_first_build_file_ms: self
                .time_to_load_first_build_file
                .and_then(duration_as_millis),
            time_to_first_command_execution_start_ms: self
                .time_to_first_command_execution_start
                .and_then(duration_as_millis),
            time_to_first_test_discovery_ms: self
                .time_to_first_test_discovery
                .and_then(duration_as_millis),
            time_to_first_test_run_ms: self.time_to_first_test_run.and_then(duration_as_millis),
            time_to_first_pass_test_result_ms: self
                .time_to_first_pass_test_result
                .and_then(duration_as_millis),
            time_to_first_fail_test_result_ms: self
                .time_to_first_fail_test_result
                .and_then(duration_as_millis),
            time_to_first_fatal_test_result_ms: self
                .time_to_first_fatal_test_result
                .and_then(duration_as_millis),
            time_to_first_skip_test_result_ms: self
                .time_to_first_skip_test_result
                .and_then(duration_as_millis),
            time_to_first_timeout_test_result_ms: self
                .time_to_first_timeout_test_result
                .and_then(duration_as_millis),
            time_to_first_infra_failure_test_result_ms: self
                .time_to_first_infra_failure_test_result
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
            time_to_first_unknown_test_result_ms: self
                .time_to_first_unknown_test_result
                .and_then(duration_as_millis),
            system_total_memory_bytes: self.system_info.system_total_memory_bytes,
            container_memory_limit_bytes: self.system_info.container_memory_limit_bytes,
            file_watcher_stats: self.file_watcher_stats.take(),
            file_watcher_duration_ms: self.file_watcher_duration.and_then(duration_as_millis),
            time_to_last_action_execution_end_ms: self
                .time_to_last_action_execution_end
                .and_then(duration_as_millis),
            isolation_dir: self.isolation_dir.take(),
            dep_file_db_probes: Some(self.dep_file_db_probes),
            dep_file_db_probe_duration_us: Some(self.dep_file_db_probe_duration_us),
            dep_file_db_fetches: Some(self.dep_file_db_fetches),
            dep_file_db_fetch_duration_us: Some(self.dep_file_db_fetch_duration_us),
            dep_file_db_hits: Some(self.dep_file_db_hits),
            dep_file_db_writes_queued: Some(self.dep_file_db_writes_queued),
            sink_success_count,
            sink_failure_count,
            sink_dropped_count,
            sink_bytes_written,
            sink_max_buffer_depth: Some(self.sink_max_buffer_depth),
            soft_error_categories: std::mem::take(&mut self.soft_error_categories)
                .into_iter()
                .collect(),
            concurrent_command_blocking_duration: self
                .concurrent_command_blocking_duration
                .and_then(|x| x.try_into().ok()),
            analysis_count: Some(self.analysis_count),
            load_count: Some(self.load_count),
            restarted_trace_id: self.restarted_trace_id.as_ref().map(|t| t.to_string()),
            has_command_result: Some(self.has_command_result),
            has_end_of_stream: Some(self.has_end_of_stream),
            // At this point we expect the event log writer to have finished
            compressed_event_log_size_bytes: Some(
                self.compressed_event_log_size_bytes
                    .as_ref()
                    .map(|x| x.load(Ordering::Relaxed))
                    .unwrap_or_default(),
            ),
            critical_path_backend: self.critical_path_backend.take(),
            instant_command_is_success: None,
            bxl_ensure_artifacts_duration: self.bxl_ensure_artifacts_duration.take(),
            re_upload_bytes,
            re_download_bytes,
            concurrent_command_ids: std::mem::take(&mut self.concurrent_command_ids)
                .into_iter()
                .collect(),
            re_exceeded_quotas: std::mem::take(&mut self.re_exceeded_quotas)
                .into_iter()
                .collect(),
            page_out_started: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.page_out_started),
            daemon_connection_failure: Some(self.daemon_connection_failure),
            daemon_was_started: self.daemon_was_started.map(|t| t as i32),
            settings_hash: self.settings_hash.take(),
            should_restart: Some(self.should_restart),
            client_metadata: std::mem::take(&mut self.client_metadata),
            agent_direct_call: agent_context
                .iter()
                .rev()
                .find(|entry| entry.key == buck2_data::AgentContextEntry::KEY_DIRECT_CALL)
                .and_then(|entry| entry.value.parse::<bool>().ok()),
            agent_inferred_direct_call: agent_invocation_info.inferred_direct_call(),
            agent_callers: agent_invocation_info.inference().callers.clone(),
            agent_detection_stop_reason: agent_invocation_info
                .inference()
                .stop_reason
                .map(i32::from),
            agent_context,
            errors,
            target_rule_type_names: unique_and_sorted(
                std::mem::take(&mut self.target_rule_type_names).into_iter(),
            ),
            new_configs_used: Some(self.has_new_buckconfigs),
            re_max_download_speed: self
                .re_max_download_speeds
                .iter()
                .map(|w| w.max_per_second().unwrap_or_default())
                .max(),
            re_max_upload_speed: self
                .re_max_upload_speeds
                .iter()
                .map(|w| w.max_per_second().unwrap_or_default())
                .max(),
            re_avg_download_speed: self.re_avg_download_speed.avg_per_second(),
            re_avg_upload_speed: self.re_avg_upload_speed.avg_per_second(),
            install_duration: self.install_duration.take(),
            install_device_metadata: std::mem::take(&mut self.install_device_metadata),
            build_duration_ms: self.build_duration_ms(),
            build_status: self.build_status().map(|s| s.into()),
            installer_log_url: self.installer_log_url.take(),
            installer_name: self.installer_name.take(),
            peak_process_memory_bytes: self.peak_process_memory_bytes.take(),
            event_log_manifold_ttl_s: manifold_event_log_ttl().ok().map(|t| t.as_secs()),
            total_disk_space_bytes: self.system_info.total_disk_space_bytes.take(),
            peak_used_disk_space_bytes: self.peak_used_disk_space_bytes.take(),
            peak_normalized_system_load1: self.peak_normalized_system_load1.take(),
            peak_normalized_system_load5: self.peak_normalized_system_load5.take(),
            zdb_download_queries,
            zdb_download_bytes,
            zdb_upload_queries,
            zdb_upload_bytes,
            zgateway_download_queries,
            zgateway_download_bytes,
            zgateway_upload_queries,
            zgateway_upload_bytes,
            manifold_download_queries,
            manifold_download_bytes,
            manifold_upload_queries,
            manifold_upload_bytes,
            hedwig_download_queries,
            hedwig_download_bytes,
            hedwig_upload_queries,
            hedwig_upload_bytes,
            active_networks_kinds: std::mem::take(&mut self.active_networks_kinds)
                .into_iter()
                .collect(),
            target_cfg: self.target_cfg.take(),
            hg_revision: self.hg_revision.take(),
            git_revision: self.git_revision.take(),
            has_local_changes: self.has_local_changes.take(),
            version_control_errors: std::mem::take(&mut self.version_control_errors),
            version_control_revision: None,
            local_cache_hits_files,
            local_cache_hits_bytes,
            local_cache_misses_files,
            local_cache_misses_bytes,
            materialization_files: Some(self.materialization_files),
            previous_uuid_with_mismatched_config: self.previous_uuid_with_mismatched_config.take(),
            file_watcher: self.file_watcher.take(),
            exec_time_ms: self.exec_time_ms,
            exit_code: self.exit_code.take(),
            exit_result_name: self.exit_result_name.take(),
            outcome: self.outcome.take().map(|out| out.into()),
            preemptible: Some(preemptible.to_owned()),
            local_cache_hits_files_from_memory_cache,
            local_cache_hits_files_from_filesystem_cache,
            local_cache_lookups,
            re_average_local_cache_lookup_microseconds: local_cache_lookups
                .filter(|&c| c != 0)
                .and_then(|c| {
                    local_cache_lookup_latency_microseconds
                        .map(|duration| duration as f64 / c as f64)
                }),
            max_dice_in_progress_keys: Some(self.max_dice_in_progress_keys),
            max_dice_compute_keys: Some(self.max_dice_compute_keys),
            max_in_progress_actions: Some(self.max_in_progress_actions),
            max_in_progress_local_actions: Some(self.max_in_progress_local_actions),
            max_in_progress_remote_actions: Some(self.max_in_progress_remote_actions),
            max_in_progress_remote_uploads: Some(self.max_in_progress_remote_uploads),
            action_concurrency_percentiles: action_parallelism
                .percentiles
                .iter()
                .map(
                    |&(percentile, concurrency)| buck2_data::ActionConcurrencyPercentile {
                        percentile,
                        concurrency,
                    },
                )
                .collect(),
            action_avg_concurrency: Some(action_parallelism.avg_concurrency),
            action_active_duration_ms: Some(
                (action_parallelism.total_active_duration_us.max(0) / 1000) as u64,
            ),
            memory_max_anon_allprocs: self.memory_max_anon_allprocs,
            memory_max_anon_forkserver_actions: self.memory_max_anon_forkserver_actions,
            memory_max_total_allprocs: self.memory_max_total_allprocs,
            memory_max_total_forkserver_actions: self.memory_max_total_forkserver_actions,
            memory_max_swap_bytes_allprocs: self.memory_max_swap_bytes_allprocs.unwrap_or(0),
            memory_max_pressure_10s_avg_allprocs: self
                .memory_max_pressure_10s_avg_allprocs
                .unwrap_or(0.0),
            memory_max_pressure_60s_avg_allprocs: self
                .memory_max_pressure_60s_avg_allprocs
                .unwrap_or(0.0),
            command_options: self.command_options,
            io_copy_count,
            io_symlink_count,
            io_hardlink_count,
            io_mkdir_count,
            io_readdir_count,
            io_readdir_eden_count,
            io_rmdir_count,
            io_rmdir_all_count,
            io_stat_count,
            io_stat_eden_count,
            io_chmod_count,
            io_readlink_count,
            io_remove_count,
            io_rename_count,
            io_read_count,
            io_write_count,
            io_canonicalize_count,
            io_eden_settle_count,
            page_in_count,
            page_in_fetch_us,
            page_in_deser_us,
            page_in_bytes,
            page_in_by_key_type,
            paging_db_size_bytes: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_db_size_bytes),
            paging_resident_node_count: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.resident_node_count),
            paging_paged_out_node_count: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paged_out_node_count),
            paging_candidate_node_count: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.candidate_node_count),
            paging_data_keys_out: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_data_keys_out),
            paging_data_key_bytes_out: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_data_key_bytes_out),
            paging_data_keys_in: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_data_keys_in),
            paging_data_key_bytes_in: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_data_key_bytes_in),
            paging_daemon_data_keys_out: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_daemon_data_keys_out),
            paging_daemon_data_key_bytes_out: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_daemon_data_key_bytes_out),
            paging_daemon_data_keys_in: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_daemon_data_keys_in),
            paging_daemon_data_key_bytes_in: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_daemon_data_key_bytes_in),
            paging_memory_offloaded_bytes: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_memory_offloaded_bytes),
            paging_memory_restored_bytes: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_memory_restored_bytes),
            paging_arc_cache_entries: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_arc_cache_entries),
            paging_evictions: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_evictions),
            paging_evicted_count: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_evicted_count),
            paging_evicted_bytes: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_evicted_bytes),
            paging_evictable_bytes: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.paging_evictable_bytes),
            starlark_partial_deser: self
                .paging_summary
                .as_ref()
                .and_then(|s| s.starlark_partial_deser),
            repo_path: self.repo_path.take(),
        };

        let event = BuckEvent::new(
            SystemTime::now(),
            self.trace_id.dupe(),
            None,
            None,
            buck2_data::RecordEvent {
                data: Some((Box::new(record)).into()),
            }
            .into(),
        );

        if let Some(path) = &self.write_to_path {
            let res = (|| {
                let out = fs_util::create_file(path)
                    // input path from --unstable-write-invocation-record
                    .categorize_input()
                    .buck_error_context("Error opening")?;
                let mut out = std::io::BufWriter::new(out);
                serde_json::to_writer(&mut out, event.event())
                    .buck_error_context("Error writing")?;
                out.flush().buck_error_context("Error flushing")?;
                buck2_error::Ok(())
            })();

            if let Err(e) = res {
                warn_user_or_panic!(
                    "invocation_record_write_failed",
                    e.context(format!(
                        "Failed to write InvocationRecord to `{}`",
                        path.as_path().display()
                    ))
                );
            }
        }
        event
    }

    fn try_read_health_check_tags(&mut self) {
        // The sender may have sent multiple tag messages since the recorder and health checker don't necessarily run at the same frequency.
        // We should not make assumptions about order of sender/receiver drop since the health checker is a BuckEvent subscriber as well.

        // We do have the slight chance that health_check_client reports something after the recorder is dropped.
        // Presently, since the health checks run at every snapshot, the likelihood of missing tags should be low.
        // If we want to ensure that all reports are flushed, we might need to implement an end-of-messages marker.
        if let Some(tags_receiver) = self.health_check_tags_receiver.as_mut() {
            while let Ok(tags) = tags_receiver.try_recv() {
                self.health_check_tags.extend(tags);
            }
        }
    }

    // Collects client-side state and data, suitable for telemetry.
    // NOTE: If data is visible from the daemon, put it in cli::metadata::collect()
    fn default_metadata() -> buck2_data::TypedMetadata {
        let mut ints = IntentionallyStdHashMap::new();
        ints.insert("is_tty".to_owned(), std::io::stderr().is_tty() as i64);
        let mut strings = IntentionallyStdHashMap::new();
        if let Some(agent_identity) = metadata::agent_identity_from_env() {
            strings.insert("client_agent_identity_from_env".to_owned(), agent_identity);
        }
        buck2_data::TypedMetadata { ints, strings }
    }

    fn handle_command_start(
        &mut self,
        command: &buck2_data::CommandStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.metadata.extend(command.metadata.clone());
        self.time_to_command_start = Some(duration_since(event.timestamp(), self.start_time));
        self.command_start_timestamp = Some(event.timestamp());
        Ok(())
    }

    async fn handle_command_end(
        &mut self,
        command: &buck2_data::CommandEnd,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        // Awkwardly unpacks the SpanEnd event so we can read its duration.
        let command_end = match event.data() {
            buck2_data::buck_event::Data::SpanEnd(end) => end.clone(),
            _ => {
                return Err(buck2_error!(
                    ErrorTag::InvalidEvent,
                    "handle_command_end was passed a CommandEnd not contained in a SpanEndEvent"
                ));
            }
        };
        self.command_duration = command_end.duration;
        let command_data = command
            .data
            .as_ref()
            .internal_error("Missing command data")?;

        let build_count = match command_data {
            buck2_data::command_end::Data::Build(..)
            | buck2_data::command_end::Data::Test(..)
            | buck2_data::command_end::Data::Install(..) => {
                let build_completed = if let Some(buck2_data::BuildResult { build_completed }) =
                    command.build_result
                {
                    build_completed
                } else {
                    false
                };
                match self
                    .build_count(build_completed, command_data.variant_name())
                    .await
                {
                    Ok(Some(build_count)) => build_count,
                    Ok(None) => Default::default(),
                    Err(e) => {
                        soft_error!("build_count_error", e);
                        Default::default()
                    }
                }
            }
            // only count builds for commands that set a build_result
            _ => Default::default(),
        };

        self.min_attempted_build_count_since_rebase = build_count.attempted_build_count;
        self.min_build_count_since_rebase = build_count.successful_build_count;

        self.command_end = Some(command.clone());
        Ok(())
    }
    fn handle_command_critical_start(
        &mut self,
        command: &buck2_data::CommandCriticalStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.metadata.extend(command.metadata.clone());
        self.time_to_command_critical_section =
            Some(duration_since(event.timestamp(), self.start_time));
        Ok(())
    }
    fn handle_command_critical_end(
        &mut self,
        command: &buck2_data::CommandCriticalEnd,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.metadata.extend(command.metadata.clone());
        Ok(())
    }

    fn handle_action_execution_start(
        &mut self,
        _action: &buck2_data::ActionExecutionStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        if self.time_to_first_action_execution.is_none() {
            self.time_to_first_action_execution =
                Some(duration_since(event.timestamp(), self.start_time));
        }

        // Increment current in-progress actions counter
        self.current_in_progress_actions = self.current_in_progress_actions.saturating_add(1);

        // Track the maximum in-progress actions
        self.max_in_progress_actions = max(
            self.max_in_progress_actions,
            self.current_in_progress_actions,
        );

        Ok(())
    }
    /// Accumulates one dep-file lookup. The times are absent when the persisted store was not
    /// consulted, which is the common case once the in-memory cache is warm.
    fn handle_match_dep_files_end(
        &mut self,
        match_dep_files: &buck2_data::MatchDepFilesEnd,
    ) -> buck2_error::Result<()> {
        if let Some(probe_us) = match_dep_files.persisted_probe_us {
            self.dep_file_db_probes += 1;
            self.dep_file_db_probe_duration_us += probe_us;
        }
        if let Some(fetches) = match_dep_files.persisted_fetches {
            self.dep_file_db_fetches += fetches;
            self.dep_file_db_fetch_duration_us +=
                match_dep_files.persisted_fetch_us.unwrap_or_default();
        }
        if match_dep_files.outcome == buck2_data::DepFileLookupOutcome::Persisted as i32 {
            self.dep_file_db_hits += 1;
        }
        Ok(())
    }

    fn handle_action_execution_end(
        &mut self,
        action: &buck2_data::ActionExecutionEnd,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.dep_file_db_writes_queued += action.dep_file_db_writes_queued.unwrap_or_default();
        // Decrement current in-progress actions counter
        self.current_in_progress_actions = self.current_in_progress_actions.saturating_sub(1);

        if action.kind == buck2_data::ActionKind::Run as i32 {
            if action_stats::was_fallback_action(action) {
                self.run_fallback_count += 1;
            }

            if let Some(scheduling_mode) = action_stats::scheduling_mode(action)
                && action_stats::was_local_action(action)
            {
                match scheduling_mode {
                    SchedulingMode::LocalOnly => {
                        self.run_local_only_count += 1;
                    }
                    SchedulingMode::FallbackReQueueEstimate => {
                        self.run_fallback_re_queue_count += 1;
                    }
                    _ => {}
                }
            }

            match last_command_execution_kind::get_last_command_execution_kind(action) {
                LastCommandExecutionKind::Local => {
                    self.run_local_count += 1;
                }
                LastCommandExecutionKind::LocalWorker => {
                    self.run_local_count += 1;
                    self.local_actions_executed_via_worker += 1;
                }
                LastCommandExecutionKind::Cached => {
                    self.run_action_cache_count += 1;
                }
                LastCommandExecutionKind::RemoteDepFileCached => {
                    self.run_remote_dep_file_cache_count += 1;
                }
                LastCommandExecutionKind::Remote => {
                    self.run_remote_count += 1;
                }
                LastCommandExecutionKind::RemoteServedFromCache => {
                    self.run_remote_served_from_cache_count += 1;
                }
                LastCommandExecutionKind::RemoteDeduplicated => {
                    self.run_remote_count += 1;
                    self.run_remote_deduplicated_count += 1;
                }
                LastCommandExecutionKind::NoCommand => {
                    self.run_skipped_count += 1;
                }
            }
        }

        if action.eligible_for_full_hybrid.unwrap_or_default() {
            self.eligible_for_full_hybrid = true;
        }

        if action.commands.iter().any(|c| {
            matches!(
                c.status,
                Some(buck2_data::command_execution::Status::Failure(..))
            )
        }) {
            self.run_command_failure_count += 1;
        }

        self.time_to_last_action_execution_end =
            Some(duration_since(event.timestamp(), self.start_time));

        self.exec_time_ms += get_last_command_execution_time(action).exec_time_ms;

        // Accumulate the execution-only interval for the action-concurrency
        // distribution (cache hits / no-command actions are excluded by
        // extract_interval).
        if let Some(interval) = buck2_action_parallelism::extract_interval(action) {
            self.action_intervals.push(interval);
        }

        Ok(())
    }

    fn handle_analysis_start(
        &mut self,
        _analysis: &buck2_data::AnalysisStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.time_to_first_analysis
            .get_or_insert_with(|| duration_since(event.timestamp(), self.start_time));
        Ok(())
    }

    fn handle_load_start(
        &mut self,
        _eval: &buck2_data::LoadBuildFileStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.time_to_load_first_build_file
            .get_or_insert_with(|| duration_since(event.timestamp(), self.start_time));
        Ok(())
    }

    fn handle_executor_stage_start(
        &mut self,
        executor_stage: &buck2_data::ExecutorStageStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        let span_id = if let Some(span_id) = event.span_id() {
            span_id
        } else {
            return Ok(());
        };

        match &executor_stage.stage {
            Some(buck2_data::executor_stage_start::Stage::Re(re_stage)) => match &re_stage.stage {
                Some(buck2_data::re_stage::Stage::Execute(_)) => {
                    self.executor_stages_by_span
                        .insert(span_id.into(), ExecutorStageType::RemoteAction);
                    self.current_in_progress_remote_actions =
                        self.current_in_progress_remote_actions.saturating_add(1);
                    self.max_in_progress_remote_actions = max(
                        self.max_in_progress_remote_actions,
                        self.current_in_progress_remote_actions,
                    );
                    self.time_to_first_command_execution_start
                        .get_or_insert_with(|| duration_since(event.timestamp(), self.start_time));
                }
                Some(buck2_data::re_stage::Stage::WorkerUpload(_))
                | Some(buck2_data::re_stage::Stage::WorkerDownload(_)) => {
                    self.executor_stages_by_span
                        .insert(span_id.into(), ExecutorStageType::RemoteUpload);
                    self.current_in_progress_remote_uploads =
                        self.current_in_progress_remote_uploads.saturating_add(1);
                    self.max_in_progress_remote_uploads = max(
                        self.max_in_progress_remote_uploads,
                        self.current_in_progress_remote_uploads,
                    );
                }
                Some(buck2_data::re_stage::Stage::QueueOverQuota(over_quota)) => {
                    self.re_exceeded_quotas
                        .extend(over_quota.exceeded_quotas.iter().cloned());
                }
                _ => {}
            },
            Some(buck2_data::executor_stage_start::Stage::Local(local_stage)) => {
                if let Some(buck2_data::local_stage::Stage::Execute(_)) = &local_stage.stage {
                    self.executor_stages_by_span
                        .insert(span_id.into(), ExecutorStageType::LocalAction);
                    self.current_in_progress_local_actions =
                        self.current_in_progress_local_actions.saturating_add(1);
                    self.max_in_progress_local_actions = max(
                        self.max_in_progress_local_actions,
                        self.current_in_progress_local_actions,
                    );
                    self.time_to_first_command_execution_start
                        .get_or_insert_with(|| duration_since(event.timestamp(), self.start_time));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_executor_stage_end(
        &mut self,
        executor_stage: &buck2_data::ExecutorStageEnd,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        if executor_stage.cache_query_error.is_some() {
            self.re_action_cache_query_error_count += 1;
        }
        // Look up the stage type from the span ID and decrement the appropriate counter
        if let Some(span_id) = event.span_id() {
            if let Some(stage_type) = self.executor_stages_by_span.remove(&span_id.into()) {
                match stage_type {
                    ExecutorStageType::LocalAction => {
                        self.current_in_progress_local_actions =
                            self.current_in_progress_local_actions.saturating_sub(1);
                    }
                    ExecutorStageType::RemoteAction => {
                        self.current_in_progress_remote_actions =
                            self.current_in_progress_remote_actions.saturating_sub(1);
                    }
                    ExecutorStageType::RemoteUpload => {
                        self.current_in_progress_remote_uploads =
                            self.current_in_progress_remote_uploads.saturating_sub(1);
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_cache_upload_end(
        &mut self,
        cache_upload: &buck2_data::CacheUploadEnd,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        if cache_upload.success {
            self.cache_upload_count += 1;
        }
        self.cache_upload_attempt_count += 1;
        Ok(())
    }

    fn handle_dep_file_upload_end(
        &mut self,
        upload: &buck2_data::DepFileUploadEnd,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        if upload.success {
            self.dep_file_upload_count += 1;
        }
        self.dep_file_upload_attempt_count += 1;
        Ok(())
    }

    fn handle_re_session_created(
        &mut self,
        session: &buck2_data::RemoteExecutionSessionCreated,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.re_session_id = Some(session.session_id.clone());
        self.re_experiment_name = Some(session.experiment_name.clone());
        self.persistent_cache_mode = session.persistent_cache_mode.clone();
        Ok(())
    }

    fn handle_materialization_end(
        &mut self,
        materialization: &buck2_data::MaterializationEnd,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.materialization_output_size += materialization.total_bytes;
        self.materialization_files += materialization.file_count;
        Ok(())
    }

    fn handle_materializer_state_info(
        &mut self,
        materializer_state_info: buck2_data::MaterializerStateInfo,
    ) -> buck2_error::Result<()> {
        self.initial_materializer_entries_from_sqlite =
            Some(materializer_state_info.num_entries_from_sqlite);
        Ok(())
    }

    fn handle_bxl_ensure_artifacts_end(
        &mut self,
        _bxl_ensure_artifacts_end: buck2_data::BxlEnsureArtifactsEnd,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        let bxl_ensure_artifacts_end = match event.data() {
            buck2_data::buck_event::Data::SpanEnd(end) => end.clone(),
            _ => {
                return Err(buck2_error!(
                    ErrorTag::InvalidEvent,
                    "handle_bxl_ensure_artifacts_end was passed a BxlEnsureArtifacts not contained in a SpanEndEvent"
                ));
            }
        };

        self.bxl_ensure_artifacts_duration = bxl_ensure_artifacts_end.duration;
        Ok(())
    }

    fn handle_install_finished(
        &mut self,
        install_finished: &buck2_data::InstallFinished,
    ) -> buck2_error::Result<()> {
        self.install_duration = install_finished.duration;
        self.install_device_metadata = install_finished.device_metadata.clone();
        self.installer_log_url = install_finished.log_url.clone();
        self.installer_name = install_finished.installer_name.clone();
        Ok(())
    }

    fn handle_system_info(
        &mut self,
        system_info: &buck2_data::SystemInfo,
    ) -> buck2_error::Result<()> {
        self.system_info = system_info.clone();
        Ok(())
    }

    fn handle_test_discovery(
        &mut self,
        test_info: &buck2_data::TestDiscovery,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        match &test_info.data {
            Some(buck2_data::test_discovery::Data::Session(session_info)) => {
                self.test_info = Some(session_info.info.clone());
            }
            Some(buck2_data::test_discovery::Data::Tests(..)) | None => {}
        }

        Ok(())
    }

    fn handle_test_discovery_start(
        &mut self,
        _test_discovery: &buck2_data::TestDiscoveryStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.time_to_first_test_discovery
            .get_or_insert_with(|| duration_since(event.timestamp(), self.start_time));
        Ok(())
    }

    fn handle_test_run_start(
        &mut self,
        _test_run: &buck2_data::TestRunStart,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.time_to_first_test_run
            .get_or_insert_with(|| duration_since(event.timestamp(), self.start_time));
        Ok(())
    }

    fn handle_build_phase_end(
        &mut self,
        build_phase_end: &buck2_data::BuildPhaseEnd,
    ) -> buck2_error::Result<()> {
        let end_time = build_phase_end
            .end_time
            .internal_error("Missing `end_time` in BuildPhaseEnd")?;
        let end_time = SystemTime::try_from(end_time)
            .map_err(|e| internal_error!("Invalid `end_time` in BuildPhaseEnd: {e}"))?;
        self.build_phase_end = Some(merge_build_phase_ends(
            self.build_phase_end,
            (end_time, build_phase_end.status()),
        ));
        Ok(())
    }

    fn build_duration_ms(&self) -> Option<u64> {
        let (end_time, _) = self.build_phase_end?;
        duration_as_millis(duration_since(end_time, self.command_start_timestamp?))
    }

    /// The build outcome for commands that have a build phase. Without a `BuildPhaseEnd`
    /// event the outcome is inferred from how the command ended.
    fn build_status(&self) -> Option<buck2_data::BuildStatus> {
        if !matches!(
            self.command_name,
            Some("build" | "test" | "install" | "run")
        ) {
            return None;
        }
        if let Some((_, status)) = self.build_phase_end {
            return Some(status);
        }
        Some(match (self.command_start_timestamp, self.outcome) {
            (_, Some(InvocationOutcome::Cancelled)) => buck2_data::BuildStatus::Cancelled,
            (None, _) => buck2_data::BuildStatus::NotStarted,
            (Some(_), Some(InvocationOutcome::Failed)) => buck2_data::BuildStatus::Failed,
            (Some(_), _) => buck2_data::BuildStatus::Unknown,
        })
    }

    fn handle_test_result(
        &mut self,
        test_result: &buck2_data::TestResult,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        let duration = duration_since(event.timestamp(), self.start_time);
        match test_result.status() {
            buck2_data::TestStatus::Pass => {
                self.time_to_first_pass_test_result.get_or_insert(duration);
            }
            buck2_data::TestStatus::Fail => {
                self.time_to_first_fail_test_result.get_or_insert(duration);
            }
            buck2_data::TestStatus::Fatal => {
                self.time_to_first_fatal_test_result.get_or_insert(duration);
            }
            buck2_data::TestStatus::Skip => {
                self.time_to_first_skip_test_result.get_or_insert(duration);
            }
            buck2_data::TestStatus::InfraFailure => {
                self.time_to_first_infra_failure_test_result
                    .get_or_insert(duration);
            }
            buck2_data::TestStatus::Timeout => {
                self.time_to_first_timeout_test_result
                    .get_or_insert(duration);
            }
            buck2_data::TestStatus::Unknown => {
                self.time_to_first_unknown_test_result
                    .get_or_insert(duration);
            }
            // Listing results, omit and rerun are not actual test results. Do nothing
            buck2_data::TestStatus::ListingFailed
            | buck2_data::TestStatus::ListingSuccess
            | buck2_data::TestStatus::Omitted
            | buck2_data::TestStatus::Rerun
            | buck2_data::TestStatus::NotSetTestStatus => (),
        };
        Ok(())
    }

    fn handle_dice_state_snapshot(
        &mut self,
        dice_state_snapshot: &buck2_data::DiceStateSnapshot,
    ) -> buck2_error::Result<()> {
        // Calculate the total in-progress keys and compute keys across all key types
        let mut total_in_progress = 0u64;
        let mut total_compute = 0u64;

        for key_state in dice_state_snapshot.key_states.values() {
            // In-progress keys are those that have been started but not finished
            let started = u64::from(key_state.started);
            let finished = u64::from(key_state.finished);
            let in_progress = started.saturating_sub(finished);
            total_in_progress = total_in_progress.saturating_add(in_progress);

            // Compute keys are those in the computation phase
            let compute_started = u64::from(key_state.compute_started);
            let compute_finished = u64::from(key_state.compute_finished);
            let compute_in_progress = compute_started.saturating_sub(compute_finished);
            total_compute = total_compute.saturating_add(compute_in_progress);
        }

        // Track the maximum values seen across all snapshots
        self.max_dice_in_progress_keys = max(self.max_dice_in_progress_keys, total_in_progress);
        self.max_dice_compute_keys = max(self.max_dice_compute_keys, total_compute);

        Ok(())
    }

    fn handle_build_graph_info(
        &mut self,
        info: &buck2_data::BuildGraphExecutionInfo,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        let mut duration = Duration::default();
        let mut page_in = Duration::default();

        for node in &info.critical_path2 {
            if let Some(d) = &node.duration {
                let d = d.try_into_duration()?;
                duration += d;
                if matches!(
                    node.entry,
                    Some(buck2_data::critical_path_entry2::Entry::PageIn(_))
                ) {
                    page_in += d;
                }
            }
        }

        self.critical_path_duration = Some(duration);
        self.critical_path_page_in = Some(page_in);
        self.critical_path_backend = info.backend_name.clone();
        Ok(())
    }

    fn handle_io_provider_info(
        &mut self,
        io_provider_info: &buck2_data::IoProviderInfo,
    ) -> buck2_error::Result<()> {
        if let Some(eden_version) = &io_provider_info.eden_version {
            self.eden_version = Some(eden_version.to_owned())
        }
        Ok(())
    }

    fn handle_tag(&mut self, tag: &buck2_data::TagEvent) -> buck2_error::Result<()> {
        self.tags.extend(tag.tags.iter().cloned());
        Ok(())
    }

    fn handle_concurrent_commands(
        &mut self,
        concurrent_commands: &buck2_data::ConcurrentCommands,
    ) -> buck2_error::Result<()> {
        concurrent_commands.trace_ids.iter().for_each(|c| {
            self.concurrent_command_ids.insert(c.clone());
        });
        self.concurrent_commands =
            self.concurrent_commands || concurrent_commands.trace_ids.len() > 1;
        Ok(())
    }

    fn update_peak_system_load(&mut self, elapsed: Duration, load1: f64, load5: f64) {
        let num_cores = self.system_info.num_cores.unwrap_or(1) as f64;
        // `load1`/`load5` are kernel exponential moving averages over the trailing 1 and 5 minutes, so a
        // sample taken sooner than that after the invocation starts still reflects load from before it
        // began. Only fold a sample into the peak once its averaging window lies entirely within the
        // invocation.
        if elapsed >= SYSTEM_LOAD1_WINDOW {
            let normalized = load1 / num_cores;
            self.peak_normalized_system_load1 = Some(
                self.peak_normalized_system_load1
                    .map_or(normalized, |v| f64::max(v, normalized)),
            );
        }
        if elapsed >= SYSTEM_LOAD5_WINDOW {
            let normalized = load5 / num_cores;
            self.peak_normalized_system_load5 = Some(
                self.peak_normalized_system_load5
                    .map_or(normalized, |v| f64::max(v, normalized)),
            );
        }
    }

    fn handle_snapshot(
        &mut self,
        update: &buck2_data::Snapshot,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.max_malloc_bytes_active =
            max(self.max_malloc_bytes_active, update.malloc_bytes_active);
        self.max_malloc_bytes_allocated = max(
            self.max_malloc_bytes_allocated,
            update.malloc_bytes_allocated,
        );
        if self.first_snapshot.is_none() {
            self.first_snapshot = Some(update.clone());
        } else {
            self.last_snapshot = Some(update.clone());
        }
        if self.initial_sink_success_count.is_none() {
            self.initial_sink_success_count = update.sink_successes;
        }
        if self.initial_sink_failure_count.is_none() {
            self.initial_sink_failure_count = update.sink_failures;
        }
        if self.initial_sink_dropped_count.is_none() {
            self.initial_sink_dropped_count = update.sink_dropped;
        }
        if self.initial_sink_bytes_written.is_none() {
            self.initial_sink_bytes_written = update.sink_bytes_written;
        }
        self.sink_max_buffer_depth = max(self.sink_max_buffer_depth, update.sink_buffer_depth());

        if self.initial_re_upload_bytes.is_none() {
            self.initial_re_upload_bytes = Some(update.re_upload_bytes);
        }
        if self.initial_re_download_bytes.is_none() {
            self.initial_re_download_bytes = Some(update.re_download_bytes);
        }

        if self.initial_zdb_download_queries.is_none() {
            self.initial_zdb_download_queries = Some(update.zdb_download_queries);
        }
        if self.initial_zdb_download_bytes.is_none() {
            self.initial_zdb_download_bytes = Some(update.zdb_download_bytes);
        }
        if self.initial_zdb_upload_queries.is_none() {
            self.initial_zdb_upload_queries = Some(update.zdb_upload_queries);
        }
        if self.initial_zdb_upload_bytes.is_none() {
            self.initial_zdb_upload_bytes = Some(update.zdb_upload_bytes);
        }

        if self.initial_zgateway_download_queries.is_none() {
            self.initial_zgateway_download_queries = Some(update.zgateway_download_queries);
        }
        if self.initial_zgateway_download_bytes.is_none() {
            self.initial_zgateway_download_bytes = Some(update.zgateway_download_bytes);
        }
        if self.initial_zgateway_upload_queries.is_none() {
            self.initial_zgateway_upload_queries = Some(update.zgateway_upload_queries);
        }
        if self.initial_zgateway_upload_bytes.is_none() {
            self.initial_zgateway_upload_bytes = Some(update.zgateway_upload_bytes);
        }

        if self.initial_manifold_download_queries.is_none() {
            self.initial_manifold_download_queries = Some(update.manifold_download_queries);
        }
        if self.initial_manifold_download_bytes.is_none() {
            self.initial_manifold_download_bytes = Some(update.manifold_download_bytes);
        }
        if self.initial_manifold_upload_queries.is_none() {
            self.initial_manifold_upload_queries = Some(update.manifold_upload_queries);
        }
        if self.initial_manifold_upload_bytes.is_none() {
            self.initial_manifold_upload_bytes = Some(update.manifold_upload_bytes);
        }

        if self.initial_hedwig_download_queries.is_none() {
            self.initial_hedwig_download_queries = Some(update.hedwig_download_queries);
        }
        if self.initial_hedwig_download_bytes.is_none() {
            self.initial_hedwig_download_bytes = Some(update.hedwig_download_bytes);
        }
        if self.initial_hedwig_upload_queries.is_none() {
            self.initial_hedwig_upload_queries = Some(update.hedwig_upload_queries);
        }
        if self.initial_hedwig_upload_bytes.is_none() {
            self.initial_hedwig_upload_bytes = Some(update.hedwig_upload_bytes);
        }

        if self.initial_local_cache_hits_files.is_none() {
            self.initial_local_cache_hits_files = Some(update.local_cache_hits_files);
        }
        if self.initial_local_cache_hits_bytes.is_none() {
            self.initial_local_cache_hits_bytes = Some(update.local_cache_hits_bytes);
        }
        if self.initial_local_cache_misses_files.is_none() {
            self.initial_local_cache_misses_files = Some(update.local_cache_misses_files);
        }
        if self.initial_local_cache_misses_bytes.is_none() {
            self.initial_local_cache_misses_bytes = Some(update.local_cache_misses_bytes);
        }
        if self
            .initial_local_cache_hits_files_from_memory_cache
            .is_none()
        {
            self.initial_local_cache_hits_files_from_memory_cache =
                Some(update.local_cache_hits_files_from_memory_cache);
        }
        if self
            .initial_local_cache_hits_files_from_filesystem_cache
            .is_none()
        {
            self.initial_local_cache_hits_files_from_filesystem_cache =
                Some(update.local_cache_hits_files_from_filesystem_cache);
        }

        if self.initial_local_cache_lookups.is_none() {
            self.initial_local_cache_lookups = Some(update.local_cache_lookups);
        }

        if self
            .initial_local_cache_lookup_latency_microseconds
            .is_none()
        {
            self.initial_local_cache_lookup_latency_microseconds =
                Some(update.local_cache_lookup_latency_microseconds);
        }

        // Initialize IO counters from first snapshot
        if self.initial_io_copy_count.is_none() {
            self.initial_io_copy_count = update.io_copy_count;
        }
        if self.initial_io_symlink_count.is_none() {
            self.initial_io_symlink_count = update.io_symlink_count;
        }
        if self.initial_io_hardlink_count.is_none() {
            self.initial_io_hardlink_count = update.io_hardlink_count;
        }
        if self.initial_io_mkdir_count.is_none() {
            self.initial_io_mkdir_count = update.io_mkdir_count;
        }
        if self.initial_io_readdir_count.is_none() {
            self.initial_io_readdir_count = update.io_readdir_count;
        }
        if self.initial_io_readdir_eden_count.is_none() {
            self.initial_io_readdir_eden_count = update.io_readdir_eden_count;
        }
        if self.initial_io_rmdir_count.is_none() {
            self.initial_io_rmdir_count = update.io_rmdir_count;
        }
        if self.initial_io_rmdir_all_count.is_none() {
            self.initial_io_rmdir_all_count = update.io_rmdir_all_count;
        }
        if self.initial_io_stat_count.is_none() {
            self.initial_io_stat_count = update.io_stat_count;
        }
        if self.initial_io_stat_eden_count.is_none() {
            self.initial_io_stat_eden_count = update.io_stat_eden_count;
        }
        if self.initial_io_chmod_count.is_none() {
            self.initial_io_chmod_count = update.io_chmod_count;
        }
        if self.initial_io_readlink_count.is_none() {
            self.initial_io_readlink_count = update.io_readlink_count;
        }
        if self.initial_io_remove_count.is_none() {
            self.initial_io_remove_count = update.io_remove_count;
        }
        if self.initial_io_rename_count.is_none() {
            self.initial_io_rename_count = update.io_rename_count;
        }
        if self.initial_io_read_count.is_none() {
            self.initial_io_read_count = update.io_read_count;
        }
        if self.initial_io_write_count.is_none() {
            self.initial_io_write_count = update.io_write_count;
        }
        if self.initial_io_canonicalize_count.is_none() {
            self.initial_io_canonicalize_count = update.io_canonicalize_count;
        }
        if self.initial_io_eden_settle_count.is_none() {
            self.initial_io_eden_settle_count = update.io_eden_settle_count;
        }

        for s in self.re_max_download_speeds.iter_mut() {
            s.update(event.timestamp(), update.re_download_bytes);
        }

        for s in self.re_max_upload_speeds.iter_mut() {
            s.update(event.timestamp(), update.re_upload_bytes);
        }

        self.re_avg_download_speed
            .update(event.timestamp(), update.re_download_bytes);

        self.re_avg_upload_speed
            .update(event.timestamp(), update.re_upload_bytes);

        self.peak_process_memory_bytes =
            max(self.peak_process_memory_bytes, process_memory(update));
        self.peak_used_disk_space_bytes = max(
            self.peak_used_disk_space_bytes,
            update.used_disk_space_bytes,
        );

        if let Some(ref unix_stats) = update.unix_system_stats {
            let elapsed = duration_since(event.timestamp(), self.start_time);
            self.update_peak_system_load(elapsed, unix_stats.load1, unix_stats.load5);
        }

        // Track maximum buck2 daemon memory usage from cgroup
        if let Some(allprocs_cgroup) = &update.allprocs_cgroup {
            self.memory_max_anon_allprocs =
                max(self.memory_max_anon_allprocs, Some(allprocs_cgroup.anon));
            let total_daemon_memory =
                allprocs_cgroup.anon + allprocs_cgroup.file + allprocs_cgroup.kernel;
            self.memory_max_total_allprocs =
                max(self.memory_max_total_allprocs, Some(total_daemon_memory));
            // Track peak allprocs swap usage
            self.memory_max_swap_bytes_allprocs = max(
                self.memory_max_swap_bytes_allprocs,
                Some(allprocs_cgroup.swap_bytes),
            );
            // Track peak allprocs memory pressure (avg10 from PSI)
            let pct = allprocs_cgroup.memory_pressure_10s_avg;
            self.memory_max_pressure_10s_avg_allprocs = Some(
                self.memory_max_pressure_10s_avg_allprocs
                    .map_or(pct, |v| f64::max(v, pct)),
            );
            // Track peak allprocs memory pressure (avg60 from PSI)
            let pct = allprocs_cgroup.memory_pressure_60s_avg;
            self.memory_max_pressure_60s_avg_allprocs = Some(
                self.memory_max_pressure_60s_avg_allprocs
                    .map_or(pct, |v| f64::max(v, pct)),
            );
        }

        // Track maximum buck2 forkserver memory usage from cgroup
        if let Some(forkserver_actions_cgroup) = &update.forkserver_actions_cgroup {
            self.memory_max_anon_forkserver_actions = max(
                self.memory_max_anon_forkserver_actions,
                Some(forkserver_actions_cgroup.anon),
            );
            let total_forkserver_memory = forkserver_actions_cgroup.anon
                + forkserver_actions_cgroup.file
                + forkserver_actions_cgroup.kernel;
            self.memory_max_total_forkserver_actions = max(
                self.memory_max_total_forkserver_actions,
                Some(total_forkserver_memory),
            );
        }

        for stat in update.network_interface_stats.values() {
            if stat.rx_bytes > 0 || stat.tx_bytes > 0 {
                self.active_networks_kinds.insert(stat.network_kind);
            }
        }
        self.try_read_health_check_tags();

        Ok(())
    }

    fn handle_file_watcher_end(
        &mut self,
        file_watcher: &buck2_data::FileWatcherEnd,
        duration: Option<&prost_types::Duration>,
        _event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        // We might receive this event twice, so ... deal with it by merging the two.
        // See: https://fb.workplace.com/groups/buck2dev/permalink/3396726613948720/
        self.file_watcher_stats =
            merge_file_watcher_stats(self.file_watcher_stats.take(), file_watcher.stats.clone());
        if let Some(duration) = duration.copied().and_then(|x| Duration::try_from(x).ok()) {
            *self.file_watcher_duration.get_or_insert_default() += duration;
        }
        if let Some(stats) = &file_watcher.stats {
            self.watchman_version = stats.watchman_version.to_owned();
        }
        if let Some(eden_version) = file_watcher
            .stats
            .as_ref()
            .and_then(|s| s.eden_version.clone())
        {
            self.eden_version = Some(eden_version);
        }
        Ok(())
    }

    fn handle_file_watcher_start(
        &mut self,
        file_watcher: FileWatcherStart,
    ) -> buck2_error::Result<()> {
        self.file_watcher = FileWatcherProvider::try_from(file_watcher.provider)
            .ok()
            .map(|p| p.as_str_name().to_owned());
        Ok(())
    }

    fn handle_parsed_target_patterns(
        &mut self,
        patterns: &buck2_data::ParsedTargetPatterns,
    ) -> buck2_error::Result<()> {
        self.parsed_target_patterns = Some(patterns.clone());
        Ok(())
    }

    fn handle_structured_error(
        &mut self,
        err: &buck2_data::StructuredError,
    ) -> buck2_error::Result<()> {
        if let Some(soft_error_category) = err.soft_error_category.as_ref() {
            self.soft_error_categories
                .insert(soft_error_category.to_owned());

            if err.daemon_in_memory_state_is_corrupted {
                self.daemon_in_memory_state_is_corrupted = true;
            }

            if err.daemon_materializer_state_is_corrupted {
                self.daemon_materializer_state_is_corrupted = true;
            }
        }

        Ok(())
    }

    fn handle_dice_block_concurrent_command_end(
        &mut self,
        _command: &buck2_data::DiceBlockConcurrentCommandEnd,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        let block_concurrent_command = match event.data() {
            buck2_data::buck_event::Data::SpanEnd(end) => end.clone(),
            _ => {
                return Err(buck2_error!(
                    ErrorTag::InvalidEvent,
                    "handle_dice_block_concurrent_command_end was passed a DiceBlockConcurrentCommandEnd not contained in a SpanEndEvent"
                ));
            }
        };

        let mut duration = self
            .concurrent_command_blocking_duration
            .unwrap_or_default();
        if let Some(d) = &block_concurrent_command.duration {
            duration += d.try_into_duration()?;
        }

        self.concurrent_command_blocking_duration = Some(duration);

        Ok(())
    }

    fn handle_dice_cleanup_end(
        &mut self,
        _command: buck2_data::DiceCleanupEnd,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        let dice_cleanup_end = match event.data() {
            buck2_data::buck_event::Data::SpanEnd(end) => end.clone(),
            _ => {
                return Err(buck2_error!(
                    ErrorTag::InvalidEvent,
                    "handle_dice_cleanup_end was passed a DiceCleanupEnd not contained in a SpanEndEvent"
                ));
            }
        };

        let mut duration = self
            .concurrent_command_blocking_duration
            .unwrap_or_default();
        if let Some(d) = &dice_cleanup_end.duration {
            duration += d.try_into_duration()?;
        }

        self.concurrent_command_blocking_duration = Some(duration);

        Ok(())
    }

    fn handle_version_control(
        &mut self,
        revision: &buck2_data::VersionControlRevision,
    ) -> buck2_error::Result<()> {
        self.hg_revision = revision.hg_revision.clone().or(self.hg_revision.clone());
        self.git_revision = revision.git_revision.clone().or(self.git_revision.clone());
        self.has_local_changes = revision.has_local_changes.or(self.has_local_changes);
        self.version_control_errors
            .extend(revision.command_error.clone());

        Ok(())
    }

    fn handle_command_options(
        &mut self,
        command_options: &buck2_data::CommandOptions,
    ) -> buck2_error::Result<()> {
        self.command_options = Some(*command_options);
        Ok(())
    }

    async fn handle_event(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        // TODO(nga): query now once in `EventsCtx`.
        let now = SystemTime::now();
        if let Ok(delay) = now.duration_since(event.timestamp()) {
            self.max_event_client_delay =
                Some(max(self.max_event_client_delay.unwrap_or_default(), delay));
        }
        self.event_count += 1;

        match event.data() {
            buck2_data::buck_event::Data::SpanStart(start) => {
                match start.data.as_ref().internal_error("Missing `start`")? {
                    buck2_data::span_start_event::Data::Command(command) => {
                        self.handle_command_start(command, event)
                    }
                    buck2_data::span_start_event::Data::CommandCritical(command) => {
                        self.handle_command_critical_start(command, event)
                    }
                    buck2_data::span_start_event::Data::ActionExecution(action) => {
                        self.handle_action_execution_start(action, event)
                    }
                    buck2_data::span_start_event::Data::Analysis(analysis) => {
                        self.handle_analysis_start(analysis, event)
                    }
                    buck2_data::span_start_event::Data::Load(eval) => {
                        self.handle_load_start(eval, event)
                    }
                    buck2_data::span_start_event::Data::ExecutorStage(stage) => {
                        self.handle_executor_stage_start(stage, event)
                    }
                    buck2_data::span_start_event::Data::TestDiscovery(test_discovery) => {
                        self.handle_test_discovery_start(test_discovery, event)
                    }
                    buck2_data::span_start_event::Data::TestRun(test_start) => {
                        self.handle_test_run_start(test_start, event)
                    }
                    buck2_data::span_start_event::Data::FileWatcher(file_watcher) => {
                        self.handle_file_watcher_start(*file_watcher)
                    }
                    _ => Ok(()),
                }
            }
            buck2_data::buck_event::Data::SpanEnd(end) => {
                match end.data.as_ref().internal_error("Missing `end`")? {
                    buck2_data::span_end_event::Data::Command(command) => {
                        self.handle_command_end(command, event).await
                    }
                    buck2_data::span_end_event::Data::CommandCritical(command) => {
                        self.handle_command_critical_end(command, event)
                    }
                    buck2_data::span_end_event::Data::ActionExecution(action) => {
                        self.handle_action_execution_end(action, event)
                    }
                    buck2_data::span_end_event::Data::MatchDepFiles(match_dep_files) => {
                        self.handle_match_dep_files_end(match_dep_files)
                    }
                    buck2_data::span_end_event::Data::FileWatcher(file_watcher) => {
                        self.handle_file_watcher_end(file_watcher, end.duration.as_ref(), event)
                    }
                    buck2_data::span_end_event::Data::CacheUpload(cache_upload) => {
                        self.handle_cache_upload_end(cache_upload, event)
                    }
                    buck2_data::span_end_event::Data::DepFileUpload(dep_file_upload) => {
                        self.handle_dep_file_upload_end(dep_file_upload, event)
                    }
                    buck2_data::span_end_event::Data::Materialization(materialization) => {
                        self.handle_materialization_end(materialization, event)
                    }
                    buck2_data::span_end_event::Data::Analysis(..) => {
                        self.analysis_count += 1;
                        Ok(())
                    }
                    buck2_data::span_end_event::Data::Load(..) => {
                        self.load_count += 1;
                        Ok(())
                    }
                    buck2_data::span_end_event::Data::DiceBlockConcurrentCommand(
                        block_concurrent_command,
                    ) => self
                        .handle_dice_block_concurrent_command_end(block_concurrent_command, event),
                    buck2_data::span_end_event::Data::DiceCleanup(dice_cleanup_end) => {
                        self.handle_dice_cleanup_end(*dice_cleanup_end, event)
                    }
                    buck2_data::span_end_event::Data::ExecutorStage(executor_stage) => {
                        self.handle_executor_stage_end(executor_stage, event)
                    }
                    buck2_data::span_end_event::Data::BxlEnsureArtifacts(_bxl_ensure_artifacts) => {
                        self.handle_bxl_ensure_artifacts_end(*_bxl_ensure_artifacts, event)
                    }
                    _ => Ok(()),
                }
            }
            buck2_data::buck_event::Data::Instant(instant) => {
                match instant.data.as_ref().internal_error("Missing `data`")? {
                    buck2_data::instant_event::Data::ReSession(session) => {
                        self.handle_re_session_created(session, event)
                    }
                    buck2_data::instant_event::Data::BuildGraphInfo(info) => {
                        self.handle_build_graph_info(info, event)
                    }
                    buck2_data::instant_event::Data::TestDiscovery(discovery) => {
                        self.handle_test_discovery(discovery, event)
                    }
                    buck2_data::instant_event::Data::Snapshot(result) => {
                        self.handle_snapshot(result, event)
                    }
                    buck2_data::instant_event::Data::TagEvent(tag) => self.handle_tag(tag),
                    buck2_data::instant_event::Data::IoProviderInfo(io_provider_info) => {
                        self.handle_io_provider_info(io_provider_info)
                    }
                    buck2_data::instant_event::Data::TargetPatterns(tag) => {
                        self.handle_parsed_target_patterns(tag)
                    }
                    buck2_data::instant_event::Data::MaterializerStateInfo(materializer_state) => {
                        self.handle_materializer_state_info(*materializer_state)
                    }
                    buck2_data::instant_event::Data::StructuredError(err) => {
                        self.handle_structured_error(err)
                    }
                    buck2_data::instant_event::Data::RestartConfiguration(conf) => {
                        self.enable_restarter = conf.enable_restarter;
                        Ok(())
                    }
                    buck2_data::instant_event::Data::ConcurrentCommands(concurrent_commands) => {
                        self.handle_concurrent_commands(concurrent_commands)
                    }
                    buck2_data::instant_event::Data::CellHasNewConfigs(_) => {
                        self.has_new_buckconfigs = true;
                        Ok(())
                    }
                    buck2_data::instant_event::Data::InstallFinished(install_finished) => {
                        self.handle_install_finished(install_finished)
                    }
                    buck2_data::instant_event::Data::BuildPhaseEnd(build_phase_end) => {
                        self.handle_build_phase_end(build_phase_end)
                    }
                    buck2_data::instant_event::Data::SystemInfo(system_info) => {
                        self.handle_system_info(system_info)
                    }
                    buck2_data::instant_event::Data::PagingSummary(paging_summary) => {
                        self.paging_summary = Some((**paging_summary).clone());
                        Ok(())
                    }
                    buck2_data::instant_event::Data::TargetCfg(target_cfg) => {
                        self.target_cfg = Some(target_cfg.clone());
                        Ok(())
                    }
                    buck2_data::instant_event::Data::TargetRuleTypeName(rule_type) => {
                        self.target_rule_type_names
                            .push(rule_type.rule_type.clone());
                        Ok(())
                    }
                    buck2_data::instant_event::Data::VersionControlRevision(revision) => {
                        self.handle_version_control(revision)
                    }
                    buck2_data::instant_event::Data::PreviousCommandWithMismatchedConfig(
                        command,
                    ) => {
                        self.previous_uuid_with_mismatched_config = Some(command.trace_id.clone());
                        Ok(())
                    }
                    buck2_data::instant_event::Data::TestResult(result) => {
                        self.handle_test_result(result, event)
                    }
                    buck2_data::instant_event::Data::DiceStateSnapshot(dice_state_snapshot) => {
                        self.handle_dice_state_snapshot(dice_state_snapshot)
                    }
                    buck2_data::instant_event::Data::CommandOptions(command_options) => {
                        self.handle_command_options(command_options)
                    }
                    _ => Ok(()),
                }
            }
            buck2_data::buck_event::Data::Record(_) => Ok(()),
        }
    }
}

const TIER0: &str = "INFRA";
const ENVIRONMENT: &str = "ENVIRONMENT";
const INPUT: &str = "USER";

fn process_error_report(error: buck2_data::ErrorReport) -> buck2_data::ProcessedErrorReport {
    let best_tag = error.best_tag();
    let best_tag = best_tag
        .map_or(
            // If we don't have tags on the errors,
            // we still want to add a tag to Scuba column.
            ERROR_TAG_UNCLASSIFIED,
            |tag| tag.as_str_name(),
        )
        .to_owned();

    let category = match error.category() {
        Tier::Tier0 => TIER0.to_owned(),
        Tier::Environment => ENVIRONMENT.to_owned(),
        Tier::Input => INPUT.to_owned(),
    };
    let tags = error
        .tags
        .iter()
        .copied()
        .filter_map(|v| ErrorTag::try_from(v).ok());

    let source_area = source_area(tags.clone()).to_string().to_ascii_uppercase();
    let tags = tags.map(|t| t.as_str_name().to_owned());

    let string_tags = error.string_tags.iter().map(|t| t.tag.clone());
    let tags = tags.chain(string_tags).collect();

    buck2_data::ProcessedErrorReport {
        message: strip_ansi_codes(&error.message).to_string(),
        telemetry_message: error
            .telemetry_message
            .map(|m| strip_ansi_codes(&m).to_string()),
        source_location: error
            .source_location
            .map(|s| SourceLocation::from(s).to_string()),
        tags,
        best_tag: Some(best_tag),
        sub_error_categories: error.sub_error_categories,
        category_key: error.category_key,
        category: Some(category),
        source_area: Some(source_area),
    }
}

fn unique_and_sorted<T: Iterator<Item = String>>(input: T) -> Vec<String> {
    let mut unique: Vec<String> = input.unique_by(|x| x.clone()).collect();
    unique.sort();
    unique
}

#[async_trait]
impl EventSubscriber for InvocationRecorder {
    fn name(&self) -> &'static str {
        "invocation recorder"
    }

    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        for event in events {
            self.handle_event(event).await?;
        }
        Ok(())
    }

    async fn handle_console_interaction(
        &mut self,
        c: &ConsoleInteraction,
    ) -> buck2_error::Result<()> {
        let ConsoleInteraction::Toggle(c) = c else {
            return Ok(());
        };
        self.tags
            .push(format!("superconsole-toggle:{}", c.key()).to_owned());
        Ok(())
    }

    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        self.has_command_result = true;
        match &result.result {
            Some(command_result::Result::BuildResponse(res)) => {
                // Append per-BuildTarget rule type names from the build
                // response. Extending (not assigning) preserves rule types
                // already accumulated from `TargetRuleTypeName` instant events.
                self.target_rule_type_names
                    .extend(res.build_targets.iter().map(|t| {
                        t.target_rule_type_name
                            .clone()
                            .unwrap_or_else(|| "NULL".to_owned())
                    }));
            }
            Some(command_result::Result::InstallResponse(res)) => {
                self.target_rule_type_names
                    .extend(res.target_rule_type_names.iter().cloned());
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_exit_result(&mut self, exit_result: &ExitResult) {
        self.command_errors = exit_result.get_all_errors();
        self.exit_code = exit_result.exit_code().map(|code| code.exit_code());
        self.exit_result_name = Some(exit_result.name().to_owned());
        self.outcome = Some(self.outcome(exit_result));
    }

    async fn handle_tailer_stderr(&mut self, stderr: &str) -> buck2_error::Result<()> {
        if self.server_stderr.len() > 100_000 {
            // Proper truncation of the head is tricky, and for practical purposes
            // discarding the whole thing is fine.
            self.server_stderr.clear();
        }

        if !stderr.is_empty() {
            // We don't know yet whether we will need stderr or not,
            // so we capture it unconditionally.
            self.server_stderr.push_str(stderr);
            self.server_stderr.push('\n');
        }

        Ok(())
    }

    fn handle_stream_end(&mut self) {
        self.has_end_of_stream = true;
    }

    async fn finalize(mut self: Box<Self>) -> buck2_error::Result<()> {
        let event = self.create_record_event();

        #[cfg(not(fbcode_build))]
        {
            drop(event);
            // In OSS, daemon-side BES upload is the source of truth for the
            // invocation stream. Sending a second client-side stream with the
            // same invocation ID causes BuildBuddy to cancel/preempt attempts
            // and can downgrade a successful invocation to disconnected.
            return Ok(());
        }

        #[cfg(fbcode_build)]
        {
            // Can't set this before the daemon forks.
            // Typically initialized already unless the command failed early.
            let fb = buck2_common::fbinit::get_or_init_fbcode_globals();
            let remote_sink_config =
                std::mem::replace(&mut self.remote_sink_config, RemoteEventConfig::default());
            if let Some(scribe_sink) = new_remote_event_sink_if_enabled(fb, remote_sink_config)? {
                tracing::info!("Recording invocation to Scribe: {:?}", &event);
                scribe_sink.send_now(event).await
            } else {
                tracing::info!("Invocation record is not sent to Scribe: {:?}", &event);
                Err(internal_error!("Scribe sink not enabled"))
            }
        }
    }

    fn as_error_observer(&self) -> Option<&dyn ErrorObserver> {
        Some(self)
    }

    fn handle_daemon_connection_failure(&mut self) {
        self.daemon_connection_failure = true;
    }

    fn handle_daemon_started(&mut self, daemon_was_started: buck2_data::DaemonWasStartedReason) {
        self.daemon_was_started = Some(daemon_was_started);
    }

    fn handle_should_restart(&mut self) {
        self.should_restart = self.restarted_trace_id.is_none();
    }
}

impl ErrorObserver for InvocationRecorder {
    fn daemon_in_memory_state_is_corrupted(&self) -> bool {
        self.daemon_in_memory_state_is_corrupted
    }

    fn daemon_materializer_state_is_corrupted(&self) -> bool {
        self.daemon_materializer_state_is_corrupted
    }

    fn restarter_is_enabled(&self) -> bool {
        self.enable_restarter
    }
}

fn calculate_diff_if_some<T>(a: &Option<T>, b: &Option<T>) -> Option<T>
where
    for<'a> &'a T: Sub<&'a T, Output = T>,
    T: Ord,
{
    match (a, b) {
        (Some(av), Some(bv)) => Some(max(av, bv) - min(av, bv)),
        _ => None,
    }
}

fn merge_file_watcher_stats(
    a: Option<buck2_data::FileWatcherStats>,
    b: Option<buck2_data::FileWatcherStats>,
) -> Option<buck2_data::FileWatcherStats> {
    let (mut a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (a, None) => return a,
        (None, b) => return b,
    };

    a.fresh_instance = a.fresh_instance || b.fresh_instance;
    a.events_total += b.events_total;
    a.events_processed += b.events_processed;
    a.branched_from_revision = a.branched_from_revision.or(b.branched_from_revision);
    a.branched_from_global_rev = a.branched_from_global_rev.or(b.branched_from_global_rev);
    a.branched_from_revision_timestamp = a
        .branched_from_revision_timestamp
        .or(b.branched_from_revision_timestamp);
    a.events.extend(b.events);
    a.incomplete_events_reason = a.incomplete_events_reason.or(b.incomplete_events_reason);
    a.watchman_version = a.watchman_version.or(b.watchman_version);
    a.eden_version = a.eden_version.or(b.eden_version);
    Some(a)
}

fn truncate_stderr(stderr: &str) -> &str {
    // If server crashed, it means something is very broken,
    // and we don't really need nicely formatted stderr.
    // We only need to see it once, fix it, and never see it again.
    let max_len = 20_000;
    let truncate_at = stderr.len().saturating_sub(max_len);
    let truncate_at = stderr.ceil_char_boundary(truncate_at);
    &stderr[truncate_at..]
}

fn duration_since(end_time: SystemTime, start_time: SystemTime) -> Duration {
    end_time.duration_since(start_time).unwrap_or_default()
}

/// A command sends one `BuildPhaseEnd` per independent build it runs (e.g. one per install
/// request). The build phase ends with the last of them and has the worst of their statuses.
fn merge_build_phase_ends(
    prev: Option<(SystemTime, buck2_data::BuildStatus)>,
    next: (SystemTime, buck2_data::BuildStatus),
) -> (SystemTime, buck2_data::BuildStatus) {
    fn severity(status: buck2_data::BuildStatus) -> u8 {
        match status {
            buck2_data::BuildStatus::Success => 0,
            buck2_data::BuildStatus::Unknown => 1,
            buck2_data::BuildStatus::Cancelled => 2,
            buck2_data::BuildStatus::NotStarted => 3,
            buck2_data::BuildStatus::Failed => 4,
        }
    }
    match prev {
        None => next,
        Some((prev_time, prev_status)) => (
            max(prev_time, next.0),
            std::cmp::max_by_key(prev_status, next.1, |s| severity(*s)),
        ),
    }
}

fn duration_as_millis(duration: Duration) -> Option<u64> {
    u64::try_from(duration.as_millis()).ok()
}

#[cfg(test)]
mod tests {

    use std::ffi::OsString;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::SystemTime;

    use buck2_data::InvocationOutcome;
    use buck2_error::ErrorTag;
    use buck2_error::ExitCode;
    use buck2_error::buck2_error;
    use buck2_error::internal_error;
    use buck2_events::BuckEvent;
    use buck2_wrapper_common::invocation_id::TraceId;

    use crate::exit_result::ExecEnvironment;
    use crate::exit_result::ExitResult;
    use crate::subscribers::recorder::InvocationRecorder;
    use crate::subscribers::recorder::merge_build_phase_ends;
    use crate::subscribers::recorder::truncate_stderr;
    use crate::subscribers::subscriber::EventSubscriber;

    #[test]
    fn test_truncate_stderr() {
        let mut stderr = String::new();
        stderr.push_str("prefix");
        stderr.push('Ъ'); // 2 bytes, so asking to truncate in the middle of the char.
        for _ in 0..19_999 {
            stderr.push('a');
        }
        let truncated = truncate_stderr(&stderr);
        assert_eq!(truncated.len(), 19_999);
    }

    #[test]
    fn test_outcome() {
        let mut recorder =
            InvocationRecorder::new(TraceId::new(), None, SystemTime::UNIX_EPOCH, vec![]);
        let exit_result = ExitResult::success();
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Success);
        let err = internal_error!("test");
        let exit_result = ExitResult::err_with_exit_code(err.clone(), ExitCode::Success);
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Unknown);

        let exit_result = ExitResult::err_with_exit_code(err.clone(), ExitCode::SignalInterrupt);
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Cancelled);

        let exit_result = ExitResult::err_with_exit_code(err.clone(), ExitCode::InfraError);
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Failed);
        recorder.daemon_connection_failure = true;
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Crashed);
        recorder.daemon_connection_failure = false;

        let exit_result =
            ExitResult::exec(OsString::new(), vec![], None, ExecEnvironment::default());
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Success);

        let err = buck2_error!(ErrorTag::IoClientBrokenPipe, "test");
        let exit_result = ExitResult::err(err);
        assert_eq!(recorder.outcome(&exit_result), InvocationOutcome::Cancelled);
    }

    fn recorder_for(command_name: &'static str) -> InvocationRecorder {
        let mut recorder =
            InvocationRecorder::new(TraceId::new(), None, SystemTime::UNIX_EPOCH, vec![]);
        recorder.command_name = Some(command_name);
        recorder
    }

    fn at_secs(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn test_build_status_from_build_phase_end() {
        let mut recorder = recorder_for("test");
        recorder.command_start_timestamp = Some(at_secs(100));
        recorder.build_phase_end = Some((at_secs(160), buck2_data::BuildStatus::Success));
        // Tests failing after the build don't change the build outcome.
        recorder.outcome = Some(InvocationOutcome::Failed);
        assert_eq!(
            recorder.build_status(),
            Some(buck2_data::BuildStatus::Success)
        );
        assert_eq!(recorder.build_duration_ms(), Some(60_000));
    }

    #[test]
    fn test_merge_build_phase_ends() {
        use buck2_data::BuildStatus;
        let merged = [
            (at_secs(30), BuildStatus::Success),
            (at_secs(50), BuildStatus::Success),
            (at_secs(40), BuildStatus::Failed),
        ]
        .into_iter()
        .fold(None, |acc, next| Some(merge_build_phase_ends(acc, next)));
        assert_eq!(merged, Some((at_secs(50), BuildStatus::Failed)));
    }

    #[test]
    fn test_build_status_without_build_phase_end() {
        let mut recorder = recorder_for("build");
        assert_eq!(
            recorder.build_status(),
            Some(buck2_data::BuildStatus::NotStarted)
        );
        assert_eq!(recorder.build_duration_ms(), None);

        recorder.command_start_timestamp = Some(at_secs(100));
        recorder.outcome = Some(InvocationOutcome::Failed);
        assert_eq!(
            recorder.build_status(),
            Some(buck2_data::BuildStatus::Failed)
        );
        recorder.outcome = Some(InvocationOutcome::Cancelled);
        assert_eq!(
            recorder.build_status(),
            Some(buck2_data::BuildStatus::Cancelled)
        );
        recorder.outcome = Some(InvocationOutcome::Crashed);
        assert_eq!(
            recorder.build_status(),
            Some(buck2_data::BuildStatus::Unknown)
        );

        assert_eq!(recorder_for("targets").build_status(), None);
    }

    #[test]
    fn test_peak_system_load_excludes_pre_invocation_window() {
        let mut recorder =
            InvocationRecorder::new(TraceId::new(), None, SystemTime::UNIX_EPOCH, vec![]);
        // num_cores = 1 makes normalization the identity, so peaks equal the raw loads.
        recorder.system_info.num_cores = Some(1);

        // A sample inside both windows must be ignored: its averaging window reaches before the
        // invocation started.
        recorder.update_peak_system_load(Duration::from_secs(30), 10.0, 10.0);
        assert_eq!(
            recorder.peak_normalized_system_load1, None,
            "load1 sampled within the 1-minute window must be ignored"
        );
        assert_eq!(
            recorder.peak_normalized_system_load5, None,
            "load5 sampled within the 5-minute window must be ignored"
        );

        // At exactly the 1-minute boundary, load1 counts but load5 is still inside its window.
        recorder.update_peak_system_load(Duration::from_secs(60), 4.0, 99.0);
        assert_eq!(recorder.peak_normalized_system_load1, Some(4.0));
        assert_eq!(
            recorder.peak_normalized_system_load5, None,
            "load5 sampled within the 5-minute window must be ignored"
        );

        // At the 5-minute boundary both count; peak stays the running max, so the earlier larger
        // load1 (4.0) is kept over the later 2.0.
        recorder.update_peak_system_load(Duration::from_secs(5 * 60), 2.0, 7.0);
        assert_eq!(recorder.peak_normalized_system_load1, Some(4.0));
        assert_eq!(recorder.peak_normalized_system_load5, Some(7.0));
    }

    fn snapshot_event(
        trace_id: &TraceId,
        at: SystemTime,
        snapshot: buck2_data::Snapshot,
    ) -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
            at,
            trace_id.clone(),
            None,
            None,
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                data: Some(buck2_data::instant_event::Data::Snapshot(Box::new(
                    snapshot,
                ))),
            }),
        ))
    }

    fn record_of(event: &buck2_events::BuckEvent) -> buck2_data::InvocationRecord {
        match event.data() {
            buck2_data::buck_event::Data::Record(r) => match r.data.as_ref().unwrap() {
                buck2_data::record_event::Data::InvocationRecord(r) => (**r).clone(),
                _ => panic!("not an invocation record"),
            },
            _ => panic!("not a record"),
        }
    }

    /// The record after two snapshots whose local cache counters moved by the given amounts.
    async fn record_after_lookups(lookups: i64, latency_us: i64) -> buck2_data::InvocationRecord {
        let trace_id = TraceId::new();
        let start = SystemTime::now();
        let mut recorder = InvocationRecorder::new(trace_id.clone(), None, start, vec![]);
        // One second apart: the snapshot rates divide by the elapsed time.
        recorder
            .handle_events(&[
                snapshot_event(&trace_id, start, buck2_data::Snapshot::default()),
                snapshot_event(
                    &trace_id,
                    start + Duration::from_secs(1),
                    buck2_data::Snapshot {
                        local_cache_lookups: lookups,
                        local_cache_lookup_latency_microseconds: latency_us,
                        ..Default::default()
                    },
                ),
            ])
            .await
            .unwrap();
        record_of(&recorder.create_record_event())
    }

    /// With no local cache lookups between the snapshots there is no average: the field is
    /// absent instead of `NaN` or `inf`.
    #[tokio::test]
    async fn test_average_local_cache_lookup_without_lookups_is_absent() {
        let record = record_after_lookups(0, 0).await;
        assert_eq!(record.local_cache_lookups, Some(0));
        assert_eq!(record.re_average_local_cache_lookup_microseconds, None);

        let record = record_after_lookups(0, 10).await;
        assert_eq!(record.re_average_local_cache_lookup_microseconds, None);

        let record = record_after_lookups(4, 10).await;
        assert_eq!(record.re_average_local_cache_lookup_microseconds, Some(2.5));
    }
}
