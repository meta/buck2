# This file is @generated, regenerate by re-running test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command

A section's scope says how long its values live: a change to a `daemon` setting starts a new daemon, an `invocation` setting is read by every command, and a `dice` setting is read by every command and a change to it recomputes what read it. The Overridable column lists the sources that may set a key besides the repository's `.bucksettings.toml`: `--setting` on the command line, the `.bucksettings.local.toml` files, or neither. Values are written as TOML, so strings are quoted.

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

The event log the client writes for each command.

Scope: `invocation`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `retained_logs` | `usize` | `12` | `--setting`, local files | How many event logs are kept on disk, counting the one about to be written. |

## [execution_platforms]

Where a build finds its execution platforms.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `target` | `String` |  | `--setting`, local files | The target whose `ExecutionPlatformRegistrationInfo` lists the execution platforms, as a label relative to the root cell. Unset, every target runs its actions on one implicit execution platform in the target's own configuration. Falls back to `build.execution_platforms` in `.buckconfig`. |

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

## [starlark]

The Starlark interpreter that evaluates build files and macros.

Scope: `dice`.

| Key | Type | Default | Overridable | Description |
| --- | --- | --- | --- | --- |
| `max_callstack_size` | `usize` |  | `--setting`, local files | The deepest call stack an evaluation may build; unset leaves the interpreter's own limit. Falls back to `buck2.starlark_max_callstack_size` in `.buckconfig`. |

