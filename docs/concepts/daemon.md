---
id: daemon
title: Daemon (buckd)
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

The first time that a Buck2 command is run, Buck2 starts a daemon process for
the current project. For subsequent commands, Buck2 checks for the running
daemon process and, if found, uses the daemon to execute the command. Using the
Buck2 daemon can save significant time as it enables Buck to share cache between
Buck2 invocations.

By default, there is 1 daemon per [project](./glossary.md#project) root, you can
run multiple daemons in the same project by specifying an
[isolation dir](./glossary.md#isolation-dir).

While it runs, the Buck daemon process monitors the project's file system for
changes. The Buck daemon excludes from monitoring any subtrees of the project
file system that are specified in the `[project].ignore` setting of
`.buckconfig`.

You can see detailed information about the status of the daemon by running
`buck2 status`.

## Experimental daemon sharing

Daemon sharing is opt-in and off by default. `BUCK2_SHARE_DAEMON=true` lets
tenants in different isolation directories share a daemon while retaining
separate output and cache state.

In this mode `buck2 kill` evicts the selected `(project root, isolation
directory)` tenant from the daemon and cancels its work. Eviction waits for
cancellation and cleanup before returning. A later command can recreate the
tenant only after the old generation releases its output and cache
directories. Other tenants continue running; the daemon shuts down after its
final tenant is evicted. `buck2 clean` uses the same ownership lock while
deleting the selected tenant's state, and `buck2 killall --repo` evicts the
repository's tenants instead of killing processes. Incompatible startup
settings or binaries are reported without restarting the daemon; stop the
affected daemon's tenants explicitly before changing these settings.

### Shared daemons and tenant proxies

With `BUCK2_SHARE_DAEMON=true`, `BUCK2_UNIDAEMON=true` enables discovery
using a BLAKE3 hash of binary version, user version, startup configuration and
trace-I/O mode. Matching tenants across checkouts select the same namespace.
Changing compatibility settings moves the selected tenant to another process
while its siblings remain usable. Leave `BUCK2_DAEMON_GROUP` unset.

Sharing requires Linux and the deferred materializer. Set
`materializer = deferred` in the `[buck2]` section of each test checkout's
`.buckconfig`. Use a private daemon home
with `BUCK2_TEST_HOME_DIR` when experimenting.

A tenant's own daemon directory holds a small proxy whose authenticated Status
and Kill methods represent that tenant to clients of every release. An older
client kills it as it kills any daemon, which evicts the tenant. A newer client
connects to it the same way to learn which namespace and generation serve the
tenant, registers explicitly with the shared daemon, and sends its generation
with every command. Migration and clean hold the legacy lifecycle lock while
changing the directory's owner; a command that may start a daemon holds it until
the daemon is up and releases it before registering, since the proxy publishes
under it and the tenant then initializes. Eviction cancels tenant work and
waits for its disk ownership to drain.

Kill arms an independent daemon watchdog before acknowledgement. The proxy has
its own fallback one second before the killing client's grace runs out: three
seconds after a released client's Kill, longer for a client of this release,
which gives the eviction its own budget. Failed eviction can terminate
the entire unidaemon and its action descendants, including other tenants. After
final-tenant eviction, the proxy waits for the unidaemon to exit before releasing
the slot. A monitoring socket verifies proxy identity before metadata publication.
Unexpected liveness loss immediately retires and evicts the generation. After a
crash or a broken liveness link, automatic recovery creates a fresh generation
once cleanup is done and the slot still names the lost proxy. A proxy stopped
with `SIGKILL` or `SIGTERM`, as `buck2 killall` does, stays stopped: its tenant
retires and its connection files are removed, and the next command registers
the tenant again. Normal Kill suppresses recovery. Kill and EOF share the first
eviction's deadline; later notifications cannot extend it.

Lifecycle outcomes are appended to `unidaemon-events.jsonl` in the parent of the
legacy isolation directories. Records include eviction, verified forced exit,
proxy death and respawn; eviction timing starts when the proxy receives Kill.

To also share across checkouts, set `BUCK2_DAEMON_GROUP` to the same name in
each checkout. A group name must be a single valid filename component. Each
tenant still uses its own checkout's files, configuration and outputs, so
checkouts may stay on different commits.

Daemon groups require compatible startup configuration and binary versions.
An incompatible invocation fails without restarting the group's daemon.
Use different group names for A/B instances with different startup settings,
or evict all tenants in a group before changing those settings.

`[buck2] share_dice = false` (the default) creates a separate DICE engine for
each tenant. Setting it to `true` shares one engine within the daemon. Each
tenant keeps its own root, repository data, and command coordinator.
Matching tracked inputs allow results to be reused across tenant roots. File
reads depend on the watcher's filesystem baseline: EdenFS and scm-aware
Watchman can use a shared mergebase when they have recorded the checkout's
differences from it; watchers without that guarantee use a unique baseline.
Artifact filesystem state also identifies the checkout's project root.

The client selects this setting at daemon startup. It is part of the daemon
configuration checked on connection. Changing it restarts an unshared daemon.
A shared daemon reports the mismatch instead of restarting, so stop its tenants
with `buck2 killall --repo` first; for a named group, choose a new group or
evict its existing tenants.
Command event logs include
a `dice-sharing:true` or `dice-sharing:false` tag.

Cycle detection is always enabled in both modes. Shared engines require
compatible paging, digest configuration, and Buck settings.
Paging storage is scoped to the daemon: it lives under the daemon state
directory of the checkout that started the daemon, keyed by the daemon id,
outside every disposable tenant tree.
Evicting a tenant releases its roots after admitted commands finish; other
tenants keep running on the shared engine.

## Killing or disabling the Buck daemon

The Buck daemon process is killed if `buck2 clean` or `buck2 kill` commands are
run. Note that they won't kill the daemon associated with custom isolation dirs.
To do that, run using the `--isolation-dir` option
(`buck2 --isolation-dir <dir> <command>`)

<FbInternalOnly>

The Daemon is also killed when:

- The `buck2 killall` command is run. By default it kills every buck2 process
  on the machine; pass `--in-isolation-dir <dir>` and/or `--repo` to only kill
  processes using that isolation dir and/or running in the current repository.
- A new buck2 version is available.

</FbInternalOnly>
