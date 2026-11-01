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
compatible paging, digest configuration, and Buck settings. Paging
uses the initial tenant's DICE storage path for the lifetime of the daemon.
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
