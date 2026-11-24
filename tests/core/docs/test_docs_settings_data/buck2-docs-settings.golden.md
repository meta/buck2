# This file is @generated, regenerate by re-running test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command

A section's scope says how long its values live: a change to a `daemon` setting starts a new daemon, an `invocation` setting is read by every command, and a `dice` setting is read by every command and a change to it recomputes what read it. The Overridable column lists the sources that may set a key besides the repository's `.bucksettings.toml`: `--setting` on the command line, the `.bucksettings.local.toml` files, or neither. Values are written as TOML, so strings are quoted.

## [action_cache]

How a command uses the remote action cache.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `default_allow_upload` | `bool` | `false` | `--setting`, local files | Whether an action that does not say uploads the result of running locally: the default of a run action's `allow_cache_upload`. Falls back to `buck2.default_allow_cache_upload` in `.buckconfig`. |

## [agent_build]

Rule-specific check and test advice for the builds an AI agent runs
directly (commands given `--agent-context direct_call=true`) that request
no artifacts.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | `--setting`, local files | Stops an AI agent's direct `buck2 build` that asks for no outputs when the rule of a target has a check or test to recommend instead, until the agent acknowledges the advice. |

## [agent_config_change]

Blocks a command an AI agent runs directly (one given `--agent-context
direct_call=true`) when it drops `--config` or `--config-file` arguments
that the daemon's configuration uses, which would invalidate the daemon's
state. This guard does not need `agent_interactive.enabled`.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | `--setting`, local files | The guard's only switch. |

## [agent_interactive]

Guards on operations an AI agent runs directly (commands given
`--agent-context direct_call=true`). A guard stops the operation and
explains the alternatives until the agent retries with an acknowledgement
and a reason. Each guard needs both the master switch and its own.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | `--setting`, local files | The master switch: no guard of this section runs without it. |
| `killall_enabled` | `bool` | `false` | `--setting`, local files | Guards `buck2 killall`, which discards every daemon's state and can interrupt other work. |
| `clean_enabled` | `bool` | `false` | `--setting`, local files | Guards `buck2 clean`, which removes build state and outputs that later commands would reuse. |
| `new_isolation_dir_enabled` | `bool` | `false` | `--setting`, local files | Guards starting a daemon for a new isolation directory, which costs memory and starts without the existing daemon's state. |

## [agent_materialization]

Skips materializing the final outputs of builds an AI agent runs directly
(commands given `--agent-context direct_call=true`) without requesting
outputs.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | `--setting`, local files | Whether such a build leaves its final outputs unmaterialized. |

## [agent_recursive_patterns]

Requires explicit intent for recursive build and test patterns in the
commands an AI agent runs directly (those given `--agent-context
direct_call=true`). This guard does not need `agent_interactive.enabled`.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | `--setting`, local files | Stops an AI agent's direct `buck2 build` or `buck2 test` of a recursive pattern such as `//dir/...` until the agent acknowledges that it needs the whole scope. |

## [analysis]

Settings controlling what analysis records beyond its providers.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `record_requested_anon_targets` | `bool` | `true` | `--setting`, local files | Whether each analysis keeps the list of anon targets it requested. |

## [buck_out]

What a command leaves in `buck-out` besides the outputs themselves.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `unhashed_links` | `bool` | `false` | `--setting`, local files | Whether a build symlinks each output it was asked for at a path without the configuration hash. Falls back to `buck2.create_unhashed_links` in `.buckconfig`. |

## [build_metrics]

