---
id: buck_hanging
title: Why is Buck2 hanging?
---

import { FbInternalOnly, OssOnly } from
'docusaurus-plugin-internaldocs-fb/internal';

Let's look at how to troubleshoot when buck2 hangs, i.e. it just sits there
saying "Jobs: In progress: 0, ..." but it’s not finishing...

When buck2 hangs, there are two possibilities: It’s either hanging doing
_something_, or it’s hanging doing _nothing_. The first thing you should do is
figure out which of those is happening. That’s because the tools to debug either
of those are _very_ different! We will mainly focus on the first in this case.

To figure out which hang you have on your hands, just look at how much CPU buck2
is using when the hang occurs using your favorite activity monitor (e.g. `top`,
`htop`). Remember that you can find the buck2 daemon’s PID using `buck2 status`.
Ideally, break the utilization down by threads (in top, that’s `top -Hp $PID`).

If any thread is using 100% CPU for some period of time, then you probably have
a busy hang (buck2 is doing “something”) which are usually easier to debug.

## How to debug a “busy” hang

### Getting a stack trace

When debugging a busy hang, the first thing to do is to work out what the
process is doing. There are many tools you can use for this (like a profiler),
but the absolute simplest one is quickstack: just run `quickstack -p $PID`, and
it’ll show you a stack dump for all the threads in your process. If you prefer
`gdb`, you can use `gdb -p $PID`, then `thread apply all bt`, and that’s the
same thing.

Note that a stack trace tells you what the process is doing at a point in time,
so don’t just look at the very last frame and call it the culprit. Instead, look
at the stack as a whole. If you need more perspective, use a sampling profiler
<FbInternalOnly>(strobeclient run --pid $PID)</FbInternalOnly>. You can also
just grab stack traces at a few points in time and see if they look similar:
this is exactly what a sampling profiler does, albeit at a higher frequency.

### Interpreting the stack trace

Let's consider an example user report <FbInternalOnly>( see
[here](https://fb.workplace.com/groups/buck2users/permalink/3232782826978076/))</FbInternalOnly>
with the following stack trace:

```
#01  0x0000000005b1ec26 in <buck2_build_api::artifact_groups::artifact_group_values::TransitiveSetIterator<buck2_build_api::artifact_groups::artifact_group_values::ArtifactGroupValues, (buck2_build_api::actions::artifact::Artifact, buck2_execute::artifact_value::ArtifactValue), buck2_build_api::artifact_groups::artifact_group_values::ArtifactValueIdentity> as core::iter::traits::iterator::Iterator>::next () from ...
#02  0x0000000005b23998 in <buck2_build_api::artifact_groups::artifact_group_values::TransitiveSetIterator<buck2_build_api::artifact_groups::artifact_group_values::ArtifactGroupValues, (buck2_build_api::actions::artifact::Artifact, buck2_execute::artifact_value::ArtifactValue), buck2_build_api::artifact_groups::artifact_group_values::ArtifactValueIdentity> as itertools::Itertools>::exactly_one () from ...
#03  0x00000000059dbb2c in buck2_server_commands::commands::build::create_unhashed_outputs () from ...
#04  0x0000000005c3c677 in <core::future::from_generator::GenFuture<<buck2_server_commands::commands::build::BuildServerCommand as buck2_server_ctx::template::ServerCommandTemplate>::command::{closure#0}> as core::future::future::Future>::poll () from ...
#05  0x00000000054c58a3 in <core::future::from_generator::GenFuture<<alloc::boxed::Box<dyn buck2_server_ctx::ctx::ServerCommandContextTrait> as buck2_server_ctx::ctx::ServerCommandDiceContext>::with_dice_ctx<buck2_server_ctx::template::run_server_command<buck2_server_commands::commands::build::BuildServerCommand>::{closure#0}::{closure#0}::{closure#0}, core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output = core::result::Result<cli_proto::BuildResponse, anyhow::Error>> + core::marker::Send>>, cli_proto::BuildResponse>::{closure#0}> as core::future::future::Future>::poll () from ...
#06  0x00000000054c7ae3 in <core::future::from_generator::GenFuture<buck2_server_ctx::template::run_server_command<buck2_server_commands::commands::build::BuildServerCommand>::{closure#0}::{closure#0}> as core::future::future::Future>::poll () from ...
#07  0x0000000005370df8 in <buck2_events::dispatch::Span>::call_in_span::<core::task::poll::Poll<(core::result::Result<cli_proto::BuildResponse, anyhow::Error>, buck2_data::CommandEnd)>, <buck2_events::dispatch::EventDispatcher>::span_async<buck2_data::CommandStart, buck2_data::CommandEnd, core::future::from_generator::GenFuture<buck2_server_ctx::template::run_server_command<buck2_server_commands::commands::build::BuildServerCommand>::{closure#0}::{closure#0}>, core::result::Result<cli_proto::BuildResponse, anyhow::Error>>::{closure#0}::{closure#0}::{closure#0}> () from ...
#08  0x00000000054f7288 in <core::future::from_generator::GenFuture<<cli::commands::daemon::BuckdServerDependenciesImpl as buck2_server::daemon::server::BuckdServerDependencies>::build::{closure#0}> as core::future::future::Future>::poll () from...
 ...
```

At this point, you can look at the code, and note that there is no span around
the output symlink creation function (`create_unhashed_outputs`). This suggests
you’ve found your culprit: there is indeed a buck2 bug and we’re spending ages
creating unhashed output symlinks, and since you need a span to get any console
feedback, the console says nothing is happening.

**An easy fix**: In this particular instance, Thomas spotted
[an easy optimization](https://github.com/facebook/buck2/commit/d677e41253b73a31aafa1255a532c38992482efd)
which resolved the issue. But, of course, that’s not always possible. If the
easy fix hadn't been available, we’d be at a dead end, so what do we do next?

**A harder fix**: If it is not clear what the root-cause is, <OssOnly>you can
bisect</OssOnly><FbInternalOnly>[you can bisect](users/faq/how_to_bisect.fb.md)</FbInternalOnly>:
i.e. do a binary search across commits for the commit that introduced a given
breakage/perf degradation. <FbInternalOnly> Thanks to the fact that we enforce a
linear history, bisecting is pretty straightforward in
`fbsource`.</FbInternalOnly> Then, once you identify their commit that caused
breakage, investigate what caused the issue.

## How to debug a “doing nothing” hang

**Cycle in dependencies**: If buck2 seems to be doing nothing (e.g. CPU usage is
0%), one of the reasons could be a cycle in your dependencies, which may cause
buck2 to hang (buck2 does implement a form of cycle detection, but it
unfortunately has false negatives). You can confirm this by running buck1, which
will report cycles properly.
