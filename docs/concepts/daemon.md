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

## Killing or disabling the Buck daemon

`buck2 kill` evicts the selected `(project root, isolation directory)` tenant
from the daemon and cancels its work. Eviction waits for cancellation and cleanup
before returning. A later command can recreate the tenant only after the old
generation releases its output and cache directories. Other tenants continue
running; the daemon shuts down after its final tenant is evicted. `buck2 clean`
uses the same ownership lock while deleting the selected tenant's state.

<FbInternalOnly>

The Daemon is also killed when:

- The unfiltered `buck2 killall` command is run. It kills every Buck2 process
  on the machine. `buck2 killall --repo` instead evicts repository tenants and
  shuts down a daemon after its final tenant is evicted. `--in-isolation-dir`
  can select one tenant.
- A new buck2 version is available.

</FbInternalOnly>
