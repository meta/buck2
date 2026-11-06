import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

# Buck Extension Language (BXL)

Buck2 will allow more complex introspection and interaction with its graphs via
the `bxl` feature. BXL will be a starlark script that allows integrators to
interact with `buck` commands like build and query within starlark, creating a
sequence of operations that introspect, build, and extend the build graph.

<FbInternalOnly>

https://fb.workplace.com/groups/buck2prototyping/permalink/2404233936540759/.

</FbInternalOnly>

These are essentially custom buck operations, defined in Starlark, that still
follow the constraints of Buck2, which will enable the same level of
incrementality and caching as native buck2 operations. Furthermore, bxl will
have subscriptions enabled in the future, where based on the incrementality
tracking, buck2 can provide "updated" bxl executions when its known that its
dependencies change, and even when generated sources need to be regenerated.

The following proposes a basic set of bxl api and building blocks that are
targeted at solving key issues for IDE integration.

## Use Cases

### Cpp LSP

<FbInternalOnly>

I’ve previously defined some proposed integrations
[here](https://docs.google.com/document/d/1jyehtuQ236rtwq2yyLnLmsIgBOctuAm9eoqx95TCO4I/edit).

</FbInternalOnly>

Lsp prefers to have a single buck command that given a file, returns the
corresponding compilation database. This requires a single command, i.e a bxl,
that accepts a file as input, performs `owners` queries, and uses the owning
target plus the desired file to get the clang flags, and then writes it to disk
in comp db format. It’s possible to write the same features using buck calls to
cquery, and build using subtargets to generate compilation database per file.
However, this requires lsp owners to maintain code in several locations and
languages, and parse and reserialize data. It also does not provide the same
incrementality and subscription update features of the resulting comp db that
writing this in bxl would have. Furthermore, we may explore the idea of trimming
the compilation command to only dependencies required per the file requested.
Bxl actions provides a straightforward api for adding this when writing the
actual comp db file.

### Android LSP

Android project requires traversing the target graph to find and java libraries,
grouping and converting them between modules or project libraries depending on
the number of references, and restructuring the graph as directory based.
Android LSP is able to take advantage of subscriptions in the future when
available, allowing developers to keep their IDE up-to-date automatically
without needing to manually regenerate the project.

With bxl, the graph traversals can be written in starlark, allowing propagation
of information down the graph, accessing targets’ attributes to analyze
dependencies, and access providers for artifacts and action information needed
to output the project file. Project generation also performs directory listings
that buck2’s dice already performs and caches (I think, need to confirm). Bxl
poses the interesting possibility that we can expose a limited set of IO
operations that are tracked by dice so bxl can access the same cached file
operations as rest of buck2. Android project generation currently doesn’t write
project files to buck-out, which prevents it from using buck2 actions. It will
have to rely on an external script to process the graph information printed by
buck and write the actual project files. If it moves to `buck-out` based, then
it can take advantage of creating actions directly using the graph information
processed, and potentially take advantage of incremental actions api to avoid
writing the entire graph on each subsequent update.

### iOS Project

iOS is currently being implemented as a series of queries that are aggregated by
an external python script, that then invokes builds of subtargets. The same can
be achieved in bxl, but with the entire sequence being cacheable and
subscribable so that when the graph is updated, or even when generated files
need updating, buck2 can automatically push the updates. However, it is
uncertain whether xcode itself can make use of push updates.

<FbInternalOnly>

In
https://docs.google.com/document/d/1USZ_ZYxq45DHUFF-BAYo6zS4lAHlpvNk9uM5SBL9e-w/edit?disco=AAAAQv4gLQ0,
it was also proposed that project generation may need information to flow down
as part of the generation, which is only possible via bxl defining its own
actions. (Although, there may have been a workaround per Chatura).

</FbInternalOnly>

### Rust LSP

(note from dbarsky@: I’m adding this at Bob’s request. Can be removed as
needed.)

### Visual Studio Project (vsgo)

Vsgo is a pile of python that converts buck query/buck targets output via a
variety of heuristics into inputs to a custom fork gyp which is then invoked to
generate visual studio projects for a given buck target. Having direct access to
the internals of buck would allow us to remove the heuristics and possibly even
move project generation directly into bxl.

## Goals

From the above use cases, BXL should offer a simple Starlark API that allows
easy introspection of the buck2 graph at unconfigured, configured, providers,
and actions stage, maintaining incremental behaviour of the BXL evaluation
itself.

Some minimal API should be offered to allow BXL to provide additional behaviour
such as output artifacts, and print results.

Most use cases from LSP desire to also propagate information via the command
line for these operations, so BXL should support command line arguments as
inputs.

## API

### Defining a bxl function

There are multiple models possible. We can have each file be its own bxl, or
have each file declare multiple bxl like rules.

There are multiple advantages to allowing declaration of multiple bxls, such as
grouping similar bxls in the same file, allowing them to "invoke" each other. It
doesn’t necessarily add much more complexity for the author, as even with one
bxl per file, the author still has to have some declaration for the bxls
arguments.

```python
# sample.bxl
func1 = bxl_main(
   impl = my_func1,
   args = {
     "arg1": arg.list(arg.str()),
   }
)

func2 = bxl_main(
   ...
)

```

To invoke buck2 for that bxl, we can have the command line as follows.

```shell
buck2 bxl sample.bxl::func1 -- --arg1 foo bar baz
```

For bxl functions to read the arguments, a similar api to rule attrs is used

```python
args = ctx.args.args_for_bxl
```

Args defined like attrs when declaring the bxl function above

### Accessing target nodes

All standard query functions will be enabled in bxl, allowing users to run query
operations, storing them in variables and interacting with them. These allow
introspection of the unconfigured targets, or the configured targets based on
api

```python
# some.bxl
targets = ctx.uquery(‘deps("//foo")’)
targets = filter(targets, my_filter)

# introspect a target
for target in targets:
  ctx.print(target.attributes) # prints selects
  # also inspect the target like below
  ctx.print(target.label)

target = ctx.cquery("//foo", "//x86").attributes # cquery has selects resolved
```

### Inspect providers

When we have a configured target, bxl can request for the analysis of the rule

```python
target = <some configured target>

ctx.analysis(target).providers # access the providers
```

### Actions

For IDEs, to generate compilation databases, or generate project files, writing
them in bxl will entail creating actions, and executing them. As such, bxl will
also be given the rules api to register actions, including dynamic outputs for
the rule in the current bxl invocation to build artifacts as part of a bxl
function.

BXL has the ability to create actions with some constraints:

1. Action is tied to a particular target
2. It’s output location is determined in the same pattern as regular actions
   defined via rules

```python
targets = ctx.cquery(‘deps("//foo:rule")’)

for t in targets:
  action_ctx = ctx.analysis(t).actions
  # the action context here is tied to the configured target `t`
  # actions registered by bxl will be attached with bxl prefix key
  action_ctx.registry.write(some_output, "foo")

```

BXL can also interact with the existing actions on an action via the action_ctx,
such as iterating through it, analyzing its outputs, or requesting it to be ran.

```python
targets = deps("foo:rule")

for t in targets:
  action_ctx = ctx.analysis(t).actions
  for action in action_ctx.iter():
    if "foo/path" in action.output:
      ctx.build(action)
```

### What is cached?

All computations requested by a bxl function will be treated as inputs. So if a
bxl function calls uquery, then uses the result to do a cquery, and then a
build, if buck2 detects that any of the recorded calls to uquery, cquery, and
build changes, the entire bxl will be reran, with no early cutoff. The
computations itself will still be cached via DICE, so no major performance
issues are expected. However, in the event that a bxl function is
computationally heavy, the recommendation would be to move that to an action, or
split up the bxl and use inter-bxl caching described below.
