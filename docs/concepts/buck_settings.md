---
id: buck_settings
title: Buck settings
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

Buck settings are the configuration Buck2 reads about itself: how the daemon
runs, where logs go, which checks are enforced. They are separate from
[`.buckconfig`](buckconfig.md), which holds the values that rules and macros
read.

- Every setting is declared in Buck2 with its type and documentation, which the
  [Buck settings reference](../buck_settings_reference) lists. A key Buck2 does
  not know, or a value of the wrong type, is an error.
- Settings are not readable from Starlark.
- Each setting says where it may be overridden, so a repository can keep a
  setting out of reach of local files and command lines.

## Settings files

Settings files are [TOML](https://toml.io). Each table is a section, and each
key in it is one setting:

```toml
[log_download]
log_use_manifold = true

[event_log]
retained_logs = 20
```

A file sets only the keys it names. Every other key keeps the value it gets
from the other sources.

## Where a value comes from

A setting takes its value from the highest of these sources that sets it:

| Priority    | Source                                                    |
| ----------- | --------------------------------------------------------- |
| 1 (highest) | `--setting section.key=value` on the command line         |
| 2           | `.bucksettings.local.toml` at the root of the project     |
| 3           | `.bucksettings.local.toml` in your home directory         |
| 4           | Rollouts (internal builds of Buck2 only)                  |
| 5           | `.bucksettings.toml` at the root of the project           |
| 6           | The `.buckconfig` key the setting falls back to, if any   |
| 7 (lowest)  | The default built into Buck2                              |

`.bucksettings.toml` is the file a repository checks in. The
`.bucksettings.local.toml` files are for one checkout or one user and are not
checked in.

A setting can have one default in Meta-internal builds of Buck2 and another in
open source builds.

### The command line

`--setting` takes one `section.key=value`. Repeat it to set several keys; when
two flags set the same key the later one wins.

```sh
buck2 --setting event_log.retained_logs=20 build //:target
```

The value is read as a TOML value when it is one (`true`, `20`, `"text"`) and
as a string otherwise.

### `.buckconfig` fallbacks

Settings that used to be `.buckconfig` keys can still read the old key. Such a
setting uses the `.buckconfig` value only when no settings file and no
`--setting` flag sets it.

## Scopes

Every section has a scope, which says how long its values live:

| Scope        | Meaning                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------- |
| `daemon`     | Fixed for the daemon's lifetime. A command that changes a value starts a new daemon.     |
| `invocation` | Read anew by every command. No daemon restart.                                           |
| `dice`       | Read anew by every command, and a changed value recomputes the parts of a build that read it. No daemon restart. |

When a command starts a new daemon because a `daemon` setting changed, it says
which one:

```
Buck settings changed (hydration.enable_paging: null -> true); restarting daemon...
```

## Where a setting may be overridden

The repository's `.bucksettings.toml` can set every setting. Each setting then
lists which of the other sources may override it: the local files, the command
line, both or neither.

Setting a key from a source it does not allow fails the command:

```
Buck setting `package_visibility.default_intersection` cannot be overridden from `--setting`
```

## Unknown keys

A settings file or a `--setting` flag that names a section or key Buck2 does
not know fails the command, as does a value of the wrong type:

```
Error parsing buck settings: unknown field `no_such_key`, expected one of `buster`, `idle_timeout_s`, `num_tokio_workers`, `macos_qos_class`
in `daemon`
```

This applies to older versions of Buck2 as well: a version from before a
setting existed rejects a file that sets it. Add a new setting to a checked-in
`.bucksettings.toml` only once every version of Buck2 used with the repository
knows it.

<FbInternalOnly>

## Rollouts

Rollouts set settings for some users, hosts or CI jobs without a change to the
repository. The Buck2 wrapper fetches the rollouts that apply and caches
them in `~/.bucksettings.rollouts`, which Buck2 reads as one more source.

- A rollout can set any setting in a `daemon` or `invocation` section, whatever
  the setting says about where it may be overridden. Rollouts do not reach
  `dice` sections.
- Every section has a version, and the cache holds values per section version.
  Buck2 takes the newest values at or below the version of the section it was
  built with, so a rollout can use a setting that older versions of Buck2 do
  not have.
- Nothing in the cache fails a command. A key Buck2 cannot use is skipped and
  reported in `buck2 audit settings`.

</FbInternalOnly>

## Seeing the settings in effect

`buck2 audit settings` prints the sources it considered and, for every setting,
its value and where the value came from:

```
Layers, lowest priority first:
  repository  /repo/.bucksettings.toml
  local       /home/user/.bucksettings.local.toml
  local       /repo/.bucksettings.local.toml  (absent)

[event_log]  invocation scope, version 0
  retained_logs = 20  (local /home/user/.bucksettings.local.toml)

[log_download]  daemon scope, version 0
  log_use_manifold = true  (repository)
  log_url = null           (unset)
```

- `--detailed` adds each setting's documentation, type, defaults and where it
  may be overridden, as the
  [Buck settings reference](../buck_settings_reference) lists them.
- `--json` prints the same report as JSON.