Measurements of the size of a build that a build or test command
computes on request and reports with its results. A sketch is a summary
of a set that can be merged with others to estimate the size of their
union.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `configured_graph_size` | `bool` | `false` | `--setting`, local files | For each top-level target, the number of configured targets in its dependency graph. Falls back to `buck2.log_configured_graph_size` in `.buckconfig`. |
| `configured_graph_sketch` | `bool` | `false` | `--setting`, local files | For each top-level target, a sketch of its configured dependency graph. Falls back to `buck2.log_configured_graph_sketch` in `.buckconfig`. |
| `total_configured_graph_sketch` | `bool` | `false` | `--setting`, local files | One sketch of the configured dependency graph of all top-level targets together. Falls back to `buck2.log_total_configured_graph_sketch` in `.buckconfig`. |
| `retained_analysis_memory_sketch` | `bool` | `false` | `--setting`, local files | A sketch of the memory that analysis retains. Falls back to `buck2.log_retained_analysis_memory_sketch` in `.buckconfig`. |
| `peak_analysis_memory_sketch` | `bool` | `false` | `--setting`, local files | A sketch of the peak memory of analysis. Falls back to `buck2.log_peak_analysis_memory_sketch` in `.buckconfig`. |
| `peak_load_memory_sketch` | `bool` | `false` | `--setting`, local files | A sketch of the peak memory of loading build files. Falls back to `buck2.log_peak_load_memory_sketch` in `.buckconfig`. |
| `action_graph_sketch` | `bool` | `false` | `--setting`, local files | For each top-level target, a sketch of its action graph. Falls back to `buck2.log_action_graph_sketch` in `.buckconfig`. |
| `artifact_count_sketch` | `bool` | `false` | `--setting`, local files | For each top-level target, a sketch of its artifacts by count. Falls back to `buck2.log_artifact_count_sketch` in `.buckconfig`. |
| `artifact_size_sketch` | `bool` | `false` | `--setting`, local files | For each top-level target, a sketch of its artifacts by size. Falls back to `buck2.log_artifact_size_sketch` in `.buckconfig`. |
| `artifact_sketch_skip_providers` | `Vec<SketchedProvider>` |  | `--setting`, local files | The outputs of a top-level target that the artifact sketches leave out: any of `build`, `run` and `test`. Falls back to `buck2.providers_to_skip_in_artifact_path_sketch` in `.buckconfig`. |
| `sketch_cardinalities` | `bool` | `false` | `--setting`, local files | Whether each sketch in the build report comes with a `<field>_cardinality` field that holds its estimated cardinality. Falls back to `buck2.log_sketch_cardinalities` in `.buckconfig`. |
| `detailed_aggregated` | `bool` | `false` | `--setting`, local files | Aggregated metrics of the actions behind each top-level target and behind all of them together. Falls back to `buck2.detailed_aggregated_metrics` in `.buckconfig`. |

## [build_report]

What goes into the build report of a build or test command.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `print_unconfigured_section` | `bool` | `true` | `--setting`, local files | Whether each target's entry has the buck1-compatible fields that are not split by configuration, next to its `configured` map. Falls back to `build_report.print_unconfigured_section` in `.buckconfig`. |

## [console]

What the console shows while a command runs.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `thread_line_limit` | `u64` |  | `--setting`, local files | The most lines the console spends on what is running; unset leaves the console's own limit. Falls back to `ui.thread_line_limit` in `.buckconfig`. |

## [daemon]

The daemon process: when it exits, how many threads it runs on, and how
the OS schedules it.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `buster` | `String` |  | `--setting`, local files | Any value; changing it starts a new daemon. Set in the repository, it restarts every daemon after a change that invalidates their state. Falls back to `buck2.daemon_buster` in `.buckconfig`. |
| `idle_timeout_s` | `u64` |  | `--setting`, local files | Seconds without a command after which the daemon exits; 4 days (`345600`) when unset. Falls back to `buck2.daemon_idle_timeout_s` in `.buckconfig`. |
| `num_tokio_workers` | `usize` |  | `--setting`, local files | Worker threads of the daemon's tokio runtime; `0` or unset lets tokio choose. Falls back to `build.num_tokio_workers` in `.buckconfig`. |
| `macos_qos_class` | `MacosQosClass` |  | `--setting`, local files | The macOS QoS class the daemon lowers itself to when it starts: `utility`, `background`, or `skip_lowering` to stay at the system default. `utility` when unset. Falls back to `buck2.macos_qos_class` in `.buckconfig`. |

## [dep_files]

The daemon's cache of run actions it already ran, keyed by the inputs
their dep files say they used.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `cross_config_lookup_on_stale_entry` | `bool` | `false` | `--setting`, local files | Whether a run action whose own cache entry holds a different action is still served an identical action that another configuration of it ran in this daemon. Without it, only an action with no entry of its own looks at other configurations' entries. Falls back to `buck2.cross_config_lookup_on_stale_entry` in `.buckconfig`. |

## [digest]

How buck2 hashes file contents and action inputs.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `algorithms` | `String` |  | `--setting`, local files | The algorithms the daemon accepts, most preferred first, as a comma-separated list, without spaces, of `SHA256`, `SHA1`, `BLAKE3` and, in Meta-internal builds only, `BLAKE3-KEYED`. Unset, it is `SHA256` in open source builds and `SHA1` in Meta-internal builds. Falls back to `buck2.digest_algorithms` in `.buckconfig`. |
| `source_algorithm` | `String` |  | `--setting`, local files | The algorithm source files are hashed with, one of `algorithms`; the first of them when unset. Falls back to `buck2.source_digest_algorithm` in `.buckconfig`. |

