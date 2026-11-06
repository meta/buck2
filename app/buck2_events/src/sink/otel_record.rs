/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Maps an [`InvocationRecord`](buck2_data::InvocationRecord) to OTLP span attributes for the
//! end-of-invocation "wide event".
//!
//! Every field is mapped *explicitly*, by hand, rather than by serializing the proto and flattening
//! it generically. This matches how the upstream Scuba pipeline curates its schema: the shape of
//! each column (key name, unit, how repeated/nested values are projected) is a deliberate choice,
//! not a mechanical consequence of the proto layout. Attribute keys follow the proto's own
//! serde field names so they line up with the event-log JSON (durations as `_us` micros, etc.).
//!
//! Repeated scalar fields become a single multi-valued attribute (the OTLP equivalent of a Scuba
//! normvector). Repeated *messages* are projected field-wise into parallel multi-valued attributes
//! (`errors.category`, `errors.message`, ...), since a list of structs has no single-column form.
//!
//! ## Attribute namespacing
//!
//! Every attribute is emitted under the `buck2.` namespace (e.g. `re_session_id` becomes
//! `buck2.re_session_id`), *except* for the handful of fields that correspond to an existing
//! OpenTelemetry [semantic convention], which are emitted under the standardized attribute name
//! (taken from the [`opentelemetry_semantic_conventions`] crate) so backends can interpret them
//! generically. The prefixing is centralized in [`Attrs::push`]; the standardized fields opt out
//! via the `*_std` helpers and the [`opentelemetry_semantic_conventions::attribute`] constants.
//!
//! The mapped fields are `cli_args` -> `process.command_args`, `exit_code` -> `process.exit.code`,
//! `git_revision`/`hg_revision` -> `vcs.ref.head.revision` (the current checkout), and
//! `file_watcher_stats.branched_from_revision` -> `vcs.ref.base.revision` (the mergebase it
//! branched from). The structured `version_control_revision` sub-message carries the same revision
//! as the top-level `git_revision`/`hg_revision`, so it stays under `buck2.` to avoid emitting
//! `vcs.ref.head.revision` twice.
//!
//! [semantic convention]: https://opentelemetry.io/docs/specs/semconv/

use std::collections::HashMap;

use opentelemetry::Array;
use opentelemetry::Key;
use opentelemetry::KeyValue;
use opentelemetry::StringValue;
use opentelemetry::Value;
use opentelemetry_semantic_conventions::attribute::PROCESS_COMMAND_ARGS;
use opentelemetry_semantic_conventions::attribute::PROCESS_EXIT_CODE;
use opentelemetry_semantic_conventions::attribute::VCS_REF_BASE_REVISION;
use opentelemetry_semantic_conventions::attribute::VCS_REF_HEAD_REVISION;

/// Accumulates OTLP attributes while keeping each field mapping to a single readable line. The
/// helpers centralise the `Option`/unit/empty-skipping conventions so the mapping below reads as a
/// flat list of "field -> key" decisions.
#[derive(Default)]
struct Attrs {
    attrs: Vec<KeyValue>,
}

impl Attrs {
    /// Push an attribute under the `buck2.` namespace. All buck2-specific keys funnel through here
    /// so the prefixing is applied in exactly one place; fields that map onto an OpenTelemetry
    /// semantic convention bypass it via [`Attrs::push_std`].
    fn push(&mut self, key: impl Into<Key>, value: impl Into<Value>) {
        let key = key.into();
        self.attrs.push(KeyValue::new(
            Key::new(format!("buck2.{}", key.as_str())),
            value.into(),
        ));
    }

    /// Push an attribute under a key taken verbatim from the OpenTelemetry semantic conventions (no
    /// `buck2.` prefix).
    fn push_std(&mut self, key: impl Into<Key>, value: impl Into<Value>) {
        self.attrs.push(KeyValue::new(key, value.into()));
    }

    /// Resolve the accumulated attributes into the final list, deduplicating by key with last-write
    /// wins. OTLP attributes are keyed by name, so a duplicate key is invalid; the dynamic
    /// key-value maps (`metadata`, `client_metadata`, `install_device_metadata`) can in principle
    /// produce one (e.g. two install devices sharing an entry key), and overwriting keeps the event
    /// usable rather than emitting an ambiguous pair. First-seen position is preserved.
    fn into_attrs(self) -> Vec<KeyValue> {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut deduped: Vec<KeyValue> = Vec::with_capacity(self.attrs.len());
        for kv in self.attrs {
            match index.get(kv.key.as_str()) {
                Some(&i) => deduped[i] = kv,
                None => {
                    index.insert(kv.key.as_str().to_owned(), deduped.len());
                    deduped.push(kv);
                }
            }
        }
        deduped
    }

    fn string(&mut self, key: impl Into<Key>, v: impl Into<String>) {
        self.push(key, v.into());
    }

    fn opt_string(&mut self, key: impl Into<Key>, v: Option<impl Into<String>>) {
        if let Some(v) = v {
            self.string(key, v);
        }
    }

    /// As [`Attrs::opt_string`], but emitted under a verbatim semantic-convention key.
    fn opt_string_std(&mut self, key: impl Into<Key>, v: Option<impl Into<String>>) {
        if let Some(v) = v {
            self.push_std(key, v.into());
        }
    }

    /// Integer-valued attribute. OTLP only has `i64`, so anything that does not fit (a `u64` past
    /// `i64::MAX`) is dropped rather than silently wrapping; in practice these are counts/sizes that
    /// never approach the limit.
    fn int(&mut self, key: impl Into<Key>, v: impl TryInto<i64>) {
        if let Ok(v) = v.try_into() {
            self.push(key, v);
        }
    }

    fn opt_int<T: TryInto<i64>>(&mut self, key: impl Into<Key>, v: Option<T>) {
        if let Some(v) = v {
            self.int(key, v);
        }
    }

    /// As [`Attrs::opt_int`], but emitted under a verbatim semantic-convention key.
    fn opt_int_std<T: TryInto<i64>>(&mut self, key: impl Into<Key>, v: Option<T>) {
        if let Some(v) = v
            && let Ok(v) = v.try_into()
        {
            self.push_std(key, v);
        }
    }

    fn float(&mut self, key: impl Into<Key>, v: f64) {
        self.push(key, v);
    }

    fn opt_float(&mut self, key: impl Into<Key>, v: Option<f64>) {
        if let Some(v) = v {
            self.float(key, v);
        }
    }

    fn bool(&mut self, key: impl Into<Key>, v: bool) {
        self.push(key, v);
    }

    fn opt_bool(&mut self, key: impl Into<Key>, v: Option<bool>) {
        if let Some(v) = v {
            self.bool(key, v);
        }
    }

    /// A `google.protobuf.Duration` as integer microseconds, matching the `_us` serde convention.
    fn opt_duration_us(&mut self, key: impl Into<Key>, v: Option<&::prost_types::Duration>) {
        if let Some(d) = v {
            let micros = d.seconds * 1_000_000 + i64::from(d.nanos) / 1_000;
            self.int(key, micros);
        }
    }

    /// A repeated scalar field as a single homogeneous string array. Skipped when empty (an empty
    /// array carries no information and just adds a column).
    fn strings<I, S>(&mut self, key: impl Into<Key>, vs: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let vs: Vec<StringValue> = vs
            .into_iter()
            .map(|s| StringValue::from(s.into()))
            .collect();
        if !vs.is_empty() {
            self.push(key, Value::Array(Array::String(vs)));
        }
    }

    /// As [`Attrs::strings`], but emitted under a verbatim semantic-convention key.
    fn strings_std<I, S>(&mut self, key: impl Into<Key>, vs: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let vs: Vec<StringValue> = vs
            .into_iter()
            .map(|s| StringValue::from(s.into()))
            .collect();
        if !vs.is_empty() {
            self.push_std(key, Value::Array(Array::String(vs)));
        }
    }

