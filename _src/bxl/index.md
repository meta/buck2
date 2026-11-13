---
id: index
title: Why BXL
---

## Buck2 Extension Language (BXL)

BXL is a Starlark-based script that enables integrators to inspect and interact
with the Buck2 graph.

Integrators are able to:

- Write Starlark code that queries, analyzes, and builds on the Buck2 graph.
- Introspect and interact with the Buck2 graph structures natively, via
  Starlark, in a safe, controlled manner.

Introspection of the Buck2 graph can occur at the unconfigured, configured,
providers, and action stages. There are also APIs offered to allow BXL to accept
custom command line argument, output artifacts, and print results to stdout.

BXL leverages Buck2 core's incremental
[caching](./faq#when-is-my-bxl-script-cached). It also has support for
[running actions](./how_tos/basic_how_tos#running-actions),
[dynamic outputs](./how_tos/how_to_run_actions_based_on_the_content_of_artifact),
and [anonymous targets](./how_tos/how_to_cache_and_share_operations). In
addition, BXL has
[profiling](./how_tos/basic_how_tos#profiling-testing-and-debugging-a-bxl-script)
capabilities, and allows users to add their own
[telemetry](./how_tos/how_to_collect_telemetry_events) directly within the BXL
scripts.

BXL is considered to be mostly stable, with a bit more active development here
and there.

## When should I use BXL over Buck2 API/CLI?

There are many overlaps between BXL and Buck2 (for example, both can run cquery
and both can build targets). It’s possible that one use case could be handled by
both BXL and Buck2.

Following are some specific recommendations to help decide when to use BXL over
regular Buck2:

- **Use/inspect resolved attributes that are not exposed/accessible to users via
  normal Buck2 operations.**
  - This includes introspecting the Starlark object of providers, analyzing the
    Starlark object of a rule’s attr before and after coercing and resolution,
    and introspecting intermediate query results.
- **Reduce/eliminate the need to make several Buck2 calls within your program,
  such as running several subprocesses to call `cquery` several times.**
  - With BXL, you can just call the BXL script once in a subprocess, potentially
    reducing the amount of code you need to write in your program. For example,
    if you need to call cquery and build several times, you can put that all
    within a single BXL script and run `buck2 bxl` once, rather than running
    `buck2 cquery` and `buck2 build` several times.
- **Reduce/eliminate the need to manually parse Buck2 output format within your
  program, and any bugs that may come with manual parsing**.
  - Some languages are more verbose than others when it comes to string parsing.
  - BXL scripts are written in Starlark, which is basically a deterministic,
    immutable Python. BXL is able to directly introspect Starlark objects (such
    as rules and target nodes, and so on) and call methods on these objects
    instead of parsing them over Buck2’s output.

## Example Use Cases

### Generate a project for IDE

IDE project generation is roughly as follows:

- Form the target graph for the project target
- Perform some filtering on the graph targets if needed. This depends on the
  target's configuration.
- For each target, generate the project target metadata, including:
  - compiler flags
  - linker flags
  - paths to generated files
  - inputs and outputs for each targets
  - the paths relative to some `PATH`
- Write a single file translating this metadata into a format understood by the
  IDE

An example BXL flow for generating a project for IDE might be:

- Add some command line arguments to accept a target (or subtarget) to generate
  the project
- Run analysis on the project target with a specific configuration to filter the
  graph targets
- For each resulting target, inspect the providers and attributes to extract the
  required metadata information. BXL uses filesystem operations to handle paths
  within the project
- Run actions based on the linker/compiler flags, and build artifacts as needed
  to generate a project
- Write a single file containing the metadata obtained from previous steps

### Build an LSP

A compilation database is a database containing information about which compile
options are used to build the files in a project. Language Server Protocols
(LSPs) uses the compilation database to provide language features like auto
complete, go to definition, and find all references for the user within an
IDE/editor.

An example BXL flow for building a C++ LSP might be:

- Add a command line argument to accept a file
- Run owners cquery in BXL to get the owning target of the file
- Run analysis on the owning target to get the desired clang flags
- Use BXL to write the clang flags to the disk in compilation database format

### Perform graph analysis

Some example graph analysis functionalities might be:

- Run an analysis in BXL on a set of targets, and then inspect their providers,
  and build some subtargets
- Run a uquery on some set of targets, and inspect the resulting nodes' coerced
  attributes
- Run a cquery on some set of targets with a specific configuration, and inspect
  the resulting nodes' attributes before and after resolution