## [event_forwarding]

Whether the daemon starts a helper program for each command that carries an event forwarding
key, and which program.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | no | Whether the daemon starts the helper for each command that carries an event forwarding key. |
| `helper` | `String` |  | no | The helper program: an absolute path, or a path relative to the project root. |

## [event_log]

The event log of a command: what goes into it and how many are kept.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `retained_logs` | `usize` | `12` | `--setting`, local files | How many event logs are kept on disk, counting the one about to be written. |
| `action_graph` | `bool` | `false` | `--setting`, local files | Whether the log records each executed action's direct dependencies, for `buck2 log graph`. Falls back to `buck2.log_action_graph` in `.buckconfig`. |

## [execution]

How a command runs actions on this machine.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `threads` | `u32` |  | `--setting`, local files | How many actions run locally at once; `0` or unset for one per core. `-j` on the command line wins. Falls back to `build.threads` in `.buckconfig`. |
| `persistent_worker_shutdown_timeout_s` | `u32` | `10` | `--setting`, local files | Seconds a persistent worker gets to exit after `SIGTERM` at shutdown before its process group gets `SIGKILL`. Falls back to `build.persistent_worker_shutdown_timeout_s` in `.buckconfig`. |
| `miniperf` | `bool` | `true` | `--setting`, local files | Whether local actions run under miniperf, which counts the instructions they execute. Turn it off where perf counters are unavailable, as in some virtual machines. Falls back to `buck2.miniperf2` in `.buckconfig`. |

## [execution_platforms]

Where a build finds its execution platforms.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `target` | `String` |  | `--setting`, local files | The target whose `ExecutionPlatformRegistrationInfo` lists the execution platforms, as a label relative to the root cell. Unset, every target runs its actions on one implicit execution platform in the target's own configuration. Falls back to `build.execution_platforms` in `.buckconfig`. |
| `preserve_marker` | `bool` | `false` | `--setting`, local files | Whether the marker constraint the registration declares stays a marker on each execution platform's configuration, rather than being folded into the platform's constraints. Falls back to `buck2.preserve_exec_platform_marker` in `.buckconfig`. |

## [health_check]

