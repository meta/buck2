---
id: architecture
title: Architectural Model
---

import useBaseUrl from '@docusaurus/useBaseUrl';

## High-level Overview

Buck2 is a build system whose core is written in Rust. Starlark, which is a
deterministic, immutable version of Python, is used to extend the Buck2 build
system, enabling Buck2 to be language-agnostic.

The high-level flow starts with a user creating a build file (a `BUCK` file)
containing one or more targets, which is specified by the target label, its
inputs (sources, attributes, configurations, and dependencies), and the type of
macro or rule to use.

Briefly, a macro is a wrapper around a rule, which runs necessary commands to
generate what’s needed for a target (for example, for a `cxx_binary` target,
generate the header map and run necessary `clang` commands). Macros can be used
to reduce boilerplate code for users (such as to supply the same set of
attributes for a rule for all targets). Macros and rules are both written in
Starlark and are specified by input sources, attributes, and the implementation
function.

If the target type is a macro, then the macro will fill in some details (for
example, for a `cxx_binary` target, these are the compilation, debug flags to
use, this is the `clang` to use). If the target type is a rule, then the macro
layer is skipped altogether.

This is all orchestrated by the core, which performs operations such as
executing Buck2 CLI args, generating/updating the dependency graph (which
contains the configured target nodes, unconfigured target nodes, action nodes,
among other types of nodes that all allow for incrementality and execution), and
materializing the artifacts. The core is written in Rust.

The following diagram shows the high-level overview.

<img src={useBaseUrl('/img/buck2_rule_workflow.png')} alt='justifyContent'/>

The Buck2 CLI runs in a client process, which sends commands to the Buck2 daemon
via gRPC. The daemon goes through several phases after receiving a request from
the client: **evaluation, configuration, analysis, execution, and
materialization** (see [Execution Model](#execution-model), below). When using
`buck2 test`, there is a final stage for **testing**. Note that these are the
phases that a build goes through, but they are not always sequential.

After finishing all phases, the daemon will send the response back to the client
via gRPC.

## Execution Model

The following diagram shows the Execution Model, which consists of 5 phases and
states.

<img src={useBaseUrl('/img/buck2_architecture.png')} alt='justifyContent'/>

Each of the phases and states shown in the Execution Model, are detailed in the
following sub-sections.

### State 0 - Build Files

Build files (commonly referred to as `BUCK` files, their default name) are the
main input to Buck2 and are syntactically Python.

Each build file is uniquely identified by the directory in which it's located.
Since all build files have the same name, there cannot be two build files in the
same directory. This is usually represented as the relative path from the root
of the project (the directory where the .buckconfig file is).

Each build file has a set of targets. These describe the things the user wants
Buck2 to know about. Each target has a type and a set of named attributes,
including at least a name (also known as the label) identifying it. Additional
attributes depend on the type of the target.

### Phase A: Evaluation

First, Buck2 evaluates a build file, and then constructs an unconfigured target
graph.

Buck2 performs directory listings to discover packages, then evaluates the build
files that were found, expands any macros detected into their underlying rules,
and then will take rule attributes and convert them from Starlark to Rust types
to construct a target node, and insert it into the unconfigured target graph,
which is a smaller portion of Buck2’s larger dependency graph. The target node
consists of a reference to rule implementation, and the set of attributes and
sources.

The result of evaluation is a list of targets read from the build file mapped to
a target node in Buck2 unconfigured target graph.

### State 1 - Unconfigured Target Graph is generated

At this point, the unconfigured target graph is available for the next stage of
transformation, which is to configure the target nodes within the graph.

### Phase B: Configuration

At the end of evaluation, the target nodes are not yet configured. Configuration
means applying a list of constraints (such as resolving selects to specify the
right CPU) to make sure the target can be run where it needs to. This is also
known as target platform resolution, and can be configured within the target,
the buckconfig, propagated from dependencies, or passed into the CLI. After
applying configurations, the target nodes are transformed into configured target
nodes within the Buck2 configured target graph, which is a smaller portion of
Buck2’s larger dependency graph.

### State 2 - Configured Target Graph is generated

At this point, the configured target graph is available for the analysis stage
to generate the action graph.

### Phase C: Analysis

In the analysis phase, Buck2 constructs a context object (ctx) which contains
relevant information (such as attributes pulled from the configuration stage),
all converted into Starlark types and made available to the rule. For example,
the target’s dependencies are turned into a `ProviderCollection`, source files
are converted into `StarlarkArtifacts`, and String attributes are turned into a
`StarlarkString`. This ctx object is backed by Buck2’s dependency graph for
computation and rules use it to tell Buck2 to run actions, create dynamic
actions, or create new files.

The rule will return a list of providers, which is data that the rule wants to
expose to its dependents (that is, can flow through the dependency graph), such
as output artifact information (such as file paths and file hashes). Providers
could be actions, source files, or attributes. Within the returned list,
DefaultInfo always needs to be returned, which indicates what the default
outputs are. Some other common built-in providers include RunInfo, TestInfo, and
InstallInfo.

The end result is a list of providers and actions (inserted into the action
graph) that Buck2 needs to execute to produce the desired outputs, known as
'bound artifacts'.

### State 3 - Action Graph and Providers are generated

At this point, the action graph and providers are available to be processed by
the execution stage.

### Phase D: Execute

Execution is where Buck2 takes all the providers (input files from the targets,
args from the command line), runs the actions, and then outputs the computed
results. The critical path is the theoretical lower bound for the duration of a
build, which are the slowest set of actions.

Buck2 can be run locally or on remote execution, or in a hybrid manner.

For each action, a digest is created which is a hash of an action's command and
all its inputs. Buck2 then checks if there is a result cached within RE for an
action with a given digest.

If there is a cache hit, Buck2 does not need to run the command for the action.
Instead, the RE returns the output action digest. This digest can be used to
download the actual output artifacts at a later time. This is known as the **RE
action cache**.

If there is a cache miss, the action needs to be run either remotely or locally.
If Buck2 decides to run the action remotely, it will first upload all of the
action's inputs that are missing from the RE's content addressable storage. If
Buck2 decides to run the action locally, it will first download and materialize
in `buck-out` all of the action's inputs. These inputs might be outputs of other
actions and are stored in RE's content addressable storage but are missing on
the local machine. Only after those steps will Buck2 schedule the action for
actual execution.

Buck2 can also decide to run local and remote execution simultaneously (a
process known as racing), and use the result of whichever action finishes first
to speed up performance. This strategy is known as **hybrid execution**."

Materialization of action outputs (which involves downloading and placing them
in the correct location in `buck-out`) can be done immediately after the action
has finished executing. Alternatively, it can be deferred until it is actually
needed for the local execution of another action. There are various
configurations that a user can set to control how this materialization is
handled.

### State 4 - Build outputs are generated

At this point, the build is complete.

If a user ran `buck2 test`, then there is a final transformation for Buck2 to
construct a command for TPX to execute the actual test.

### Phase E: Execute tests

For more detail on testing, review
[Test Execution](../rule_authors/test_execution.md).