    /// A repeated bool field as a single homogeneous bool array. Skipped when empty.
    fn bools<I>(&mut self, key: impl Into<Key>, vs: I)
    where
        I: IntoIterator<Item = bool>,
    {
        let vs: Vec<bool> = vs.into_iter().collect();
        if !vs.is_empty() {
            self.push(key, Value::Array(Array::Bool(vs)));
        }
    }
}

/// Map every field of an [`InvocationRecord`](buck2_data::InvocationRecord) to OTLP span attributes.
pub(crate) fn invocation_record_attributes(record: &buck2_data::InvocationRecord) -> Vec<KeyValue> {
    let mut a = Attrs::default();

    // Marker so backends can segment this wide event from buck2's fine-grained action/analysis
    // spans (e.g. into a dedicated Honeycomb dataset/view).
    a.string("event_type", "invocation_record");

    // -- Identity / command ------------------------------------------------------------------
    a.string("re_session_id", record.re_session_id.clone());
    // `cli_args` is the full argv as received by the process -- OTel `process.command_args`.
    a.strings_std(PROCESS_COMMAND_ARGS, record.cli_args.iter().cloned());
    a.string("filesystem", record.filesystem.clone());
    a.opt_string("repo_path", record.repo_path.clone());
    a.opt_string("isolation_dir", record.isolation_dir.clone());
    a.opt_string("command_name", record.command_name.clone());
    a.opt_string("test_info", record.test_info.clone());
    a.strings("tags", record.tags.iter().cloned());
    a.string("re_experiment_name", record.re_experiment_name.clone());
    a.opt_string("restarted_trace_id", record.restarted_trace_id.clone());
    a.strings(
        "concurrent_command_ids",
        record.concurrent_command_ids.iter().cloned(),
    );
    a.opt_string("preemptible", record.preemptible.clone());
    a.opt_string(
        "previous_uuid_with_mismatched_config",
        record.previous_uuid_with_mismatched_config.clone(),
    );

    // -- Durations (microseconds) ------------------------------------------------------------
    a.opt_duration_us("command_duration_us", record.command_duration.as_ref());
    a.opt_duration_us("client_walltime_us", record.client_walltime.as_ref());
    a.opt_duration_us(
        "critical_path_duration_us",
        record.critical_path_duration.as_ref(),
    );
    a.opt_duration_us(
        "concurrent_command_blocking_duration_us",
        record.concurrent_command_blocking_duration.as_ref(),
    );
    a.opt_duration_us(
        "bxl_ensure_artifacts_duration_us",
        record.bxl_ensure_artifacts_duration.as_ref(),
    );
    a.opt_duration_us("install_duration_us", record.install_duration.as_ref());

    // -- Action execution counts -------------------------------------------------------------
    a.int("run_local_count", record.run_local_count);
    a.int("run_remote_count", record.run_remote_count);
    a.int("run_action_cache_count", record.run_action_cache_count);
    a.int("run_skipped_count", record.run_skipped_count);
    a.opt_int("run_fallback_count", record.run_fallback_count);
    a.opt_int(
        "run_fallback_re_queue_count",
        record.run_fallback_re_queue_count,
    );
    a.opt_int("run_local_only_count", record.run_local_only_count);
    a.opt_int(
        "local_actions_executed_via_worker",
        record.local_actions_executed_via_worker,
    );
    a.int(
        "run_remote_dep_file_cache_count",
        record.run_remote_dep_file_cache_count,
    );
    a.opt_int(
        "run_command_failure_count",
        record.run_command_failure_count,
    );

    // -- Cache / uploads ---------------------------------------------------------------------
    a.int("cache_upload_count", record.cache_upload_count);
    a.int(
        "cache_upload_attempt_count",
        record.cache_upload_attempt_count,
    );
    a.int("dep_file_upload_count", record.dep_file_upload_count);
    a.int(
        "dep_file_upload_attempt_count",
        record.dep_file_upload_attempt_count,
    );
    a.float("cache_hit_rate", f64::from(record.cache_hit_rate));
    a.int(
        "min_build_count_since_rebase",
        record.min_build_count_since_rebase,
    );
    a.int(
        "min_attempted_build_count_since_rebase",
        record.min_attempted_build_count_since_rebase,
    );

    // -- Build graph / analysis --------------------------------------------------------------
    a.opt_int("analysis_count", record.analysis_count);
    a.opt_int("load_count", record.load_count);
    a.opt_int("event_count", record.event_count);
    a.opt_int(
        "materialization_output_size",
        record.materialization_output_size,
    );
    a.opt_int("materialization_files", record.materialization_files);
    a.strings(
        "target_rule_type_names",
        record.target_rule_type_names.iter().cloned(),
    );

    // -- "Time to X" latency milestones (milliseconds) ---------------------------------------
    a.opt_int(
        "max_event_client_delay_ms",
        record.max_event_client_delay_ms,
    );
    a.opt_int(
        "time_to_first_action_execution_ms",
        record.time_to_first_action_execution_ms,
    );
    a.opt_int("time_to_command_start_ms", record.time_to_command_start_ms);
    a.opt_int(
        "time_to_command_critical_section_ms",
        record.time_to_command_critical_section_ms,
    );
    a.opt_int(
        "time_to_first_analysis_ms",
        record.time_to_first_analysis_ms,
    );
    a.opt_int(
        "time_to_load_first_build_file_ms",
        record.time_to_load_first_build_file_ms,
    );
    a.opt_int(
        "time_to_first_command_execution_start_ms",
        record.time_to_first_command_execution_start_ms,
    );
    a.opt_int(
        "time_to_last_action_execution_end_ms",
        record.time_to_last_action_execution_end_ms,
    );
    a.opt_int(
        "time_to_first_test_discovery_ms",
        record.time_to_first_test_discovery_ms,
    );
    a.opt_int(
        "time_to_first_test_run_ms",
        record.time_to_first_test_run_ms,
    );
    a.opt_int(
        "time_to_first_pass_test_result_ms",
        record.time_to_first_pass_test_result_ms,
    );
    a.opt_int(
        "time_to_first_fail_test_result_ms",
        record.time_to_first_fail_test_result_ms,
    );
    a.opt_int(
        "time_to_first_fatal_test_result_ms",
        record.time_to_first_fatal_test_result_ms,
    );
    a.opt_int(
        "time_to_first_timeout_test_result_ms",
        record.time_to_first_timeout_test_result_ms,
    );
    a.opt_int(
        "time_to_first_skip_test_result_ms",
        record.time_to_first_skip_test_result_ms,
    );
    a.opt_int(
        "time_to_first_unknown_test_result_ms",
        record.time_to_first_unknown_test_result_ms,
    );
    a.opt_int(
        "time_to_first_infra_failure_test_result_ms",
        record.time_to_first_infra_failure_test_result_ms,
    );
    a.int("exec_time_ms", record.exec_time_ms);

    // -- Memory / system ---------------------------------------------------------------------
    a.opt_int("max_malloc_bytes_active", record.max_malloc_bytes_active);
    a.opt_int(
        "max_malloc_bytes_allocated",
        record.max_malloc_bytes_allocated,
    );
    a.opt_int(
        "system_total_memory_bytes",
        record.system_total_memory_bytes,
    );
    a.opt_int(
        "peak_process_memory_bytes",
        record.peak_process_memory_bytes,
    );
    a.opt_int(
        "peak_used_disk_space_bytes",
        record.peak_used_disk_space_bytes,
    );
    a.opt_int("total_disk_space_bytes", record.total_disk_space_bytes);
    a.opt_int("memory_max_anon_allprocs", record.memory_max_anon_allprocs);
    a.opt_int(
        "memory_max_anon_forkserver_actions",
        record.memory_max_anon_forkserver_actions,
    );
    a.opt_int(
        "memory_max_total_allprocs",
        record.memory_max_total_allprocs,
    );
    a.opt_int(
        "memory_max_total_forkserver_actions",
        record.memory_max_total_forkserver_actions,
    );
    // Peak swap / memory-pressure (PSI) over the invocation, from the allprocs cgroup. These are
    // non-optional scalars (0 when unavailable, e.g. off-Linux), so they always emit.
    a.int(
        "memory_max_swap_bytes_allprocs",
        record.memory_max_swap_bytes_allprocs,
    );
    a.float(
        "memory_max_pressure_10s_avg_allprocs",
        record.memory_max_pressure_10s_avg_allprocs,
    );
    a.float(
        "memory_max_pressure_60s_avg_allprocs",
        record.memory_max_pressure_60s_avg_allprocs,
    );
    // Peak normalized (per-core) system load averages over the invocation.
    a.opt_float(
        "peak_normalized_system_load1",
        record.peak_normalized_system_load1,
    );
    a.opt_float(
        "peak_normalized_system_load5",
        record.peak_normalized_system_load5,
    );

    // -- DICE / in-progress peaks ------------------------------------------------------------
    a.opt_int(
        "max_dice_in_progress_keys",
        record.max_dice_in_progress_keys,
    );
    a.opt_int("max_dice_compute_keys", record.max_dice_compute_keys);
    a.opt_int("max_in_progress_actions", record.max_in_progress_actions);
    a.opt_int(
        "max_in_progress_local_actions",
        record.max_in_progress_local_actions,
    );
    a.opt_int(
        "max_in_progress_remote_actions",
        record.max_in_progress_remote_actions,
    );
    a.opt_int(
        "max_in_progress_remote_uploads",
        record.max_in_progress_remote_uploads,
    );

    // -- DICE page-in (deserialize-on-demand) overhead ---------------------------------------
    // Aggregate page-in cost for the invocation (delta between first and last snapshot).
    a.opt_int("page_in_count", record.page_in_count);
    a.opt_int("page_in_fetch_us", record.page_in_fetch_us);
    a.opt_int("page_in_deser_us", record.page_in_deser_us);
    a.opt_int("page_in_bytes", record.page_in_bytes);
    // Per-DICE-key-type breakdown: each key type is a dynamic map key, emitted as its own nested
    // attributes (like `network_interface_stats`). Key types with no page-ins are omitted upstream.
    for (key_type, stats) in &record.page_in_by_key_type {
        a.int(format!("page_in_by_key_type.{key_type}.count"), stats.count);
        a.int(
            format!("page_in_by_key_type.{key_type}.fetch_us"),
            stats.fetch_us,
        );
        a.int(
            format!("page_in_by_key_type.{key_type}.deser_us"),
            stats.deser_us,
        );
        a.int(format!("page_in_by_key_type.{key_type}.bytes"), stats.bytes);
    }

    // -- Remote execution bytes / speeds -----------------------------------------------------
    a.opt_int("re_upload_bytes", record.re_upload_bytes);
    a.opt_int("re_download_bytes", record.re_download_bytes);
    a.opt_int("re_max_download_speed", record.re_max_download_speed);
    a.opt_int("re_max_upload_speed", record.re_max_upload_speed);
    a.opt_int("re_avg_download_speed", record.re_avg_download_speed);
    a.opt_int("re_avg_upload_speed", record.re_avg_upload_speed);
    a.opt_float(
        "re_average_local_cache_lookup_microseconds",
        record.re_average_local_cache_lookup_microseconds,
    );

    // -- Local cache -------------------------------------------------------------------------
    a.opt_int("local_cache_hits_files", record.local_cache_hits_files);
    a.opt_int("local_cache_hits_bytes", record.local_cache_hits_bytes);
    a.opt_int("local_cache_misses_files", record.local_cache_misses_files);
    a.opt_int("local_cache_misses_bytes", record.local_cache_misses_bytes);
    a.opt_int(
        "local_cache_hits_files_from_memory_cache",
        record.local_cache_hits_files_from_memory_cache,
    );
    a.opt_int(
        "local_cache_hits_files_from_filesystem_cache",
        record.local_cache_hits_files_from_filesystem_cache,
    );
    a.opt_int("local_cache_lookups", record.local_cache_lookups);

    // -- Storage backends (zdb / zgateway / manifold / hedwig) -------------------------------
    a.opt_int("zdb_download_queries", record.zdb_download_queries);
    a.opt_int("zdb_download_bytes", record.zdb_download_bytes);
    a.opt_int("zdb_upload_queries", record.zdb_upload_queries);
    a.opt_int("zdb_upload_bytes", record.zdb_upload_bytes);
    a.opt_int(
        "zgateway_download_queries",
        record.zgateway_download_queries,
    );
    a.opt_int("zgateway_download_bytes", record.zgateway_download_bytes);
    a.opt_int("zgateway_upload_queries", record.zgateway_upload_queries);
    a.opt_int("zgateway_upload_bytes", record.zgateway_upload_bytes);
    a.opt_int(
        "manifold_download_queries",
        record.manifold_download_queries,
    );
    a.opt_int("manifold_download_bytes", record.manifold_download_bytes);
    a.opt_int("manifold_upload_queries", record.manifold_upload_queries);
    a.opt_int("manifold_upload_bytes", record.manifold_upload_bytes);
    a.opt_int("hedwig_download_queries", record.hedwig_download_queries);
    a.opt_int("hedwig_download_bytes", record.hedwig_download_bytes);
    a.opt_int("hedwig_upload_queries", record.hedwig_upload_queries);
    a.opt_int("hedwig_upload_bytes", record.hedwig_upload_bytes);

    // -- Event sink --------------------------------------------------------------------------
    a.opt_int("sink_success_count", record.sink_success_count);
    a.opt_int("sink_failure_count", record.sink_failure_count);
    a.opt_int("sink_dropped_count", record.sink_dropped_count);
    a.opt_int("sink_bytes_written", record.sink_bytes_written);
    a.opt_int("sink_max_buffer_depth", record.sink_max_buffer_depth);

    // -- IO syscall counts -------------------------------------------------------------------
    a.opt_int("io_copy_count", record.io_copy_count);
    a.opt_int("io_symlink_count", record.io_symlink_count);
    a.opt_int("io_hardlink_count", record.io_hardlink_count);
    a.opt_int("io_mkdir_count", record.io_mkdir_count);
    a.opt_int("io_readdir_count", record.io_readdir_count);
    a.opt_int("io_readdir_eden_count", record.io_readdir_eden_count);
    a.opt_int("io_rmdir_count", record.io_rmdir_count);
    a.opt_int("io_rmdir_all_count", record.io_rmdir_all_count);
    a.opt_int("io_stat_count", record.io_stat_count);
    a.opt_int("io_stat_eden_count", record.io_stat_eden_count);
    a.opt_int("io_chmod_count", record.io_chmod_count);
    a.opt_int("io_readlink_count", record.io_readlink_count);
    a.opt_int("io_remove_count", record.io_remove_count);
    a.opt_int("io_rename_count", record.io_rename_count);
    a.opt_int("io_read_count", record.io_read_count);
    a.opt_int("io_write_count", record.io_write_count);
    a.opt_int("io_canonicalize_count", record.io_canonicalize_count);
    a.opt_int("io_eden_settle_count", record.io_eden_settle_count);

    // -- Versions / environment --------------------------------------------------------------
    a.opt_string("watchman_version", record.watchman_version.clone());
    a.opt_string("eden_version", record.eden_version.clone());
    a.opt_string("file_watcher", record.file_watcher.clone());
    a.opt_string(
        "persistent_cache_mode",
        record.persistent_cache_mode.clone(),
    );
    a.opt_int("file_watcher_duration_ms", record.file_watcher_duration_ms);
    a.opt_int(
        "initial_materializer_entries_from_sqlite",
        record.initial_materializer_entries_from_sqlite,
    );

    // -- Source control ----------------------------------------------------------------------
    // A repo is either hg or git, so at most one of these is set; both map to the same OTel
    // `vcs.ref.head.revision` (the current checkout's revision).
    a.opt_string_std(VCS_REF_HEAD_REVISION, record.hg_revision.clone());
    a.opt_string_std(VCS_REF_HEAD_REVISION, record.git_revision.clone());
    a.opt_bool("has_local_changes", record.has_local_changes);
    a.strings(
        "version_control_errors",
        record.version_control_errors.iter().cloned(),
    );
    a.strings(
        "representative_config_flags",
        record.representative_config_flags.iter().cloned(),
    );

    // -- Outcome / status --------------------------------------------------------------------
    // The process exit status -- OTel `process.exit.code`.
    a.opt_int_std(PROCESS_EXIT_CODE, record.exit_code);
    a.opt_string("exit_result_name", record.exit_result_name.clone());
    a.opt_bool("has_command_result", record.has_command_result);
    a.opt_bool("has_end_of_stream", record.has_end_of_stream);
    a.opt_bool(
        "instant_command_is_success",
        record.instant_command_is_success,
    );
    a.opt_bool(
        "daemon_connection_failure",
        record.daemon_connection_failure,
    );
    a.opt_bool("should_restart", record.should_restart);
    a.opt_bool("eligible_for_full_hybrid", record.eligible_for_full_hybrid);
    a.opt_bool("new_configs_used", record.new_configs_used);
    a.opt_string(
        "critical_path_backend",
        record.critical_path_backend.clone(),
    );
    a.opt_int(
        "compressed_event_log_size_bytes",
        record.compressed_event_log_size_bytes,
    );
    a.opt_int("event_log_manifold_ttl_s", record.event_log_manifold_ttl_s);
    a.opt_int("wrapper_start_time", record.wrapper_start_time);
    a.opt_string("installer_log_url", record.installer_log_url.clone());

    // -- Enums (stored as i32; emit the proto enum name) -------------------------------------
    if let Some(outcome) = record
        .outcome
        .and_then(|v| buck2_data::InvocationOutcome::try_from(v).ok())
    {
        a.string("outcome", outcome.as_str_name());
    }
    if let Some(reason) = record
        .daemon_was_started
        .and_then(|v| buck2_data::DaemonWasStartedReason::try_from(v).ok())
    {
        a.string("daemon_was_started", reason.as_str_name());
    }
    a.strings(
        "active_networks_kinds",
        record
            .active_networks_kinds
            .iter()
            .filter_map(|&v| buck2_data::NetworkKind::try_from(v).ok())
            .map(|k| k.as_str_name()),
    );

    // -- Resolved target patterns (unwrap the single-field `TargetPattern` wrapper) ----------
    if let Some(patterns) = &record.parsed_target_patterns {
        a.strings(
            "parsed_target_patterns",
            patterns.target_patterns.iter().map(|p| p.value.clone()),
        );
    }

    // -- Dynamic key-value maps: each entry's key becomes its own attribute key ---------------
    // `metadata` is the explicit Scuba passthrough; `client_metadata` and `install_device_metadata`
    // are likewise lists of (key, value) pairs, so we emit `<map>.<entry_key>` rather than two
    // parallel `.key`/`.value` arrays (which can't be correlated by a backend). These keys are
    // dynamic and not guaranteed unique across entries, so the final dedup pass (last-wins) resolves
    // any collisions.
    if let Some(metadata) = &record.metadata {
        for (k, v) in &metadata.strings {
            a.push(format!("metadata.{k}"), v.clone());
        }
        for (k, v) in &metadata.ints {
            a.push(format!("metadata.{k}"), *v);
        }
    }
    for m in &record.client_metadata {
        a.push(format!("client_metadata.{}", m.key), m.value.clone());
    }
    for entry in record
        .install_device_metadata
        .iter()
        .flat_map(|d| d.entry.iter())
    {
        a.push(
            format!("install_device_metadata.{}", entry.key),
            entry.value.clone(),
        );
    }
    // `--agent-context` entries: likewise a (key, value) list emitted as `agent_context.<key>`.
    for entry in &record.agent_context {
        a.push(
            format!("agent_context.{}", entry.key),
            entry.value.clone(),
        );
    }

    // -- Repeated messages projected field-wise into parallel arrays -------------------------
    a.strings(
        "soft_error_categories.category",
        record
            .soft_error_categories
            .iter()
            .map(|e| e.category.clone()),
    );
    a.bools(
        "soft_error_categories.is_quiet",
        record.soft_error_categories.iter().map(|e| e.is_quiet),
    );

    a.strings(
        "errors.message",
        record.errors.iter().map(|e| e.message.clone()),
    );
    a.strings(
        "errors.category",
        record.errors.iter().filter_map(|e| e.category.clone()),
    );
    a.strings(
        "errors.category_key",
        record.errors.iter().filter_map(|e| e.category_key.clone()),
    );
    a.strings(
        "errors.best_tag",
        record.errors.iter().filter_map(|e| e.best_tag.clone()),
    );
    a.strings(
        "errors.source_area",
        record.errors.iter().filter_map(|e| e.source_area.clone()),
    );
    a.strings(
        "errors.source_location",
        record
            .errors
            .iter()
            .filter_map(|e| e.source_location.clone()),
    );
    a.strings(
        "errors.telemetry_message",
        record
            .errors
            .iter()
            .filter_map(|e| e.telemetry_message.clone()),
    );
    a.strings(
        "errors.tags",
        record.errors.iter().flat_map(|e| e.tags.iter().cloned()),
    );
    a.strings(
        "errors.sub_error_categories",
        record
            .errors
            .iter()
            .flat_map(|e| e.sub_error_categories.iter().cloned()),
    );

    // -- Nested sub-messages -----------------------------------------------------------------
    if let Some(snapshot) = &record.first_snapshot {
        push_snapshot(&mut a, "first_snapshot", snapshot);
    }
    if let Some(snapshot) = &record.last_snapshot {
        push_snapshot(&mut a, "last_snapshot", snapshot);
    }
    if let Some(command_end) = &record.command_end {
        push_command_end(&mut a, command_end);
    }
    if let Some(stats) = &record.file_watcher_stats {
        push_file_watcher_stats(&mut a, stats);
    }
    if let Some(options) = &record.command_options {
        push_command_options(&mut a, options);
    }
    if let Some(target_cfg) = &record.target_cfg {
        push_target_cfg(&mut a, target_cfg);
    }
    if let Some(revision) = &record.version_control_revision {
        push_version_control_revision(&mut a, revision);
    }

    a.into_attrs()
}