The health checks the client runs alongside a command.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enabled` | `bool` | `false` | `--setting`, local files | Whether the client runs health checks and shows what they find. Meta-internal builds only: open source builds have no health checks. Falls back to `buck2_health_check.enable_health_checks` in `.buckconfig`. |

## [http]

The HTTP client the daemon downloads with.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `connect_timeout_ms` | `u64` |  | `--setting`, local files | Milliseconds to establish a connection; `0` for no limit, `5000` when unset. Falls back to `http.connect_timeout_ms` in `.buckconfig`. |
| `read_timeout_ms` | `u64` |  | `--setting`, local files | Milliseconds to wait for response data; `0` for no limit, `10000` when unset. Falls back to `http.read_timeout_ms` in `.buckconfig`. |
| `write_timeout_ms` | `u64` |  | `--setting`, local files | Milliseconds to send request data; `0` or unset for no limit. Falls back to `http.write_timeout_ms` in `.buckconfig`. |
| `http2` | `bool` | `true` | `--setting`, local files | Whether HTTP/2 is negotiated. Falls back to `http.http2` in `.buckconfig`. |
| `max_redirects` | `usize` |  | `--setting`, local files | Redirects followed per request; `10` when unset. Falls back to `http.max_redirects` in `.buckconfig`. |
| `max_concurrent_requests` | `usize` |  | `--setting`, local files | Requests in flight at once; unset for no limit. Falls back to `http.max_concurrent_requests` in `.buckconfig`. |
| `proxy_env_allowlist` | `Vec<String>` |  | `--setting`, local files | In internal builds, the hosts that are reached through the proxy named in the daemon's environment: exact hostnames or IP addresses. Every other host is reached directly. Falls back to `http.proxy_env_allowlist` in `.buckconfig`. |

## [hydration]

Paging the values of the daemon's dependency graph (DICE) out to disk,
and reading them back as commands need them.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `enable_paging` | `bool` | `false` | `--setting`, local files | Whether DICE values can be paged out to on-disk storage. Implied by `page_out_on_idle`. |
| `defer_field_reads` | `bool` | `true` | `--setting`, local files | Whether values read back from disk (registered actions, analysis results and, in Meta-internal builds, Starlark fields) are decoded when they are first used, instead of all at once. |
| `page_out_on_idle` | `bool` | `false` | `--setting`, local files | Whether the daemon pages DICE out when it goes idle. |
| `page_out_on_idle_isolation_dir_scope` | `PageOutOnIdleIsolationDirScope` | `"all"` | `--setting`, local files | Which daemons page out when idle: `all`, or `non_default` to leave the daemon of the default isolation directory alone. |
| `page_out_min_free_disk_gb` | `u64` | `100` | `--setting`, local files | Idle page-out runs only when at least this many GiB of disk are free. |
| `page_out_policy` | `PageOutPolicy` | `"once_per_value"` | `--setting`, local files | Which values a page-out writes. `once_per_value` writes each value once, so a value recomputed or read back from disk stays backed by a row and can leave memory again without a write. `once_per_key` pages a key out at most once: a value recomputed or read back afterwards stays in memory. |
| `eviction_policy` | `EvictionPolicy` | `"on_memory_pressure"` | `--setting`, local files | When the values a page-out writes leave memory. `on_idle` evicts them as they are written; `on_memory_pressure` keeps them in memory, backed by their rows, until the daemon sees memory pressure. `on_idle_lru` and `on_memory_pressure_lru` evict the values least recently used first, and `on_idle_lru` leaves writes in memory for a later idle reclaim. |
| `pagable_storage_backend` | `PagableStorageBackend` | `"sqlite"` | `--setting`, local files | The on-disk backend for pagable DICE storage: `sqlite`, `sled`, or `noop`, which serializes values and discards them. |
| `key_read_ahead` | `bool` | `true` | `--setting`, local files | Whether the SQLite backend reads a shard's key file into the page cache whole once key lookups in it have started going to disk. |

## [labels]

How labels written in build files are read.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `infer_target_names` | `bool` | `false` | `--setting`, local files | Whether a label in a build or `.bzl` file that names no target means the target named like the last directory of its package: `//foo/bar` for `//foo/bar:bar`. In open source builds, falls back to `buck2.infer_target_names` in `.buckconfig`. |

## [log_download]

Where `buck2 log` commands fetch an event log that is not on this machine.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `log_use_manifold` | `bool` | `false`; `true` in Meta-internal builds | `--setting`, local files | Whether such logs are fetched from Manifold, Meta's blob store. Falls back to `buck2.log_use_manifold` in `.buckconfig`. |
| `log_url` | `String` |  | `--setting`, local files | With `log_use_manifold` off, the base URL such logs are fetched from, as `<log_url>/v1/logs/get/<trace id>`. Falls back to `buck2.log_url` in `.buckconfig`. |

## [materializer]

The daemon's materializer.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `materializations` | `String` |  | `--setting`, local files | Whether a build writes its final outputs to disk: `deferred` (the default when unset) and `all` do, `deferred_skip_final_artifacts` does not. Falls back to `buck2.materializations` in `.buckconfig`. |

## [package_visibility]

Settings controlling PACKAGE visibility intersection.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `default_intersection` | `PackageVisibilityDefaultIntersection` | `"off"` | no | Whether ordinary `package(visibility=...)` declarations contribute to the visibility intersection: `off` leaves them out, `audit` reports the dependencies they would block without failing, and `enforce` fails those dependencies. |

## [paths]

Which paths build files may name.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `allow_relative_in` | `Vec<String>` |  | `--setting`, local files | Directories, as cell paths such as `root//foo`, under which a build file may reach files of a parent directory with `../`, up to the listed directory itself. At most one entry may cover a given build file. Falls back to `buck2.directories_to_allow_relative_paths` in `.buckconfig`. |

## [remote_execution]

How a command uses remote execution.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `cancel_on_estimated_queue_time_exceeds_s` | `u64` |  | `--setting`, local files | Cancels a remote action once the seconds remote execution estimates it will wait in the queue exceed this. Falls back to `build.remote_execution_cancel_on_estimated_queue_time_exceeds_s` in `.buckconfig`. |
| `fallback_on_estimated_queue_time_exceeds_s` | `u64` |  | `--setting`, local files | Lets an action that may also run locally start locally once the seconds remote execution estimates it will wait in the queue exceed this. Wins over an executor's own `remote_execution_queue_time_threshold_s`. Falls back to `build.remote_execution_fallback_on_estimated_queue_time_exceeds_s` in `.buckconfig`. |
| `output_trees_download_max_concurrent_bytes` | `u32` |  | `--setting`, local files | Bytes of output directory listings a command downloads at once; a download that would exceed this waits. Unset for no limit. Falls back to `buck2.output_trees_download_semaphore_size` in `.buckconfig`. |

