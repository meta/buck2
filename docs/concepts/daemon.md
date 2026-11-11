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

## Daemon sharing

Daemon sharing is opt-in and off by default. `[buck2] share_daemon = true` in
the root `.buckconfig` lets compatible tenants share one daemon while retaining
separate output and cache state. The key accepts a hostname share such as
`hostname:0.1`, which enables sharing on that fraction of hosts for A/B
testing; every client of a host then agrees on its routing. Hosts that cannot
run a shared daemon (today: anything but Linux, or a non-deferred materializer)
keep one daemon per isolation directory whatever the share says, as does
`--no-buckd`. The `daemon_routing` command metadata records the mode in use.

A tenant is a `(project root, isolation directory)` pair. Clients select the
shared daemon by a compatibility key, a BLAKE3 hash of the binary version, user
version, startup configuration and trace-I/O mode, and the daemon's connection
information lives in a namespace directory named by that key. Tenants with the
same key share a daemon across isolation directories and across checkouts;
a client whose key differs never sees that daemon's files. Each tenant also
keeps a small proxy in its usual daemon directory whose authenticated Status
and Kill methods represent the tenant, so a client of another release, old or
new, evicts the tenant the way it kills any daemon, and then joins or starts a
daemon for its own key. A client of the same release connects to the same proxy
to learn which namespace and generation serve the tenant, and sends that
generation with every command; the proxy's connection files are the only
record of the tenant. Routing is a daemon startup setting, so a daemon only
serves tenants routed the same way.

`buck2 kill` evicts the selected tenant from the daemon and cancels its work.
Eviction waits for cancellation and cleanup before returning. A later command
can recreate the tenant only after the old generation releases its output and
cache directories. Other tenants continue running; the daemon shuts down after
its final tenant is evicted. `buck2 clean` uses the same ownership lock while
deleting the selected tenant's state, and `buck2 killall --repo` evicts the
repository's tenants instead of killing processes.

Kill arms an independent daemon watchdog before acknowledgement. The proxy has
its own fallback one second before the killing client's grace runs out: three
seconds after a released client's Kill, longer for a client of this release,
which gives the eviction its own budget. Failed eviction can terminate
the entire daemon and its action descendants, including other tenants. This is
deliberate. Released clients do not know tenants exist: they start a daemon of
their own over a tenant's outputs four seconds after asking the slot's owner to
stop, or at once when that owner is dead. By then the shared daemon must have
released those outputs or not be running, so an eviction that overruns its
budget, a proxy that is lost, or an eviction the daemon cannot start ends the
whole daemon rather than risk two daemons writing one buck-out. After
final-tenant eviction, the proxy waits for the daemon to exit before releasing
the slot. A monitoring socket verifies proxy identity before metadata publication.
Unexpected liveness loss immediately retires and evicts the generation. After a
crash or a broken liveness link, automatic recovery creates a fresh generation
once cleanup is done and the slot still names the lost proxy. A proxy stopped
with `SIGKILL` or `SIGTERM`, as `buck2 killall` does, stays stopped: its tenant
retires and its connection files are removed, and the next command registers
the tenant again. Normal Kill suppresses recovery. Kill and EOF share the first
eviction's deadline; later notifications cannot extend it.

Lifecycle outcomes are appended to `unidaemon-events.jsonl` in the parent of
the tenant daemon directories. Records include eviction, verified forced exit,
proxy death and respawn; eviction timing starts when the proxy receives Kill.

`[buck2] share_dice = false` (the default) creates a separate DICE engine for
each tenant. Setting it to `true` shares one engine within the daemon. Each
tenant keeps its own root, repository data, and command coordinator.
Matching tracked inputs allow results to be reused across tenant roots. File
reads depend on the watcher's filesystem baseline: EdenFS and scm-aware
Watchman can use a shared mergebase when they have recorded the checkout's
differences from it; watchers without that guarantee use a unique baseline.
Artifact filesystem state also identifies the checkout's project root.

The client selects this setting at daemon startup. It is part of the
compatibility key, so changing it selects another shared daemon.
Command event logs include
a `dice-sharing:true` or `dice-sharing:false` tag.

Command metadata identifies the tenant a command ran against, so telemetry
that used to key on `daemon_uuid` can key on a tenant instead:

- `daemon_routing`: `unshared` or `shared`.
- `tenant_id`: stable hash of the checkout root and isolation directory.
- `tenant_generation`: the generation the command ran against. Generations come from one
  counter per daemon, so a recreated tenant gets a higher number, but numbers are not consecutive
  for one tenant.
- `tenant_instance_id`: hash of the daemon id, tenant id, and generation. In
  an unshared daemon this is one-to-one with `daemon_uuid`.
- `tenant_count`: initialized tenants in the daemon when the command started.
  Process-wide measurements such as daemon memory cover all of them.
- `tenant_started_reason`: on the first command of a generation only.
  `initial`, `joined`, or `recreated:<command>` naming what evicted the
  previous generation, such as `recreated:kill`.

Heartbeat snapshots carry `tenant_uptime_s` alongside `daemon_uptime_s`.

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