fn push_command_options(a: &mut Attrs, o: &buck2_data::CommandOptions) {
    a.int(
        "command_options.configured_parallelism",
        o.configured_parallelism,
    );
    a.int(
        "command_options.available_parallelism",
        o.available_parallelism,
    );
}

fn push_target_cfg(a: &mut Attrs, t: &buck2_data::TargetCfg) {
    a.strings(
        "target_cfg.target_platforms",
        t.target_platforms.iter().cloned(),
    );
    a.strings("target_cfg.cli_modifiers", t.cli_modifiers.iter().cloned());
}

fn push_version_control_revision(a: &mut Attrs, r: &buck2_data::VersionControlRevision) {
    a.opt_string(
        "version_control_revision.hg_revision",
        r.hg_revision.clone(),
    );
    a.opt_string(
        "version_control_revision.git_revision",
        r.git_revision.clone(),
    );
    a.opt_bool(
        "version_control_revision.has_local_changes",
        r.has_local_changes,
    );
    a.opt_string(
        "version_control_revision.command_error",
        r.command_error.clone(),
    );
}

fn push_file_watcher_stats(a: &mut Attrs, s: &buck2_data::FileWatcherStats) {
    a.bool("file_watcher_stats.fresh_instance", s.fresh_instance);
    a.int("file_watcher_stats.events_total", s.events_total);
    a.int("file_watcher_stats.events_processed", s.events_processed);
    // The mergebase the working copy branched from -- OTel `vcs.ref.base.revision`.
    a.opt_string_std(VCS_REF_BASE_REVISION, s.branched_from_revision.clone());
    a.opt_int(
        "file_watcher_stats.branched_from_global_rev",
        s.branched_from_global_rev,
    );
    a.opt_string(
        "file_watcher_stats.incomplete_events_reason",
        s.incomplete_events_reason.clone(),
    );
    a.opt_string(
        "file_watcher_stats.watchman_version",
        s.watchman_version.clone(),
    );
    a.opt_int(
        "file_watcher_stats.branched_from_revision_timestamp",
        s.branched_from_revision_timestamp,
    );
    a.opt_string("file_watcher_stats.eden_version", s.eden_version.clone());

    // `events` is a repeated message: project field-wise into parallel arrays.
    a.strings(
        "file_watcher_stats.events.path",
        s.events.iter().map(|e| e.path.clone()),
    );
    a.strings(
        "file_watcher_stats.events.event",
        s.events
            .iter()
            .filter_map(|e| buck2_data::FileWatcherEventType::try_from(e.event).ok())
            .map(|e| e.as_str_name()),
    );
    a.strings(
        "file_watcher_stats.events.kind",
        s.events
            .iter()
            .filter_map(|e| buck2_data::FileWatcherKind::try_from(e.kind).ok())
            .map(|e| e.as_str_name()),
    );

    if let Some(fresh) = &s.fresh_instance_data {
        a.bool(
            "file_watcher_stats.fresh_instance_data.new_mergebase",
            fresh.new_mergebase,
        );
        a.bool(
            "file_watcher_stats.fresh_instance_data.cleared_dice",
            fresh.cleared_dice,
        );
        a.bool(
            "file_watcher_stats.fresh_instance_data.cleared_dep_files",
            fresh.cleared_dep_files,
        );
    }
}