## [resource_control]

The cgroups the daemon and its actions run in, and what happens under
memory pressure. Linux only.

Scope: `daemon`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `status` | `String` |  | `--setting`, local files | `off`, `if_available` (when systemd can provide a cgroup) or `required`. Unset defers to `min_version_for_gated_status`. Falls back to `buck2_resource_control.status` in `.buckconfig`. |
| `init` | `String` |  | `--setting`, local files | Where the daemon's cgroup comes from: `systemd`, or `cgroup:<path>` for one that already exists. `systemd` when unset. Falls back to `buck2_resource_control.init` in `.buckconfig`. |
| `memory_max` | `String` |  | `--setting`, local files | Memory limit for everything buck2 manages, as bytes or as a percentage (`50%`) of the limit of the cgroup buck2 starts in; a percentage sets no limit when that cgroup has none. Falls back to `buck2_resource_control.memory_max` in `.buckconfig`. |
| `memory_high` | `String` |  | `--setting`, local files | Like `memory_max`, for cgroup v2's `memory.high`. Falls back to `buck2_resource_control.memory_high` in `.buckconfig`. |
| `memory_max_per_action` | `String` |  | `--setting`, local files | Has no effect at present. Falls back to `buck2_resource_control.memory_max_per_action` in `.buckconfig`. |
| `memory_high_per_action` | `String` |  | `--setting`, local files | Memory any one action may reach before it is throttled. Falls back to `buck2_resource_control.memory_high_per_action` in `.buckconfig`. |
| `memory_high_actions` | `String` |  | `--setting`, local files | `memory.high` for all actions together. Mainly for tests. Falls back to `buck2_resource_control.memory_high_actions` in `.buckconfig`. |
| `memory_max_actions` | `String` |  | `--setting`, local files | `memory.max` for all actions together. Mainly for tests. Falls back to `buck2_resource_control.memory_max_actions` in `.buckconfig`. |
| `memory_swap_max_actions` | `String` |  | `--setting`, local files | `memory.swap.max` for all actions together, as bytes or `max`. `0` makes the kernel stall an action at its memory limit instead of swapping, which is the pressure the scheduler suspends on. Mainly for tests. Falls back to `buck2_resource_control.memory_swap_max_actions` in `.buckconfig`. |
| `enable_suspension` | `bool` |  | `--setting`, local files | Whether actions are suspended under memory pressure. Falls back to `buck2_resource_control.enable_suspension` in `.buckconfig`. |
| `enable_suspension_if_min_algo_version` | `u32` |  | `--setting`, local files | Also enables suspension when the binary's suspension algorithm is at least this version, so a fix can be rolled out without reaching binaries that lack it. Falls back to `buck2_resource_control.enable_suspension_if_min_algo_version` in `.buckconfig`. |
| `experimental_suspension_algo_variant` | `u8` |  | `--setting`, local files | Selects an experimental variant of the suspension algorithm. There is none at present, so it has no effect. Falls back to `buck2_resource_control.experimental_suspension_algo_variant` in `.buckconfig`. |
| `preferred_action_suspend_strategy` | `String` |  | `--setting`, local files | How an action is suspended: `kill_and_retry` (the default when unset) or `cgroup_freeze`. Falls back to `buck2_resource_control.preferred_action_suspend_strategy` in `.buckconfig`. |
| `min_version_for_gated_status` | `u32` |  | `--setting`, local files | With `status` unset, binaries whose daemon cgroup logic is at least this version use `version_gated_default_status`; older ones stay off. Falls back to `buck2_resource_control.min_version_for_gated_status` in `.buckconfig`. |
| `version_gated_default_status` | `String` |  | `--setting`, local files | The status a binary new enough for `min_version_for_gated_status` takes; `if_available` when unset. Falls back to `buck2_resource_control.version_gated_default_status` in `.buckconfig`. |

## [source]

How source files become action inputs.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `correct_symlink_reading` | `bool` | `true` | `--setting`, local files | Whether a source that is itself a symlink reaches actions as that symlink, with its target as a further input, instead of as a copy of what it points to. Falls back to `buck2.use_correct_source_symlink_reading` in `.buckconfig`. |

## [starlark]

