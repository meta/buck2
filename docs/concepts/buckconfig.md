---
id: buckconfig
title: .buckconfig
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

The root of your [project](glossary.md#project) must contain a configuration
file named `.buckconfig`. Before executing, Buck2 reads this file to incorporate
any customizations it specifies.

`.buckconfig` holds the project's cells, the values that rules and macros read
with `read_config`, and those options of Buck2 itself that have not moved to
[Buck settings](buck_settings.md) yet. Options that have moved are set in
`.bucksettings.toml`; the [Buck settings reference](../buck_settings_reference)
names the buckconfig key each one still falls back to.

## Configuring a new project

A project that uses the prelude needs the cells, the bundled prelude, a default
target platform, and the execution platforms. `buck2 init` generates this
configuration:

```ini
[cells]
  root = .
  prelude = prelude
  toolchains = toolchains
  none = none

[cell_aliases]
  config = prelude
  ovr_config = prelude
  fbcode = none
  fbsource = none
  fbcode_macros = none
  buck = none

# Uses a copy of the prelude bundled with the buck2 binary. You can alternatively delete this
# section and vendor a copy of the prelude to the `prelude` directory of your project.
[external_cells]
  prelude = bundled

[parser]
  target_platform_detector_spec = target:root//...->prelude//platforms:default \
    target:prelude//...->prelude//platforms:default \
    target:toolchains//...->prelude//platforms:default

[build]
  execution_platforms = prelude//platforms:default
```

With `--git`, it also adds:

```ini
[project]
  ignore = .git
```

[Setting Up a New Project](../getting_started/new_project.md) goes through
this file section by section.

The cells and aliases are what the prelude expects to find; the `none` cell
exists only so that the prelude's references to Meta-internal cells resolve to
an empty cell. `[build] execution_platforms` is what the
`execution_platforms.target` [Buck setting](buck_settings.md) falls back to.
The `[project] ignore` entry keeps Buck2 from watching and listing the
version-control directory.

Beyond that, most projects go on to set:

- `[alias]`, for command-line shortcuts.
- `[buildfile]`, when the build files are not named `BUCK`.
- `[buck2_re_client]` and the `digest.algorithms`
  [Buck setting](buck_settings.md) (or the `[buck2] digest_algorithms` key it
  falls back to), to use a remote execution service; see
  [Remote Execution](../users/remote_execution.md).

The remaining keys Buck2 reads exist for debugging and tuning and do not need
to be set.

## The .buckconfig file uses the INI file format

The `.buckconfig` file uses the
[INI file format](http://en.wikipedia.org/wiki/INI_file). That is, it is divided
into _sections_ where each section contains a collection of key _names_ and key
_values_:

```ini
[section]
  key = value
  other_key = other value
```

Buck2's parser extends the format with [includes](#configuration-files-can-include-other-files)
and [references to other keys](#transclusion-of-values-from-one-key-to-another),
so INI parsers from other languages and libraries often cannot read Buck2's
configuration files. The rest of the format is simpler than most INI dialects,
as described below.

### Keys and values are taken literally

A line is `key = value`. Whitespace around the key and around the value is
removed; everything else in the value is kept as it is. There is no quoting and
there are no escape sequences: a `"` or a `\` in a value is part of the value
(except a `\` at the very end of a line, which continues the line).

```ini
[cxx]
  cxxppflags = -D MYMACRO="Buck"
```

gives `cxxppflags` the value `-D MYMACRO="Buck"`, quotes included.

A line that ends in `\` continues onto the next line. The `\` and the line
break are removed; whitespace before the `\` is kept and the next line's leading
whitespace is dropped, so `a \` followed by `  b` is the value `a b`:

```ini
[parser]
  target_platform_detector_spec = target:root//...->prelude//platforms:default \
    target:prelude//...->prelude//platforms:default
```

A blank line or a comment line ends the continuation.

### Comments

A line whose first non-blank character is `#` or `;` is a comment. A `#` later
on a line is part of the value, so a comment cannot follow a value on the same
line; the one exception is a section header, where ` #` (a space followed by
`#`) starts a comment that runs to the end of the line.

```ini
# A comment line.
; Also a comment line.
[alias] # A comment after a section header.
  app = //apps/myapp:app
```

### Lists

Buck2 does not define a list syntax; a value is a single string, and each key
that holds a list splits it in its own way. Most of the keys that Buck2 itself
reads as lists are comma-separated, and for most of those a space after a comma
becomes part of the next item: write `BUCK.v2,BUCK`, not `BUCK.v2, BUCK`.
`[parser] target_platform_detector_spec`, shown above, is separated by
whitespace instead. Keys read by Starlark code with `read_config` get the whole
string and split it however they like; the prelude's keys are mostly
whitespace-separated.

### Dot character not supported in section names

We do not support the use of the _dot_ character (`.`) in section names within
Buck2 configuration files. For example, the following is **not**
supported—_although Buck2 does not issue a warning or error_.

```ini
[foo.bar]
  baz=1
```

Be careful not to introduce the dot character when you define sections of your
own. This constraint is because Buck2 uses the dot character to delimit section
names and key names in other contexts such as the `--config` command-line
parameter.

## Transclusion of values from one key to another

Values from other keys can be transcluded into the current key using the
following syntax inside the current key value.

```
$(config <section>.<field>)
```

For example, to build one value from another:

```ini
[paths]
  tools = third-party/tools

[my_toolchain]
  compiler = $(config paths.tools)/bin/clang
```

A reference to a key that is not set becomes the empty string. References that
form a cycle are an error.

## .buckconfig.local

The root of your [project](glossary.md#project) may contain a second
configuration file named `.buckconfig.local`. Its format is the same as that of
`.buckconfig`, but values in `.buckconfig.local` override those in
`.buckconfig`. In practice, `.buckconfig` is a version-controlled file that
contains configuration that applies to all team members, whereas
`.buckconfig.local` is excluded from version control to allow users to define
personal configuration, such as personal aliases.

## Other initialization files

In addition to the `.buckconfig` and `.buckconfig.local` files in the project
root, Buck2 reads configuration from the following additional
locations, some of which are actually directories:

1. Directory `.buckconfig.d` located in the project root directory.
2. File `.buckconfig.local` and directory `.buckconfig.d` located in the
   current user's home directory which, on Unix-like systems, is available from
   the `HOME` environment variable or through the `~` symbol.
3. File `buckconfig` and directory `buckconfig.d` located in system directory
   `/etc/` (`C:\ProgramData\` on Windows).

Buck2 treats _any_ file—irrespective of name—in a
`.buckconfig.d`(`buckconfig.d`) directory, including files in its
subdirectories, as a Buck2 configuration file, provided that it adheres to
`.buckconfig` syntax. Symbolic links in these directories are skipped. For a
description of how Buck2 resolves collisions between values in these
configuration files, see the section
[**Precedence of Buck2 configuration specifications**](#precedence-of-buck2-configuration-specifications)
below.

Each [cell](key_concepts.md#cells) has its own configuration, read from the
`.buckconfig`, `.buckconfig.local` and `.buckconfig.d` at the cell's root. The
files in the home directory and in `/etc/` apply to every cell.

## Command-line control of configuration

In addition to the above configuration files, Buck2 supports specifying
additional configuration files from the Buck2 command line using the
`--config-file` parameter. You can also specify configuration values
_individually_ on the Buck2 command line using the `--config` (`-c`) parameter.
Furthermore, you can collect these flags into _flag files_ using the
`--flagfile` parameter. A flag file provides similar functionality to a
configuration file but uses a different syntax. Flag files are sometimes called
_mode files_ or _at_ (`@`) files.

A `--config` value applies to every cell; prefix it with a cell name to apply
it to one cell only: `-c mycell//section.key=value`. An empty value
(`-c section.key=`) unsets the key. `-c` cannot set keys of `[cells]`. A
`--config-file` always applies to every cell; a `cell//path` prefix only
resolves the path relative to that cell's root.

## Precedence of Buck2 configuration specifications

The following list shows the order of precedence for how Buck2 interprets its
configuration specifications. Values specified using a method closer to the
top of the list have higher precedence and will override those lower on the
list. For example, the `.buckconfig` file in the repo overrides a
`.buckconfig.local` file in the user's `HOME` directory.

1. Configuration specified on the command line using `--config` (`-c`),
   `--config-file` and `--flagfile`. Configuration specified later on the
   command line overrides configuration specified earlier.
1. `.buckconfig.local` in the repo.
1. `.buckconfig` in the repo.
1. Files in a `.buckconfig.d` folder of the repo.
1. `.buckconfig.local` in user's `HOME` directory.
1. Files in a `.buckconfig.d` folder in user's `HOME` directory.
1. The global file `/etc/buckconfig`
1. Files in the global directory `/etc/buckconfig.d`

Files in a `.buckconfig.d` (`buckconfig.d`) directory have precedence according
to the lexicographical order of their file names. Files _later_ in the
lexicographical order have precedence over files earlier in that order.

## Configuration files can include other files

`.buckconfig`, `.buckconfig.local`, the files in the project's `.buckconfig.d`
and `--config-file` files can include by reference other files that contain
configuration information; includes in the files in your home directory and in
`/etc/` are not followed. These included files can contain complete
`.buckconfig` sections or they can contain a group of key name/value pairs that
constitute part of a section. In this second use case, you'll need to ensure
that the _included_ file is referenced beneath the appropriate section in the
_including_ file. Because of this additional complexity, we recommend that you
include only files that contain complete sections. The syntax to include a file
is

```
<file:path-to-included-file>
```

where _path-to-included-file_ is either a relative path from the including file
(recommended) or an absolute path from the root of the file system. You can also
specify that the file should be included only if it exists by prefixing with a
question mark (`?`).

```
<?file:path-to-included-file>
```

If you use this prefix, it is not an error condition if the file does not exist;
Buck2 just silently continues to process the rest of the configuration file. In
the following example, the `.buckconfig` file includes the file
`cxx-other-platform.include` which exists in the subdirectory
`cxx-other-platform`. The `.buckconfig` file will also include the file
`future-platform.include` from the directory `future-platform` if that file
exists.

```ini
#
# .buckconfig
#
[cxx]
  cxxppflags = -D MYMACRO=Buck

<file:cxx-other-platform/cxx-other-platform.include>

<?file:future-platform/future-platform.include>
#
# cxx-other-platform.include
#
[cxx]
  cxxflags = -Wall
```

After an include, the including file continues in the section the included file
ended in, so put a section header after each include.

Some reads do not follow includes. Set these in `.buckconfig`,
`.buckconfig.local` or a `.buckconfig.d` file directly:

- `[cells]` and `[external_cells]`.
- The keys that start a new daemon when they change. Among them, the
  `[buck2_re_client]` keys are an exception: the daemon uses a value that an
  include sets, but a change to it there does not start a new daemon.
- Some of the keys Buck2 reads for each command, such as `build.threads` and
  `ui.thread_line_limit`.

A cell alias defined in an included file works in build files and target
patterns but not in `-c <alias>//...`. `buck2 audit config` shows included
values even where Buck2 does not use them.

## When a change takes effect

Most keys are read for every command, so a change in a file or on the command
line (`-c section.key=value`) applies to the next command. Reading a key through
Buck2's incremental computation also records it as a dependency, so changing it
invalidates the work that read it: Buck2 rebuilds whatever depended on the old
value, which can cost build time.

Some keys configure the daemon process itself and are read once when it starts:
a change to some of them starts a new daemon on the next command, and a change
to the others is picked up only after the daemon is killed (`buck2 kill`) or
exits on its own. Buck2 reads both kinds from the configuration files only:
`-c` and `--config-file` do not reach them. See [Daemon](daemon.md).

## Sections

This page describes the sections Buck2 reads as a whole, and
`[project] ignore`. Buck2 reads the keys of other sections (`[build]`,
`[buck2]`, `[buck2_re_client]`, ...) one at a time; those keys are not listed
here.

These are not all the keys a `.buckconfig` can hold: Starlark code reads
arbitrary keys with `read_config` and `read_root_config`, so toolchains and
rules (including the prelude's `[cxx]`, `[python]` and similar sections) define
keys of their own, and `config_setting` rules match arbitrary keys named in
their `values` and `root_values` arguments. For more on configuration matching,
see [Configurations](configurations.md#configuration-values). For more on
`read_config` in `PACKAGE` files, see [Packages](../rule_authors/package.md).

Unless a section says otherwise, Buck2 reads its keys from the root cell's
configuration; the same key in another cell's `.buckconfig` is ignored.
`[cell_aliases]`, `[buildfile]` and `[project] ignore` are read from each
cell's own configuration.

## [alias]

This section contains definitions of [build target](build_target.md) aliases.

```ini
[alias]
  app = //apps/myapp:app
  apptest = //apps/myapp:test
```

These aliases can then be used from the command line:

```sh
$ buck2 build app
$ buck2 test apptest
```

For more on target aliases, see
[Target aliases](target_pattern.md#target-aliases).

## [cells]

Lists the cells that constitute the Buck2 project. Buck2 builds that are part of
this project—that is, which use this `.buckconfig`—can access the cells
specified in this section.

```ini
[cells]
    buck = .
    bazel_skylib = third-party/skylark/bazel-skylib
```

The string on the left-hand side of the equals sign is the _alias_ for the cell.
The string on the right-hand side of the equals sign is the normalized path to
the cell relative to the project root, or `.` for the project root itself. Two
entries with the same path are one cell with two names. The root cell must be
listed, with the path `.`, and only the root cell's `[cells]` is read.

You can view the contents of this section using the `buck2 audit cell` command.

For more on cells, see [Key Concepts](key_concepts.md#cells).

`[repositories]` is additionally supported as a deprecated alternative name for
this section.

## [cell_aliases]

Defines additional names for cells, so that `app//foo:bar` below refers to
the same cell as `root//foo:bar`. The target of an alias is a name from
`[cells]`. The root cell's aliases are available in every cell; another cell's
`[cell_aliases]` add to them, and override them, for the build files in that
cell.

```ini
[cell_aliases]
    app = root
```

`[repository_aliases]` is additionally supported as a deprecated alternative
name for this section.

## [external_cells]

Declares cells whose contents Buck2 fetches instead of reading them from the
repository. The value is the cell's origin: `bundled` uses a copy of the cell
shipped inside the `buck2` binary (available for `prelude`), `git` checks out a
commit from a Git repository, and `disabled` treats the entry as an ordinary
cell. The cell still needs a path in `[cells]`, and no other cell may be nested
inside it.

```ini
[external_cells]
    toolchains = git

[external_cell_toolchains]
    git_origin = https://example.com/toolchains.git
    commit_hash = 0123456789abcdef0123456789abcdef01234567
```

A `git` cell reads its details from the `[external_cell_<cell>]` section:

| Key             | Description                                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------------ |
| `git_origin`    | URL of the Git repository.                                                                                   |
| `commit_hash`   | Full hash of the commit to check out.                                                                        |
| `object_format` | Hash function of the repository, `sha1` or `sha256`. Defaults to validating `commit_hash` as a SHA-1 hash.  |

For the full explanation and limitations, see
[External Cells](../users/advanced/external_cells.md).

## [project]

### ignore

A comma-separated list of patterns, relative to the cell, for paths that Buck2
treats as absent: they are left out of directory listings and globs, changes to
them are not noticed, and reading one that is named explicitly is refused. A
pattern without glob characters names a directory and covers everything under
it. One with glob characters is a glob matched against the whole path relative
to the cell, in which `*` and `?` do not cross `/` and `**` matches any number
of directories: `*.o` ignores only files at the top of the cell, and `**/*.o`
those at every depth. `buck-out` is always ignored in the root cell.

Buck2 reads `ignore` when the daemon starts, and neither `-c` nor
`--config-file` can set it. After a change to the root cell's `ignore`, commands
fail until you run `buck2 kill`; a change to another cell's takes effect only
after `buck2 kill`.

A pattern starting with `!` un-ignores the paths it matches, whatever other
patterns say about them, and only those paths: under a directory that stays
ignored, everything stays ignored.

```ini
[project]
  ignore = \
    **/*.xcodeproj, \
    **/*.xcodeproj/**, \
    !apps/*/*.xcodeproj, \
    !apps/*/*.xcodeproj/project.pbxproj
```

Here every Xcode project is ignored, except that the project files of those
directly under `apps` are not. Those directories are un-ignored too, since a
file under an ignored directory could not be. Everything else in them, such as
the state Xcode writes while a project is open, stays ignored.

## [buck2_metadata]

Every key in this section is attached to the command's events as metadata, so a
project can add its own columns to whatever consumes its event logs. Values may
contain `$VAR` references to environment variables; an entry whose variable is
unset is skipped. Only open source builds of Buck2 read this section, when the
daemon starts.

<FbInternalOnly>

## [experiments]

Keys of this section in the wrapper's experiments file are recorded as the
command's enabled experiments.

</FbInternalOnly>
