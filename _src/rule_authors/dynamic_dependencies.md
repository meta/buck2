---
id: dynamic_dependencies
title: Dynamic Dependencies
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

Dynamic dependencies allow a rule to use information that was not available when
the rule was first run at analysis time. Dynamic dependencies in Buck2 are
implemented using `dynamic_output` and are restricted in their power compared to
fully generic dynamic dependencies.

A rule for a target is run with the attributes of the target, plus the providers
of its attribute dependencies, which contain artifacts. Those values (but not
the artifact contents) are all available directly and immediately when running
the rule. The rule generates providers containing artifacts. Using
`dynamic_output`, a rule can read the contents of an artifact to produce new
artifacts and bind existing artifacts, which were already returned in providers.

Examples of rules requiring dynamic dependencies include:

- Distributed ThinLTO, where the index file says what the dependencies are.
- OCaml builds, where the dependencies between source files can only be obtained
  from running `ocamldeps`.
- Erlang header files, where only a subset of the available headers are
  accessed, which can be determined by reading the source file.
- Erlang BEAM files, where some subset of BEAM files must be compiled in a given
  order, as they provide features like compiler plugins, but most can be
  compiled in parallel.

<FbInternalOnly>

The original design document with discussion is available
[here](https://docs.google.com/document/d/1K8RgvDMvdDFsLWAu0cehauJstHZaFe-7NeaAqWe4-L4/edit).

</FbInternalOnly>

## Implementation

Buck2 provides the following function:

```python
ctx.actions.dynamic_output(dynamic, inputs, outputs, lambda ctx: …)
```

The arguments are:

- `dynamic` - a list of artifacts whose values will be available in the
  function. These will be built before the function is run.
- `inputs` - this parameter is accepted but **ignored**. It exists for
  historical reasons and has no effect. You can omit it or pass an empty list.
- `outputs` - a list of unbound artifacts (created with `declare_artifact`)
  which will be bound by the function.
- The function argument is given 3 arguments:
  - `ctx` (context) - which is the same as that passed to the initial rule
    analysis.
  - `outputs` - using one of the artifacts from the `dynamic_output`'s `outputs`
    (example usage: `outputs[artifact_from_dynamic_output_outputs]`) gives an
    unbounded artifact. The function argument must use its `outputs` argument to
    bind output artifacts, rather than reusing artifacts from the outputs passed
    into `dynamic_output` directly.
  - `artifacts` - using one of the artifacts from `dynamic` (example usage:
    `artifacts[artifact_from_dynamic])` gives an artifact value containing the
    methods `read_string`, `read_lines`, and `read_json` to obtain the values
    from the disk in various formats. Anything too complex should be piped
    through a Python script for transformation to JSON.
- The function must call `ctx.actions` (probably `ctx.actions.run`) to bind all
  outputs. It can examine the values of the dynamic variables and depends on the
  inputs.
  - The function will usually be a `def`, as `lambda` in Starlark does not allow
    statements, making it quite underpowered.

Following is an example of using the function to determine Erlang BEAM
dependencies:

```python
def erlang(ctx):
  beams = {}
  for x in ctx.attr.srcs:
    dep_file = ctx.actions.declare_output(x + ".out")
    ctx.actions.run("erl", "-dump-output", x, dep_file.as_output())
    beam_file = ctx.actions.declare_output(x + ".beam")
    beams[x] = beam_file
    def f(ctx, artifacts, outputs, x=x, dep_file=dep_file):
      deps = artifacts[dep_file].read_lines()
      ctx.actions.run(
        "erl", "-comp", x,
        [beams[d] for d in deps],
        outputs[beams[x]].as_output()
      )
    ctx.actions.dynamic_output([dep_file], [x] + deps, [beam_file], f)
  return [ErlangInfo(objects = beams.values())]
```

The above code uses `declare_output` for the `beam_file` then binds it within
the function `f`, after having read the `dep_file` with `read_lines`.