The Starlark interpreter that evaluates build files and macros.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `max_callstack_size` | `usize` |  | `--setting`, local files | The deepest call stack an evaluation may build; unset leaves the interpreter's own limit. Falls back to `buck2.starlark_max_callstack_size` in `.buckconfig`. |

## [target_compatibility]

What happens to a target that is incompatible with the configuration it
is built in.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `error_on_dep_only_incompatible` | `Vec<String>` |  | `--setting`, local files | Target patterns under which a target that is incompatible only because one of its dependencies is, is an error. Elsewhere such a target is skipped as incompatible, with a soft error. Falls back to `buck2.error_on_dep_only_incompatible` in `.buckconfig`. |
| `error_on_dep_only_incompatible_excluded` | `Vec<String>` |  | `--setting`, local files | Target patterns taken back out of `error_on_dep_only_incompatible`. Falls back to `buck2.error_on_dep_only_incompatible_excluded` in `.buckconfig`. |
| `dep_only_incompatible_info` | `String` |  | `--setting`, local files | A target whose `DepOnlyIncompatibleInfo` names further soft errors to report for such a target, each for its own target patterns. Falls back to `buck2.dep_only_incompatible_info` in `.buckconfig`. |

## [target_platforms]

How a target that names no platform of its own gets one.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `detector_spec` | `String` |  | `--setting`, local files | The default target platform by package, for targets without a `default_target_platform`: space-separated entries of the form `target:<recursive pattern>-><platform target>`, the first matching entry winning. Unset means such targets get no platform. Falls back to `parser.target_platform_detector_spec` in `.buckconfig`. |

## [test]

How `buck2 test` runs tests.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `executor` | `String` | unset; `"$BUCK2_BINARY_DIR/buck2-tpx"` in Meta-internal builds | `--setting`, local files | The path of the test executor, where a leading `$BUCK2_BINARY_DIR/` is the directory of the buck2 binary. Empty uses the executor built into buck2, and so does unset in open source. Falls back to `test.v2_test_executor` in `.buckconfig`. |
| `internal_runner` | `String` |  | `--setting`, local files | Which tests buck2 runs itself instead of handing them to the executor: `true` for all of them, which is also what unset means, `false` for none, or a comma-separated list of test frameworks. Falls back to `test.use_internal_runner` in `.buckconfig`. |
| `default_timeout_s` | `u64` | `600` | `--setting`, local files | Seconds a test that buck2 runs itself may take. Falls back to `test.timeout_default_s` in `.buckconfig`. |
| `build_targets` | `bool` | `false` | `--setting`, local files | Whether `buck2 test` also builds the default outputs and the run info of the tests it runs, where the command line does not say. Falls back to `buck2.test_builds_targets` in `.buckconfig`. |
| `timeout_grace_period_s` | `u64` |  | `--setting`, local files | Seconds a local test that hit its timeout gets to exit after `SIGTERM` before it and what is left of its process group get `SIGKILL`. Unset or `0` kills at once. Falls back to `buck2.test_timeout_grace_period_s` in `.buckconfig`. |
| `kill_detached_processes` | `bool` | `false` | `--setting`, local files | On Linux, whether killing a local test also kills the processes it left outside its process group, such as daemons. Falls back to `buck2.kill_detached_test_processes` in `.buckconfig`. |
| `local_resource_cleanup_registration` | `bool` | `false` | `--setting`, local files | On Linux, whether a local resource's setup command is handed a way to register cleanup that outlives it. Falls back to `buck2.local_resource_cleanup_registration` in `.buckconfig`. |
| `inherit_client_coding_agent_metadata` | `bool` | `false` | `--setting`, local files | Whether local tests see the `CODING_AGENT_METADATA` of the client that started the command. Falls back to `buck2.tests_inherit_client_coding_agent_metadata` in `.buckconfig`. |

## [write_actions]

How a command produces the outputs of write actions.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `cas_probe` | `bool` | `false` | `--setting`, local files | Whether a write action asks the CAS for its content first and declares a CAS download instead of a write when it is there. Only takes effect where the materializer defers writes. |
| `cache` | `WriteActionCacheMode` | `"off"` | `--setting`, local files | How write actions use the action cache: `off`, `verify` or `use`. |
| `cache_populate` | `RolloutFraction` | `0.0` | `--setting`, local files | The share of write keys, from 0 to 1, whose misses are recorded in the action cache for later builds. By key rather than by host, so the cache fills in steps that are the same on every host. A write within the share goes through the cache whatever `cache` says. |