fn push_snapshot(a: &mut Attrs, prefix: &str, s: &buck2_data::Snapshot) {
    let k = |n: &str| format!("{prefix}.{n}");

    a.opt_int(k("buck2_rss"), s.buck2_rss);
    a.int(k("buck2_max_rss"), s.buck2_max_rss);
    a.int(k("buck2_user_cpu_us"), s.buck2_user_cpu_us);
    a.int(k("buck2_system_cpu_us"), s.buck2_system_cpu_us);
    a.int(
        k("blocking_executor_io_queue_size"),
        s.blocking_executor_io_queue_size,
    );
    a.int(
        k("tokio_blocking_queue_depth"),
        s.tokio_blocking_queue_depth,
    );
    a.int(
        k("tokio_num_idle_blocking_threads"),
        s.tokio_num_idle_blocking_threads,
    );
    a.int(
        k("tokio_num_blocking_threads"),
        s.tokio_num_blocking_threads,
    );

    a.int(k("re_download_bytes"), s.re_download_bytes);
    a.int(k("re_upload_bytes"), s.re_upload_bytes);
    a.int(k("re_uploads_started"), s.re_uploads_started);
    a.int(
        k("re_uploads_finished_successfully"),
        s.re_uploads_finished_successfully,
    );
    a.int(
        k("re_uploads_finished_with_error"),
        s.re_uploads_finished_with_error,
    );
    a.int(k("re_downloads_started"), s.re_downloads_started);
    a.int(
        k("re_downloads_finished_successfully"),
        s.re_downloads_finished_successfully,
    );
    a.int(
        k("re_downloads_finished_with_error"),
        s.re_downloads_finished_with_error,
    );
    a.int(k("re_action_cache_started"), s.re_action_cache_started);
    a.int(
        k("re_action_cache_finished_successfully"),
        s.re_action_cache_finished_successfully,
    );
    a.int(
        k("re_action_cache_finished_with_error"),
        s.re_action_cache_finished_with_error,
    );
    a.int(k("re_executes_started"), s.re_executes_started);
    a.int(
        k("re_executes_finished_successfully"),
        s.re_executes_finished_successfully,
    );
    a.int(
        k("re_executes_finished_with_error"),
        s.re_executes_finished_with_error,
    );
    a.int(k("re_materializes_started"), s.re_materializes_started);
    a.int(
        k("re_materializes_finished_successfully"),
        s.re_materializes_finished_successfully,
    );
    a.int(
        k("re_materializes_finished_with_error"),
        s.re_materializes_finished_with_error,
    );
    a.int(
        k("re_write_action_results_started"),
        s.re_write_action_results_started,
    );
    a.int(
        k("re_write_action_results_finished_successfully"),
        s.re_write_action_results_finished_successfully,
    );
    a.int(
        k("re_write_action_results_finished_with_error"),
        s.re_write_action_results_finished_with_error,
    );
    a.int(
        k("re_get_digest_expirations_started"),
        s.re_get_digest_expirations_started,
    );
    a.int(
        k("re_get_digest_expirations_finished_successfully"),
        s.re_get_digest_expirations_finished_successfully,
    );
    a.int(
        k("re_get_digest_expirations_finished_with_error"),
        s.re_get_digest_expirations_finished_with_error,
    );

    a.int(k("io_in_flight_copy"), s.io_in_flight_copy);
    a.int(k("io_in_flight_symlink"), s.io_in_flight_symlink);
    a.int(k("io_in_flight_hardlink"), s.io_in_flight_hardlink);
    a.int(k("io_in_flight_mk_dir"), s.io_in_flight_mk_dir);
    a.int(k("io_in_flight_read_dir"), s.io_in_flight_read_dir);
    a.int(
        k("io_in_flight_read_dir_eden"),
        s.io_in_flight_read_dir_eden,
    );
    a.int(k("io_in_flight_rm_dir"), s.io_in_flight_rm_dir);
    a.int(k("io_in_flight_rm_dir_all"), s.io_in_flight_rm_dir_all);
    a.int(k("io_in_flight_stat"), s.io_in_flight_stat);
    a.int(k("io_in_flight_stat_eden"), s.io_in_flight_stat_eden);
    a.int(k("io_in_flight_chmod"), s.io_in_flight_chmod);
    a.int(k("io_in_flight_read_link"), s.io_in_flight_read_link);
    a.int(k("io_in_flight_remove"), s.io_in_flight_remove);
    a.int(k("io_in_flight_rename"), s.io_in_flight_rename);
    a.int(k("io_in_flight_read"), s.io_in_flight_read);
    a.int(k("io_in_flight_write"), s.io_in_flight_write);
    a.int(k("io_in_flight_canonicalize"), s.io_in_flight_canonicalize);
    a.int(k("io_in_flight_eden_settle"), s.io_in_flight_eden_settle);

    a.int(k("daemon_uptime_s"), s.daemon_uptime_s);

    a.opt_int(k("malloc_bytes_active"), s.malloc_bytes_active);
    a.opt_int(k("malloc_bytes_allocated"), s.malloc_bytes_allocated);
    a.opt_int(k("used_disk_space_bytes"), s.used_disk_space_bytes);

    a.opt_int(k("host_cpu_usage_system_ms"), s.host_cpu_usage_system_ms);
    a.opt_int(k("host_cpu_usage_user_ms"), s.host_cpu_usage_user_ms);

    a.int(k("dice_key_count"), s.dice_key_count);
    a.int(
        k("dice_currently_active_key_count"),
        s.dice_currently_active_key_count,
    );
    a.int(
        k("dice_active_transaction_count"),
        s.dice_active_transaction_count,
    );

    a.int(
        k("deferred_materializer_queue_size"),
        s.deferred_materializer_queue_size,
    );

    a.opt_int(k("sink_successes"), s.sink_successes);
    a.opt_int(k("sink_failures"), s.sink_failures);
    a.opt_int(
        k("sink_failures_invalid_request"),
        s.sink_failures_invalid_request,
    );
    a.opt_int(
        k("sink_failures_unauthorized"),
        s.sink_failures_unauthorized,
    );
    a.opt_int(
        k("sink_failures_rate_limited"),
        s.sink_failures_rate_limited,
    );
    a.opt_int(k("sink_failures_pushed_back"), s.sink_failures_pushed_back);
    a.opt_int(
        k("sink_failures_enqueue_failed"),
        s.sink_failures_enqueue_failed,
    );
    a.opt_int(
        k("sink_failures_internal_error"),
        s.sink_failures_internal_error,
    );
    a.opt_int(k("sink_failures_timed_out"), s.sink_failures_timed_out);
    a.opt_int(k("sink_failures_unknown"), s.sink_failures_unknown);
    a.opt_int(k("sink_buffer_depth"), s.sink_buffer_depth);
    a.opt_int(k("sink_dropped"), s.sink_dropped);
    a.opt_int(k("sink_bytes_written"), s.sink_bytes_written);

    // `network_interface_stats` is a map keyed by interface name; emit each interface's counters
    // under its own dynamic key (like `metadata`).
    for (interface, stats) in &s.network_interface_stats {
        a.int(
            format!("{prefix}.network_interface_stats.{interface}.tx_bytes"),
            stats.tx_bytes,
        );
        a.int(
            format!("{prefix}.network_interface_stats.{interface}.rx_bytes"),
            stats.rx_bytes,
        );
        if let Ok(network_kind) = buck2_data::NetworkKind::try_from(stats.network_kind) {
            a.string(
                format!("{prefix}.network_interface_stats.{interface}.network_kind"),
                network_kind.as_str_name(),
            );
        }
    }

    a.int(k("http_download_bytes"), s.http_download_bytes);

    a.int(
        k("deferred_materializer_declares"),
        s.deferred_materializer_declares,
    );
    a.int(
        k("deferred_materializer_declares_reused"),
        s.deferred_materializer_declares_reused,
    );

    if let Some(unix) = &s.unix_system_stats {
        a.float(k("unix_system_stats.load1"), unix.load1);
        a.float(k("unix_system_stats.load5"), unix.load5);
        a.float(k("unix_system_stats.load15"), unix.load15);
    }

    a.int(k("zdb_download_queries"), s.zdb_download_queries);
    a.int(k("zdb_download_bytes"), s.zdb_download_bytes);
    a.int(k("zdb_upload_queries"), s.zdb_upload_queries);
    a.int(k("zdb_upload_bytes"), s.zdb_upload_bytes);

    a.int(k("zgateway_download_queries"), s.zgateway_download_queries);
    a.int(k("zgateway_download_bytes"), s.zgateway_download_bytes);
    a.int(k("zgateway_upload_queries"), s.zgateway_upload_queries);
    a.int(k("zgateway_upload_bytes"), s.zgateway_upload_bytes);

    a.int(k("manifold_download_queries"), s.manifold_download_queries);
    a.int(k("manifold_download_bytes"), s.manifold_download_bytes);
    a.int(k("manifold_upload_queries"), s.manifold_upload_queries);
    a.int(k("manifold_upload_bytes"), s.manifold_upload_bytes);

    a.int(k("hedwig_download_queries"), s.hedwig_download_queries);
    a.int(k("hedwig_download_bytes"), s.hedwig_download_bytes);
    a.int(k("hedwig_upload_queries"), s.hedwig_upload_queries);
    a.int(k("hedwig_upload_bytes"), s.hedwig_upload_bytes);

    a.int(k("local_cache_hits_files"), s.local_cache_hits_files);
    a.int(k("local_cache_hits_bytes"), s.local_cache_hits_bytes);
    a.int(k("local_cache_misses_files"), s.local_cache_misses_files);
    a.int(k("local_cache_misses_bytes"), s.local_cache_misses_bytes);

    a.int(
        k("local_cache_hits_files_from_memory_cache"),
        s.local_cache_hits_files_from_memory_cache,
    );
    a.int(
        k("local_cache_hits_files_from_filesystem_cache"),
        s.local_cache_hits_files_from_filesystem_cache,
    );

    a.int(k("local_cache_lookups"), s.local_cache_lookups);
    a.int(
        k("local_cache_lookup_latency_microseconds"),
        s.local_cache_lookup_latency_microseconds,
    );

    a.opt_int(
        k("this_event_client_delay_ms"),
        s.this_event_client_delay_ms,
    );
    a.opt_int(k("client_cpu_percents"), s.client_cpu_percents);

    if let Some(cgroup) = &s.allprocs_cgroup {
        push_cgroup_memory_stats(a, &k("allprocs_cgroup"), cgroup);
    }
    if let Some(cgroup) = &s.forkserver_actions_cgroup {
        push_cgroup_memory_stats(a, &k("forkserver_actions_cgroup"), cgroup);
    }

    // Per-DICE-key-type page-in breakdown at this snapshot; dynamic map keyed by key type, emitted
    // as its own nested attributes (like `network_interface_stats`).
    for (key_type, stats) in &s.dice_page_in_by_key_type {
        let p = format!("{prefix}.dice_page_in_by_key_type.{key_type}");
        a.int(format!("{p}.count"), stats.count);
        a.int(format!("{p}.fetch_us"), stats.fetch_us);
        a.int(format!("{p}.deser_us"), stats.deser_us);
        a.int(format!("{p}.bytes"), stats.bytes);
    }

    // Tokio runtime metrics: top-level runtime gauges/deltas only. The per-`Worker` array and the
    // poll-time histogram buckets are intentionally not projected here -- they are high-cardinality
    // and better suited to a dedicated per-worker export than this single wide event.
    if let Some(tokio) = &s.tokio_runtime_stats {
        a.int(k("tokio_runtime_stats.num_alive_tasks"), tokio.num_alive_tasks);
        a.int(
            k("tokio_runtime_stats.global_queue_depth"),
            tokio.global_queue_depth,
        );
        a.int(
            k("tokio_runtime_stats.budget_forced_yield_count"),
            tokio.budget_forced_yield_count,
        );
        a.int(
            k("tokio_runtime_stats.remote_schedule_count"),
            tokio.remote_schedule_count,
        );
    }
}

