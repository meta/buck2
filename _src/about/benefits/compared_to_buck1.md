---
id: compared_to_buck1
title: Benefits When Compared to Buck1
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

## Benefits for end users

> _"`buck2 build SOME_TARGET_I_ALREADY_BUILT_BEFORE` is basically instantaneous
> and is a super delightful experience. 🙂"_ - End user experience
> <FbInternalOnly>([source](https://fb.prod.workplace.com/groups/buck2users/posts/3030704467185914))</FbInternalOnly>

> _"Buck2 is largely faster and more memory efficient than buck1, and where I’ve
> seen counter-examples, the buck2 team quickly optimizes and fixes that.🙂"_ -
> Software Engineer feedback
> <FbInternalOnly>([source](https://fb.prod.workplace.com/groups/devx.ci.bffs/posts/616830502778501))</FbInternalOnly>

For people who use Buck on a daily basis (such as using Buck build as part of
their development inner loop), switching to Buck2 provides the following
benefits:

- **Performance** - the performance of Buck2 is better in four ways:
  - **_Fast things are fast_** - in Buck1, simply typing `buck build` when there
    is nothing to do can be expensive (23 seconds in some benchmarks). In Buck2,
    the same build action takes 0.1 seconds. Actions that should be fast are
    fast, which enables developers to use Buck more freely, without trying to
    work around the build system.
  - **_Slow things are faster_** - when there is real work to do, Buck2 is
    significantly closer to the critical path. Benchmarks range from 5%/10s
    faster for changing a header file (lots of parallel C++ computations, Buck1
    already nearly at the critical path) to 42%/145s faster (changing a Thrift
    file in a large project).
  - **_Users contribute to the shared cache_** - with Buck1, only trusted CI
    builds write to the network cache, while with Buck2 everyone writes to the
    cache through sandboxed remote execution. This increases the chance of cache
    hits, saving capacity and time.
  - **_CI builds go faster_** - these numbers vary day by day, but most projects
    are 2-4x faster. This means spending less time waiting for CI and saving
    some capacity at the same time.
- **Correctness** - in Buck2, rules are hermetic by default. Missing
  dependencies are errors. These restrictions apply to both the user-written
  `BUCK` files and the language rules.
  - During the process of migrating to Buck2, a huge number of missing
    dependencies have been fixed. However, during the same process, several
    Buck1 issues were identified that are not going to be fixed in Buck1 (such
    as missing headers, genrules without dependencies, and OCaml rules don’t
    track all deps). The end result is that Buck2 gives the right answer more
    often, cutting down on user surprises.
- **Rule features** - the rules in Buck2, especially for less commonly used
  languages (such as Haskell, OCaml, and Rust) support features above and beyond
  those in Buck1.
  - Examples: dependencies can be given as arguments to
    `prebuilt_ocaml_library`, Haskell enables the use of stub headers from C++,
    and Rust has experimental pipelining support.
- **Actively developed** - the Meta build team is putting all its efforts behind
  Buck2; it's vastly easier to develop than Buck1. While Buck2 is already ahead
  of Buck1 in many important aspects, the difference is only going to grow with
  several improvements in the pipeline.
- **Support** - Meta can provide much better support to those having
  difficulties with Buck2 than to those using Buck1.

## Benefits for Rule Authors

If you write language-specific rules, then Buck2 is in a different league to
Buck1.

> _"This is all rather fun! Buck2 rules are so much more hackable than
> Buck1."_ - Software Engineer feedback <FbInternalOnly>
> ([source](https://fb.prod.workplace.com/groups/333784157210625/posts/928214407767594))</FbInternalOnly>

There are a number of reasons why Buck2 excels for Rule Authors:

- **Faster developer cycle** - in Buck1, the time from changing a rule to seeing
  the impact is many minutes: you first have to compile Buck1, invalidate the
  dependency cache (and so on), and perhaps work between multiple OSs. With
  Buck2, it takes seconds, you don’t even need to restart the daemon.
- **Simple API** - Buck2 rules use a small and documented Starlark API, which is
  dependency-correct by construction. In Buck1, the rules must obey a lot of
  subtle side conditions with a much larger API.
- **Easier deployment** - for Buck2, deployment is just checking the rules in,
  with an atomic commit changing associated macros (when required). For Buck1,
  you have to make the repo work with the old and new rules and wait for a Buck
  version bump to ship your changes, perhaps a few days later.
- **Low barrier to entry** - writing rules in Buck2 is vastly easier than Buck1,
  significantly increasing the developer pool. This means that writing rules is
  now accessible to language experts, not just Buck experts.

## Benefits for Integrators

For those people who integrate Buck2 into larger systems, in addition to many of
the above benefits apply, Buck2 provides the following benefits:

- **Faster queries** - many integrators make extensive use of `buck uquery` and
  `cquery`. In Buck2, these commands are **faster** and use **less memory**.
  - For example, on CI target determination (a bunch of targets/queries), Buck2
    is 25% faster at P50 (moving to 40% faster at P95) with 25% less memory
    (saving over 20Gb, and crossing below the 64Gb threshold).
- **Profiling** - Buck2 already ships with five types of profiling for both
  loading and analysis (flame graphs, statement breakdown, heap profiles etc).
  With Buck2, these tools are much more easily accessible to people not on the
  Build Infra team.

<FbInternalOnly>

- **Eden friendly** - Buck2 is tuned for the Eden architecture, performing fewer
  disk operations with greater parallelism.
  - For example, the slowdown caused by using Eden for `targets` on `fbandroid`
    is
    [reduced from 300s to 80s](https://fb.workplace.com/groups/132499338763090/posts/132580122088345).
- **Better observability** - Buck2 populates many Scuba tables with information
  about
  [loading](https://www.internalfb.com/intern/scuba/query/?dataset=buck2_loads),
  [analysis](https://www.internalfb.com/intern/scuba/query/?dataset=buck2_analyses),
  [builds](https://www.internalfb.com/intern/scuba/query/?dataset=buck2_builds)
  and
  [errors](https://www.internalfb.com/intern/scuba/query/?dataset=buck2_action_errors),
  and more. The architecture of Buck2 ensures that all important information can
  be recorded in a uniform manner, enabling sensible trade-offs to be made about
  what to store vs for how long.

</FbInternalOnly>

## The downside

While there are many benefits, it would be remiss not to include a small list of
temporary issues:

- **Stability** - Buck2 is under active development, which means the risk of
  regression is correspondingly higher. There may be issues, but they will be
  fixed as quickly as possible (and lessons learned) through the through Meta's
  SEV-review process.
- **Corner cases** - Buck1 has been battle-tested for nearly a decade, which has
  included attention to events such as error messages in unlikely corner cases.
  Only time and user feedback will enable Meta to bring Buck2 to the same level.
  Please share all such feedback!

<FbInternalOnly>

- **Buck2 Web UI** - there isn’t yet a working Web UI equivalent to the one
  provided by Buck1. But we’re working on it and hope to share an initial
  version shortly.

</FbInternalOnly>
