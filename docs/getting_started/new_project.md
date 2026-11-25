---
id: new_project
title: Setting Up a New Project
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

This page describes what a Buck2 project is made of: what each file that
`buck2 init` creates is for, and what to change before a real codebase depends
on it.

<FbInternalOnly>

Meta's repositories are already set up. This page is for projects outside Meta.

</FbInternalOnly>

## Create the project

Run `buck2 init` in the root of your repository, or `buck2 init <dir>` to create
a directory for the project:

```sh
buck2 init
```

It creates:

| File              | Purpose                                                             |
| ----------------- | ------------------------------------------------------------------- |
| `.buckconfig`     | The project's configuration: its cells, its prelude, its platforms. |
| `.buckroot`       | An empty file that marks the project root.                          |
| `toolchains/BUCK` | The toolchains: which compilers and tools the rules use.            |
| `BUCK`            | An example target. Delete it once you have targets of your own.     |

`buck2 build //...` works at this point, and builds the example target.

Buck2 writes into the directory `buck-out`, which belongs in `.gitignore`. For
a directory that is not a git repository yet, `buck2 init --git` does that: it
runs `git init`, writes a `.gitignore` with `/buck-out`, and adds the
`[project]` section shown below to `.buckconfig`. In a repository that exists
already, add both yourself.

## What `.buckconfig` sets up

The generated [`.buckconfig`](../concepts/buckconfig.md) is short:

```ini
[cells]
  root = .
  prelude = prelude
  toolchains = toolchains
  none = none
```