fn push_cgroup_memory_stats(a: &mut Attrs, prefix: &str, s: &buck2_data::UnixCgroupMemoryStats) {
    a.int(format!("{prefix}.anon"), s.anon);
    a.int(format!("{prefix}.file"), s.file);
    a.int(format!("{prefix}.kernel"), s.kernel);
    a.int(format!("{prefix}.swap_bytes"), s.swap_bytes);
    // Memory pressure (PSI full avg10/avg60 %) from cgroup memory.pressure.
    a.float(
        format!("{prefix}.memory_pressure_10s_avg"),
        s.memory_pressure_10s_avg,
    );
    a.float(
        format!("{prefix}.memory_pressure_60s_avg"),
        s.memory_pressure_60s_avg,
    );
}

fn push_command_end(a: &mut Attrs, c: &buck2_data::CommandEnd) {
    a.bool("command_end.is_success", c.is_success);
    if let Some(build_result) = &c.build_result {
        a.bool(
            "command_end.build_result.build_completed",
            build_result.build_completed,
        );
    }
    match &c.data {
        Some(buck2_data::command_end::Data::Build(e)) => {
            a.string("command_end.kind", "build");
            push_build_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Targets(e)) => {
            a.string("command_end.kind", "targets");
            push_targets_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Query(e)) => {
            a.string("command_end.kind", "query");
            push_query_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Cquery(e)) => {
            a.string("command_end.kind", "cquery");
            push_cquery_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Test(e)) => {
            a.string("command_end.kind", "test");
            push_test_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Audit(e)) => {
            a.string("command_end.kind", "audit");
            push_audit_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Docs(e)) => {
            a.string("command_end.kind", "docs");
            push_docs_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Clean(e)) => {
            a.string("command_end.kind", "clean");
            push_clean_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Aquery(e)) => {
            a.string("command_end.kind", "aquery");
            push_aquery_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Install(e)) => {
            a.string("command_end.kind", "install");
            push_install_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Materialize(e)) => {
            a.string("command_end.kind", "materialize");
            push_materialize_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Profile(e)) => {
            a.string("command_end.kind", "profile");
            push_profile_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Bxl(e)) => {
            a.string("command_end.kind", "bxl");
            push_bxl_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Lsp(e)) => {
            a.string("command_end.kind", "lsp");
            push_lsp_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::FileStatus(e)) => {
            a.string("command_end.kind", "file_status");
            push_file_status_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Starlark(e)) => {
            a.string("command_end.kind", "starlark");
            push_starlark_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Subscribe(e)) => {
            a.string("command_end.kind", "subscribe");
            push_subscription_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Trace(e)) => {
            a.string("command_end.kind", "trace");
            push_trace_io_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Ctargets(e)) => {
            a.string("command_end.kind", "ctargets");
            push_configured_targets_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::StarlarkDebugAttach(e)) => {
            a.string("command_end.kind", "starlark_debug_attach");
            push_starlark_debug_attach_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Explain(e)) => {
            a.string("command_end.kind", "explain");
            push_explain_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::ExpandExternalCell(e)) => {
            a.string("command_end.kind", "expand_external_cell");
            push_expand_external_cells_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Complete(e)) => {
            a.string("command_end.kind", "complete");
            push_complete_command_end(a, e);
        }
        Some(buck2_data::command_end::Data::Hydration(e)) => {
            a.string("command_end.kind", "hydration");
            push_hydration_command_end(a, e);
        }
        None => {}
    }
}

