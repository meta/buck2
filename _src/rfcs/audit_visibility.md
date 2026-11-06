# `buck2 audit visibility` command

## Context

Buck has a concept of Visibility for every target. It allows users to define,
for each target, the targets it can depend on and targets that can depend on it.
Visibility is specified as an allowlist of targets/target patterns, and any
target used that falls outside of the allowlist fails visibility checking.
Visibility pattern can be specified on `visibility` and `within_view` attributes
in buildfiles and
[PACKAGE files](https://www.internalfb.com/intern/wiki/Buck-users/Key_Concepts/Package_Files/).

Visibility is important to lots of codebase maintainers because it can be used
to keep projects from pulling in unwanted dependencies. As some examples, App
Core teams are using Buck visibility as a
[replacement to current supermodules for protecting app modularity](https://fb.prod.workplace.com/groups/2292177024436518/permalink/3112235492430663/).
Instagram's using visibility to
[protect modularity and define Link Groups used for build speed optimizations](https://fb.prod.workplace.com/groups/devx.build.bffs/posts/5169450219756775/?comment_id=5169500636418400).
There's interest from various DevX teams in using Buck visibility on
[PACKAGE files](https://www.internalfb.com/intern/wiki/Buck-users/Key_Concepts/Package_Files/)
to
[enforce repo boundaries, which will allow target determinators to migrate off of sparse profiles and onto Eden](https://fb.prod.workplace.com/groups/devx.build.bffs/posts/5169450219756775/),
although visibility in its current form is likely not fit for enforcing such
repo boundaries. Visibility has also been used to enforce
[requirements that only certain targets are allowed to depend on targets in fbcode/scripts](https://fb.workplace.com/groups/buckeng/permalink/4392940254087889/).

For perf reasons, buck2 doesn't always enforce visibility. Instead, it only
enforces visibility on construction of the configured target graph. Visibility
checking is expensive memory-wise because it requires tracking all deps at each
node. When constructing configured target graph, this cost is already paid for
when buck2 checks transitive target compatibility. When constructing the
unconfigured target graph, however, this is costly, so we avoid checking
visibility there. (Note that buck does not allow you to specify selects in
visibility attributes.)

In practice, this means that commands like `cquery` and `build` can enforce
visibility whereas commands like `uquery` and `targets` cannot. Having
visibility checked only on the configured target graph is problematic for 2
reasons:

1. Visibility is only checked on configured deps after selects are resolved, so
   it's possible for a target to pass visibility checking in one configuration
   but fail visibility checking in another. For example, a target may pass
   visibility checking on a linux configuration but fail visibility checking on
   mac configuration if it has a bad mac-only dependency. This makes visibility
   enforcement more difficult because now you have to query the same graph in
   both linux and mac configuration before you know that visibility is always
   valid.

2. Uquery (querying the unconfigured target graph) has better performance than
   cquery (querying the configured target graph). Big-O wise, uquery scales with
   O(# of targets) whereas cquery scales with O((# number of configurations) x
   (# of targets)). Having a way to check visibility on unconfigured target
   graph can be much cheaper than doing so on configured target graph.

## Proposed Solution: `audit visibility` command

It's clear that we need a way to check visibility on the unconfigured target
graph, but we don't want `buck2 uquery` and `buck2 targets` to regress in memory
use. To get the best of both worlds, I propose adding a separate command to
buck2, `buck2 audit visibility`, that will check visibility on the unconfigured
target graph. Instead of checking on construction of the unconfigured target
graph, this command will check after construction, which will avoid any memory
regression. The tradeoff is that the visibility checking won't be cached, and
rerunning `audit visibility` will rerun visibility checking on each invocation.

## Usage and Invocation

`buck2 audit visibility` command will take in a list of target patterns as well
as common build args like config flags and mode files as args. It will construct
the unconfigured target graph based on the **transitive deps** of those targets
and check that this graph has valid visibility. Checking transitive deps matches
the behavior of visibility checking on cquery, but we may revisit this decision
in the future if there is a need for just verifying the immediate dependencies.

For example, an invocation to check visibility on the transitive closure of
fbobjc can be

```shell
buck2 audit visibility fbsource//fbobjc/...
```

It cannot be used to check that a target has a valid visibility with respect to
targets outside of the transitive closure of its deps. For example,
`buck2 audit visibility fbcode//buck2/starlark-rust/starlark:starlark` will just
check that all transitive deps of `starlark` target (including `starlark`
target) have valid visibility with respect to each other. It will not check that
any targets that depend on `starlark` respect `starlark` target's visibility
attribute.