A [cell](../concepts/key_concepts.md#cells) is a directory tree with a name.
The name is the part of a target label before `//`: `root//lib:greet` is the
target `greet` in the directory `lib` of the cell `root`. A label that starts
with `//` is in the cell of the file it is written in, so within your own code
you rarely write `root`. Your code lives in `root`. The rules come from
`prelude`, and the toolchains from `toolchains`. `none` is a cell with nothing
in it.

```ini
[cell_aliases]
  config = prelude
  ovr_config = prelude
  fbcode = none
  fbsource = none
  fbcode_macros = none
  buck = none
```

The prelude refers to a few cells by these names. The aliases point each of
them at the prelude or at the empty cell. Leave this section as it is.

```ini
[external_cells]
  prelude = bundled
```

The [prelude](../concepts/glossary.md#prelude) is the set of rules that comes
with Buck2: `cxx_library`, `rust_binary`, `genrule` and the rest of the
[rule reference](../../prelude/rules). `bundled` uses the copy inside the
`buck2` binary, so there is no `prelude` directory in your repository, and the
rules change when the binary does. [Pin the Buck2 version](#pin-the-buck2-version)
so that they change when you decide. To read or edit the rules instead, see
[Keeping the prelude in your repository](#keeping-the-prelude-in-your-repository).

```ini
[parser]
  target_platform_detector_spec = target:root//...->prelude//platforms:default \
    target:prelude//...->prelude//platforms:default \
    target:toolchains//...->prelude//platforms:default

[build]
  execution_platforms = prelude//platforms:default
```

Every target is built for a [platform](../concepts/configurations.md). The
first key says which platform a target is built for when nothing else chooses
one, and the second where build commands run. `prelude//platforms:default`
describes the machine Buck2 runs on, which is what a project that does not
cross-compile or use [remote execution](../users/remote_execution.md) needs.

```ini
[project]
  ignore = .git
```

`buck2 init --git` adds this section. Buck2 does not look for targets in
ignored directories and does not watch them for changes.

## The project root and `.buckroot`

Buck2 commands work from any directory of the project. To find the project
root, Buck2 walks up from the current directory and takes the outermost
directory that has a `.buckconfig`. It stops walking at a directory that has a
`.buckroot`.

Without `.buckroot`, a project checked out somewhere below another directory
with a `.buckconfig` is taken for a part of that outer project. Keep the file,
and commit it.

```sh
buck2 root --kind project
```

prints the root that Buck2 found.

## Toolchains

A rule does not know where its compiler is. It asks a
[toolchain](../concepts/toolchain.md), which is a target in the `toolchains`
cell with a name the prelude expects: `toolchains//:cxx` for the C and C++
rules, `toolchains//:rust` for the Rust rules, and so on.

The generated `toolchains/BUCK` defines all of them at once:

```python
load("@prelude//toolchains:demo.bzl", "system_demo_toolchains")

system_demo_toolchains()
```

Most of these toolchains run whatever tools are on `PATH`: `clang` and `lld` for
C++ on Linux, `rustc` for Rust. That is enough to try Buck2. A project should
say which tools it uses, and with which flags, by defining the toolchains for
its languages itself. This `toolchains/BUCK` is for a project with C++ and Rust
code, compiled with the system's GCC and `rustc`:

```python
load("@prelude//tests:test_toolchain.bzl", "noop_test_toolchain")
load("@prelude//toolchains:cxx.bzl", "system_cxx_toolchain")
load("@prelude//toolchains:genrule.bzl", "system_genrule_toolchain")
load("@prelude//toolchains:python.bzl", "system_python_bootstrap_toolchain")
load("@prelude//toolchains:remote_test_execution.bzl", "remote_test_execution_toolchain")
load("@prelude//toolchains:rust.bzl", "system_rust_toolchain")

system_cxx_toolchain(
    name = "cxx",
    compiler = "gcc",
    compiler_type = "gcc",
    cxx_compiler = "g++",
    cxx_flags = ["-std=c++20", "-Wall"],
    linker = "g++",
    visibility = ["PUBLIC"],
)

# In Buck2 releases up to 2026-11-15, the rules that use the C++ toolchain also
# refer to a toolchain of this name, and `buck2 uquery` follows the reference.
toolchain_alias(
    name = "cxx_no_default_deps",
    actual = ":cxx",
    visibility = ["PUBLIC"],
)

system_rust_toolchain(
    name = "rust",
    default_edition = "2024",
    visibility = ["PUBLIC"],
)

# The prelude's own build scripts are run with this Python.
system_python_bootstrap_toolchain(
    name = "python_bootstrap",
    visibility = ["PUBLIC"],
)

system_genrule_toolchain(
    name = "genrule",
    visibility = ["PUBLIC"],
)

# Test rules need these two.
noop_test_toolchain(
    name = "test",
    visibility = ["PUBLIC"],
)

remote_test_execution_toolchain(
    name = "remote_test_execution",
    visibility = ["PUBLIC"],
)
```

A rule whose toolchain is missing fails with
``Unknown target `rust` from package `toolchains//` ``. The name in the message
is the toolchain to add; `system_demo_toolchains` in the prelude's
`toolchains/demo.bzl` shows how each one is defined.

These "system" toolchains still take their tools from `PATH`, so two machines
with different compilers build different things. A toolchain can instead
download its tools as part of the build. See
[Writing Toolchains](../rule_authors/writing_toolchains.md), and the
[toolchain examples](https://github.com/meta/buck2/tree/main/examples/toolchains)
in the Buck2 repository.

## Enable configuration modifiers

[Modifiers](../concepts/modifiers.md) are how a build is switched between
variants such as debug and release: `buck2 build //app:server -m release`. They
are off until the project enables them in a `PACKAGE` file in its root, and
until then a modifier on the command line is an error.
[Set up configuration modifiers](../users/how_tos/modifiers_setup.md) has the
file, and goes on from there to a working `-m release`.

The same root [`PACKAGE` file](../concepts/package_files.md) is where
project-wide defaults go, such as the default visibility of targets.

## Pin the Buck2 version

With the bundled prelude, the version of `buck2` decides what the rules do. So
everyone who builds the project, and its CI, should run the same `buck2`.

Each dated [release](https://github.com/meta/buck2/releases), such as
`2026-11-15`, has a [DotSlash](https://dotslash-cli.com) file named `buck2`.
It is a small text file that names the release's binary for each operating
system and CPU. Commit it to the root of the repository:

```sh
curl -L -o buck2 https://github.com/meta/buck2/releases/download/2026-11-15/buck2
chmod +x buck2
```

Then run Buck2 as `./buck2`. With DotSlash
[installed](https://dotslash-cli.com/docs/installation/), the first run
downloads the right binary and checks its hash, and later runs start it from a
cache. To move the project to another release, replace the file.

The `latest` release follows the main branch and has no DotSlash file.

## Keeping the prelude in your repository

There are two alternatives to the bundled prelude. With either, the prelude no
longer changes with the binary. A prelude and a `buck2` from different releases
are not guaranteed to work together: each release has a `prelude_hash` file
with the commit of [buck2-prelude](https://github.com/meta/buck2-prelude) that
matches it.

To fetch the prelude from git, at that commit:

```ini
[external_cells]
  prelude = git

[external_cell_prelude]
  git_origin = https://github.com/meta/buck2-prelude.git
  commit_hash = <contents of prelude_hash>
```

To have the prelude's files in the repository, where they can be read and
changed, run `buck2 expand-external-cell prelude`, which writes the prelude into
the `prelude` directory, and then delete the `prelude = bundled` line.

[External Cells](../users/advanced/external_cells.md) describes both in full.

## Settings for one checkout

`.buckconfig.local`, next to `.buckconfig`, has the same format and overrides
it. It is for values that should apply to one checkout only, so add it to
`.gitignore`.

## Where to go next

- [Build File](../concepts/build_file.md) and
  [Visibility](../concepts/visibility.md): how targets are declared, and which
  targets may depend on which.
- [`PACKAGE` files](../concepts/package_files.md): defaults for a whole
  directory tree.
- [Configurations](../concepts/configurations.md): building the same target in
  more than one way.
- The [rule reference](../../prelude/rules), and the
  [examples](https://github.com/meta/buck2/tree/main/examples/with_prelude)
  of each language in the Buck2 repository.
- [Remote Execution](../users/remote_execution.md): running build commands on
  other machines, and sharing a cache.
- [Writing Rules](../rule_authors/writing_rules.md): when the prelude has no
  rule for what you build.