fn push_build_command_end(a: &mut Attrs, e: &buck2_data::BuildCommandEnd) {
    a.strings(
        "command_end.build.unresolved_target_patterns",
        e.unresolved_target_patterns.iter().map(|p| p.value.clone()),
    );
}

fn push_targets_command_end(a: &mut Attrs, e: &buck2_data::TargetsCommandEnd) {
    a.strings(
        "command_end.targets.unresolved_target_patterns",
        e.unresolved_target_patterns.iter().map(|p| p.value.clone()),
    );
}

fn push_query_command_end(_a: &mut Attrs, _e: &buck2_data::QueryCommandEnd) {}

fn push_cquery_command_end(_a: &mut Attrs, _e: &buck2_data::CQueryCommandEnd) {}

fn push_test_command_end(a: &mut Attrs, e: &buck2_data::TestCommandEnd) {
    a.strings(
        "command_end.test.unresolved_target_patterns",
        e.unresolved_target_patterns.iter().map(|p| p.value.clone()),
    );
}

fn push_audit_command_end(_a: &mut Attrs, _e: &buck2_data::AuditCommandEnd) {}

fn push_docs_command_end(_a: &mut Attrs, _e: &buck2_data::DocsCommandEnd) {}

fn push_clean_command_end(a: &mut Attrs, e: &buck2_data::CleanCommandEnd) {
    if let Some(s) = &e.clean_stale_stats {
        a.int(
            "command_end.clean.clean_stale_stats.stale_artifact_count",
            s.stale_artifact_count,
        );
        a.int(
            "command_end.clean.clean_stale_stats.stale_bytes",
            s.stale_bytes,
        );
        a.int(
            "command_end.clean.clean_stale_stats.retained_artifact_count",
            s.retained_artifact_count,
        );
        a.int(
            "command_end.clean.clean_stale_stats.retained_bytes",
            s.retained_bytes,
        );
        a.int(
            "command_end.clean.clean_stale_stats.untracked_artifact_count",
            s.untracked_artifact_count,
        );
        a.int(
            "command_end.clean.clean_stale_stats.untracked_bytes",
            s.untracked_bytes,
        );
        a.int(
            "command_end.clean.clean_stale_stats.cleaned_artifact_count",
            s.cleaned_artifact_count,
        );
        a.int(
            "command_end.clean.clean_stale_stats.cleaned_bytes",
            s.cleaned_bytes,
        );
        a.int(
            "command_end.clean.clean_stale_stats.total_duration_s",
            s.total_duration_s,
        );
        a.int(
            "command_end.clean.clean_stale_stats.scan_duration_s",
            s.scan_duration_s,
        );
        a.int(
            "command_end.clean.clean_stale_stats.clean_duration_s",
            s.clean_duration_s,
        );
    }
}

