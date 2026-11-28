---
id: why
title: Why Buck2
---

Buck2 is a build system from Meta. This page answers the questions:
[why does Buck2 exist](#why-does-buck2-exist),
[what's different about Buck2](#whats-different-about-buck2), and
[why use Buck2](#why-use-buck2).

## Why does Buck2 exist?

Meta employs a very large monorepo, consisting of a variety of programming
languages, including C++, Python, Rust, Kotlin, Swift, Objective-C, Haskell,
OCaml, and more. Google employs a different but functionally similar monorepo.

These large scale and multi-language repositories are generally beyond the
capabilities of traditional build systems like `make`. To optimize the build and
performance of these large systems, Meta (then called Facebook) and Google developed their own
build systems, respectively Buck and Bazel. While the internal version of Bazel
was started first (also known as Blaze), Buck was open sourced first (back in
March 2013), followed by Bazel a few years later (March 2015).

The retroactively named Buck1 was a capable build system, but had significant
limitations and has been entirely phased out at Meta today. Buck2 is a rewrite
that aims to keep the best bits of Buck1 (with a high degree of target
compatibility) but also borrows ideas from
[academic](https://ndmitchell.com/#shake_10_sep_2012)
[research](https://ndmitchell.com/#shake_21_apr_2020) and build systems,
including [Bazel](https://bazel.build/), [Pants](https://www.pantsbuild.org/),
[Shake](https://shakebuild.com/), [Tup](https://gittup.org/tup/), and more.

Following are aspects common to Buck1 and Buck2 (and in most cases, Bazel):

- **Targets that can be queried** - the build is defined as a series of targets,
  specified in `BUCK` files, that depend on other targets. This graph of targets
  can be queried to understand how they relate to each other and what the
  potential impact of a change might be.
- **Remote execution** - the build can send actions to a set of remote servers
  to be executed, increasing the parallelism significantly.
- **Multi-language composability** - there can be lots of different languages in
  a single build, and they can be put together. For example, you could have a
  Python library that depends on a Rust library, which, in turn depends on a C
  library.
- **File watching** - at large enough scale, simply looking for changed files is
  prohibitively expensive. Buck can integrate with
  [Watchman](https://facebook.github.io/watchman/) to discover which files have
  changed efficiently. However, for simplicity of setup, the open-source version
  defaults to using `inotify` or similar functionality.
- **Uses Starlark** - Starlark is a deterministic Python-like language used to
  specify the targets, enabling the definition of targets as literals and more
  advanced manipulation/sharing.

## What's different about Buck2?

Buck2 has several major differences (as well as many minor differences) from
Buck1. Of particular note, there are a number that give new efficiency or
expressiveness (most of these are also different from Bazel).

- **Buck2 is written in Rust** - Buck1 was written in Java. One of the
  advantages of using Rust is the absence of GC pauses, However, Java also has
  advantages, such as better memory profiling tools.
- **Buck2 is remote execution first** - local execution is considered a special
  case of remote execution, in contrast to Buck1 where it was added after. That
  means that things such as directory hashes can be pre-computed ready to send
  to remote execution, giving efficiency benefits.
- **All Buck2 rules are written in Starlark** - whereas, in Buck1, they were
  written in Java as part of the binary, which makes iteration on rules much
  faster.
- **The Buck2 binary is entirely language agnostic** - as a consequence of
  having all the rules external to the binary, the most important and complex
  rule (such as in C++), don't have access to magic internal features. As a
  result, features have been made available to all rules, including:
  - [Dep files](../rule_authors/dep_files.md) - the ability to declare that a
    subset of the files weren't actually used, and thus not be sensitive to
    changes within them.
  - [Incremental actions](../rule_authors/incremental_actions.md) - the ability
    to have the action short-circuit some subset of the work if run again.
- **Buck2 uses a dynamic (aka monadic) graph as its underlying computation
  engine** - while most dependencies are specified statically, there are two
  particular features that expose dynamic power to rule authors:
  - [Dynamic dependencies](../rule_authors/dynamic_dependencies.md) - enable
    rules to build a file then look at its contents before specifying the
    dependencies and steps in future actions. Common uses are languages where
    the dependency structure within a project must follow imports (e.g. Haskell,
    OCaml) and distributed ThinLTO (where the best optimization plan is
    generated from summaries).
  - [Anonymous targets](../rule_authors/anon_targets.md) - enable rules to
    create a graph that has more sharing than the original user graph. As a
    result, two unrelated binaries can compile shared code only once, despite
    the shared code not knowing about this commonality. This feature is useful
    for rules like Swift feature resolution.
- **[Transitive-sets](../rule_authors/transitive_sets.md)** - similar in purpose
  to Bazel's [depset](https://bazel.build/rules/lib/depset). But, instead of
  being just a memory optimization, are also wired into the dependency graph,
  providing a reduction in the size of the dependency graph.
- **Buck2 is not phased** - there are no target graph/action graph phases, just
  a series of dependencies in a
  [single graph on DICE](https://github.com/facebook/buck2/blob/main/dice/dice/docs/index.md)
  that result in whatever the user requested. That means that Buck2 can
  sometimes parallelise different phases and track changes very precisely.
- **Buck2 can integrate with the virtual filesystem
  [Eden](https://github.com/facebook/sapling)** - this provides good
  performance, even when the file system is backed by source control fetches.
  However, Eden is not required, and a normal file system will also work well.
- **The Buck2 Starlark implementation is available
  [as a standalone library](https://developers.facebook.com/blog/post/2021/04/08/rust-starlark-library/)** -
  this provides features such as IDE integration (both LSP and DAP bindings),
  linters, typecheckers, and more. These features are integrated into Buck2 to
  give a better developer experience (which is still evolving).
- **Buck2 supports configurations** - (such as `select`) to provide
  multi-platform/architecture builds, which are heavily inspired by Bazel.
  Within that space, there is a number of small differences, such as
  `toolchain_deps`.
- **Buck2 is fast** - in our internal tests, we observed that Buck2 completed
  builds 2x as fast as Buck1.

For a comprehensive list of benefits, see
[Benefits Compared to Buck1](benefits/compared_to_buck1.md).

## Why use Buck2?

It would be delightful if you tried out Buck2! But it is early-stage software,
so users may run into unexpected issues. If you encounter an issue, please
report it via [Github issues](https://github.com/facebook/buck2/issues).

Buck2 is being used internally within Meta and is available as open-source
from 2023.

There are several differences between the internal and open-source versions:

- Meta uses an internal version of remote execution with builds always hooked up
  to remote execution. The open-source binding, which uses Buck2 without remote
  execution, may be less polished.
- There are some configuration differences between the open source and internal
  versions. For example, file changes default to `inotify` in open-source, and
  to Watchman internally.
- The prelude (containing all the rules) is the same for open-source as
  internal, but toolchains are not open-sourced. The required custom toolchains
  may not work as well.

There are also some things that aren't quite yet finished:

- There are not yet mechanisms to build in release mode (that should be achieved
  by modifying the toolchain).

If none of that puts you off, [give Buck2 a go](../getting_started/index.md)!