fn push_aquery_command_end(_a: &mut Attrs, _e: &buck2_data::AqueryCommandEnd) {}

fn push_install_command_end(a: &mut Attrs, e: &buck2_data::InstallCommandEnd) {
    a.strings(
        "command_end.install.unresolved_target_patterns",
        e.unresolved_target_patterns.iter().map(|p| p.value.clone()),
    );
}

fn push_materialize_command_end(_a: &mut Attrs, _e: &buck2_data::MaterializeCommandEnd) {}

fn push_profile_command_end(_a: &mut Attrs, _e: &buck2_data::ProfileCommandEnd) {}

fn push_bxl_command_end(a: &mut Attrs, e: &buck2_data::BxlCommandEnd) {
    a.string("command_end.bxl.bxl_label", e.bxl_label.clone());
}

fn push_lsp_command_end(_a: &mut Attrs, _e: &buck2_data::LspCommandEnd) {}

fn push_file_status_command_end(_a: &mut Attrs, _e: &buck2_data::FileStatusCommandEnd) {}

fn push_starlark_command_end(_a: &mut Attrs, _e: &buck2_data::StarlarkCommandEnd) {}

fn push_subscription_command_end(_a: &mut Attrs, _e: &buck2_data::SubscriptionCommandEnd) {}

fn push_trace_io_command_end(_a: &mut Attrs, _e: &buck2_data::TraceIoCommandEnd) {}

fn push_configured_targets_command_end(
    _a: &mut Attrs,
    _e: &buck2_data::ConfiguredTargetsCommandEnd,
) {
}

fn push_starlark_debug_attach_command_end(
    _a: &mut Attrs,
    _e: &buck2_data::StarlarkDebugAttachCommandEnd,
) {
}

fn push_explain_command_end(_a: &mut Attrs, _e: &buck2_data::ExplainCommandEnd) {}

fn push_expand_external_cells_command_end(
    _a: &mut Attrs,
    _e: &buck2_data::ExpandExternalCellsCommandEnd,
) {
}

fn push_complete_command_end(_a: &mut Attrs, _e: &buck2_data::CompleteCommandEnd) {}

fn push_hydration_command_end(_a: &mut Attrs, _e: &buck2_data::HydrationCommandEnd) {}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn find<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a Value> {
        attrs
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| &kv.value)
    }

    fn string(attrs: &[KeyValue], key: &str) -> Option<String> {
        match find(attrs, key)? {
            Value::String(s) => Some(s.as_str().to_owned()),
            _ => None,
        }
    }

    fn int(attrs: &[KeyValue], key: &str) -> Option<i64> {
        match find(attrs, key)? {
            Value::I64(i) => Some(*i),
            _ => None,
        }
    }

    fn boolean(attrs: &[KeyValue], key: &str) -> Option<bool> {
        match find(attrs, key)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn strings(attrs: &[KeyValue], key: &str) -> Option<Vec<String>> {
        match find(attrs, key)? {
            Value::Array(Array::String(v)) => {
                Some(v.iter().map(|s| s.as_str().to_owned()).collect())
            }
            _ => None,
        }
    }

    /// Exercises one representative field of each mapping shape: scalars, optionals (set and unset),
    /// the `_us` duration conversion, repeated scalars, an enum projected to its proto name, a nested
    /// sub-message prefix, the dynamic `metadata` passthrough, and a repeated message projected
    /// field-wise into parallel arrays. If the mapping for any of these shapes regresses, one of
    /// these assertions fails.
    #[test]
    fn maps_representative_fields() {
        let record = buck2_data::InvocationRecord {
            // Scalar string.
            re_session_id: "session-123".to_owned(),
            // Repeated scalar -> single string array.
            cli_args: vec!["build".to_owned(), "//foo:bar".to_owned()],
            // Required int.
            run_local_count: 7,
            // Optional int, present.
            run_fallback_count: Some(3),
            // Optional int, absent -- `run_local_only_count` is left None below.
            // Duration -> integer microseconds.
            command_duration: Some(::prost_types::Duration {
                seconds: 2,
                nanos: 500_000,
            }),
            // Enum stored as i32 -> proto enum name.
            outcome: Some(buck2_data::InvocationOutcome::Success as i32),
            // Fields mapped onto OpenTelemetry semantic conventions.
            exit_code: Some(0),
            git_revision: Some("deadbeef".to_owned()),
            // Nested sub-message, emitted under a prefix.
            first_snapshot: Some(buck2_data::Snapshot {
                buck2_max_rss: 42,
                ..Default::default()
            }),
            // Nested sub-message carrying a semantic-convention field (`branched_from_revision`).
            file_watcher_stats: Some(buck2_data::FileWatcherStats {
                branched_from_revision: Some("cafef00d".to_owned()),
                ..Default::default()
            }),
            // Dynamic passthrough map.
            metadata: Some(buck2_data::TypedMetadata {
                strings: HashMap::from([("flavor".to_owned(), "vanilla".to_owned())]),
                ints: HashMap::from([("answer".to_owned(), 42)]),
            }),
            // Repeated message -> parallel field-wise arrays. The second error has no category, so
            // `errors.category` is shorter than `errors.message` (it filters out the None).
            errors: vec![
                buck2_data::ProcessedErrorReport {
                    message: "boom".to_owned(),
                    category: Some("INFRA".to_owned()),
                    ..Default::default()
                },
                buck2_data::ProcessedErrorReport {
                    message: "bad".to_owned(),
                    category: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let attrs = invocation_record_attributes(&record);

        // Static marker so backends can segment this wide event.
        assert_eq!(
            string(&attrs, "buck2.event_type").as_deref(),
            Some("invocation_record")
        );

        // buck2-specific fields are namespaced under `buck2.`.
        assert_eq!(
            string(&attrs, "buck2.re_session_id").as_deref(),
            Some("session-123")
        );
        assert_eq!(int(&attrs, "buck2.run_local_count"), Some(7));
        assert_eq!(int(&attrs, "buck2.run_fallback_count"), Some(3));

        // Fields that map onto OpenTelemetry semantic conventions use the standardized name and
        // are *not* `buck2.`-prefixed.
        assert_eq!(
            strings(&attrs, PROCESS_COMMAND_ARGS),
            Some(vec!["build".to_owned(), "//foo:bar".to_owned()])
        );
        assert!(find(&attrs, "buck2.cli_args").is_none());
        assert_eq!(int(&attrs, PROCESS_EXIT_CODE), Some(0));
        assert_eq!(
            string(&attrs, VCS_REF_HEAD_REVISION).as_deref(),
            Some("deadbeef")
        );
        assert_eq!(
            string(&attrs, VCS_REF_BASE_REVISION).as_deref(),
            Some("cafef00d")
        );

        // Unset optional fields produce no attribute at all (rather than a zero/empty column).
        assert!(find(&attrs, "buck2.run_local_only_count").is_none());

        // 2s + 500_000ns = 2_000_000us + 500us.
        assert_eq!(int(&attrs, "buck2.command_duration_us"), Some(2_000_500));

        assert_eq!(string(&attrs, "buck2.outcome").as_deref(), Some("Success"));
        assert_eq!(int(&attrs, "buck2.first_snapshot.buck2_max_rss"), Some(42));

        assert_eq!(
            string(&attrs, "buck2.metadata.flavor").as_deref(),
            Some("vanilla")
        );
        assert_eq!(int(&attrs, "buck2.metadata.answer"), Some(42));

        assert_eq!(
            strings(&attrs, "buck2.errors.message"),
            Some(vec!["boom".to_owned(), "bad".to_owned()])
        );
        assert_eq!(
            strings(&attrs, "buck2.errors.category"),
            Some(vec!["INFRA".to_owned()])
        );
    }

    /// Empty repeated fields and absent optionals must not produce attributes -- an empty column
    /// carries no information and just inflates every event. The default record has nothing set.
    #[test]
    fn skips_empty_and_absent_fields() {
        let attrs = invocation_record_attributes(&buck2_data::InvocationRecord::default());

        // Empty repeated scalar: no array attribute.
        assert!(find(&attrs, PROCESS_COMMAND_ARGS).is_none());
        assert!(find(&attrs, "buck2.tags").is_none());
        // Absent optional duration / enum / nested message.
        assert!(find(&attrs, "buck2.command_duration_us").is_none());
        assert!(find(&attrs, "buck2.outcome").is_none());
        assert!(find(&attrs, "buck2.first_snapshot.buck2_max_rss").is_none());

        // The static marker is unconditional, so it is always present even for an empty record.
        assert_eq!(
            string(&attrs, "buck2.event_type").as_deref(),
            Some("invocation_record")
        );
    }

    /// `bool` optionals: present when set, absent when not. (The default-record test covers absence
    /// of most shapes; bools get their own check because `false` is a meaningful set value that must
    /// not be confused with "unset".)
    #[test]
    fn maps_optional_bools() {
        let record = buck2_data::InvocationRecord {
            has_local_changes: Some(false),
            ..Default::default()
        };
        let attrs = invocation_record_attributes(&record);
        // Explicitly set to `false` -- must be emitted, not skipped.
        assert_eq!(boolean(&attrs, "buck2.has_local_changes"), Some(false));
        // Never set -- must be absent.
        assert!(find(&attrs, "buck2.new_configs_used").is_none());
    }

    /// `client_metadata` / `install_device_metadata` are key-value maps: each entry's key becomes
    /// its own attribute (`<map>.<entry_key>`), not parallel `.key`/`.value` arrays. Duplicate keys
    /// (here, two install devices sharing `os`) are resolved last-wins by the dedup pass.
    #[test]
    fn maps_key_value_metadata() {
        let record = buck2_data::InvocationRecord {
            client_metadata: vec![buck2_data::ClientMetadata {
                key: "user".to_owned(),
                value: "alice".to_owned(),
            }],
            install_device_metadata: vec![
                buck2_data::DeviceMetadata {
                    entry: vec![buck2_data::device_metadata::Entry {
                        key: "os".to_owned(),
                        value: "ios".to_owned(),
                    }],
                },
                buck2_data::DeviceMetadata {
                    entry: vec![buck2_data::device_metadata::Entry {
                        key: "os".to_owned(),
                        value: "android".to_owned(),
                    }],
                },
            ],
            ..Default::default()
        };
        let attrs = invocation_record_attributes(&record);

        // Entry key becomes the attribute key; no parallel `.key`/`.value` arrays.
        assert_eq!(
            string(&attrs, "buck2.client_metadata.user").as_deref(),
            Some("alice")
        );
        assert!(find(&attrs, "buck2.client_metadata.key").is_none());
        assert!(find(&attrs, "buck2.client_metadata.value").is_none());

        // Duplicate key across devices: last write wins, and only one attribute is emitted.
        assert_eq!(
            string(&attrs, "buck2.install_device_metadata.os").as_deref(),
            Some("android")
        );
        assert_eq!(
            attrs
                .iter()
                .filter(|kv| kv.key.as_str() == "buck2.install_device_metadata.os")
                .count(),
            1
        );
    }
}
