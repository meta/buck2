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
ctx.actions.dynamic_output(
    dynamic = [...],
    inputs = [...],
    outputs = [...],
    f = lambda ctx, artifacts, outputs: …,
)
```

The arguments must be passed by name. They are:

- `dynamic` - a list of artifacts whose values will be available in the
  function. These will be built before the function is run.
- `inputs` - this parameter is accepted but **ignored**. It exists for
  historical reasons and has no effect. You can omit it or pass an empty list.
- `outputs` - a list of unbound artifacts (created with `declare_output`), each
  passed as `artifact.as_output()`, which will be bound by the function.
- `f` - the function, which is given 3 arguments:
  - `ctx` (context) - which is the same as that passed to the initial rule
    analysis.
  - `artifacts` - using one of the artifacts from `dynamic` (example usage:
    `artifacts[artifact_from_dynamic])` gives an artifact value containing the
    methods `read_string` and `read_json` to obtain the values from the disk.
    Anything too complex should be piped through a Python script for
    transformation to JSON.
  - `outputs` - using one of the artifacts that were declared for the
    `dynamic_output`'s `outputs` (example usage: `outputs[out]`, and not
    `outputs[out.as_output()]`) gives an unbounded artifact. The function argument must use its `outputs` argument to
    bind output artifacts, rather than reusing artifacts from the outputs passed
    into `dynamic_output` directly.
- The function must call `ctx.actions` (probably `ctx.actions.run`) to bind all
  outputs. It can examine the values of the dynamic variables and depends on the
  inputs.
  - The function will usually be a `def`, as `lambda` in Starlark does not allow
    statements, making it quite underpowered.

Following is a complete rule that reads a file it has written and uses the
content to write its output:

```python
def _impl(ctx):
    dep_file = ctx.actions.write("deps.txt", "x\ny\n")
    out = ctx.actions.declare_output("out")

    def f(ctx, artifacts, outputs):
        deps = artifacts[dep_file].read_string().splitlines()
        ctx.actions.write(outputs[out], "".join(deps))

    ctx.actions.dynamic_output(
        dynamic = [dep_file],
        inputs = [],
        outputs = [out.as_output()],
        f = f,
    )
    return [DefaultInfo(default_output = out)]

my_rule = rule(impl = _impl, attrs = {})
```

Following is an example of using the function to determine Erlang BEAM
dependencies, where `erl -dump-output` stands for a tool that writes the short
paths of the source files a source file depends on, one per line:

```python
def _erlang_impl(ctx):
  beams = {}
  for x in ctx.attrs.srcs:
    dep_file = ctx.actions.declare_output(x.short_path + ".out")
    ctx.actions.run(
      ["erl", "-dump-output", x, dep_file.as_output()],
      category = "erl_deps",
      identifier = x.short_path,
    )
    beam_file = ctx.actions.declare_output(x.short_path + ".beam")
    beams[x.short_path] = beam_file
    def f(ctx, artifacts, outputs, x=x, dep_file=dep_file):
      deps = artifacts[dep_file].read_string().splitlines()
      ctx.actions.run(
        [
          "erl", "-comp", x,
          [beams[d] for d in deps],
          outputs[beams[x.short_path]].as_output(),
        ],
        category = "erl_compile",
        identifier = x.short_path,
      )
    ctx.actions.dynamic_output(
      dynamic = [dep_file],
      inputs = [],
      outputs = [beam_file.as_output()],
      f = f,
    )
  return [DefaultInfo(default_outputs = beams.values())]

erlang = rule(impl = _erlang_impl, attrs = {"srcs": attrs.list(attrs.source())})
```

The above code uses `declare_output` for the `beam_file` then binds it within
the function `f`, after having read the `dep_file` with `read_string`.

## `dynamic_actions`

`ctx.actions.dynamic_output_new` is the newer version of `dynamic_output`. The
function is defined at the top level and wrapped with `dynamic_actions`, which
declares the values the function receives with `dynattrs`. Following is the
first rule above, written this way:

```python
def _my_impl(actions: AnalysisActions, dep_file: ArtifactValue, out: OutputArtifact):
    deps = dep_file.read_string().splitlines()
    actions.write(out, "".join(deps))
    return []

_my_dynamic_actions = dynamic_actions(
    impl = _my_impl,
    attrs = {
        "dep_file": dynattrs.artifact_value(),
        "out": dynattrs.output(),
    },
)

def _impl(ctx):
    dep_file = ctx.actions.write("deps.txt", "x\ny\n")
    out = ctx.actions.declare_output("out")
    ctx.actions.dynamic_output_new(
        _my_dynamic_actions(dep_file = dep_file, out = out.as_output()),
    )
    return [DefaultInfo(default_output = out)]

my_rule = rule(impl = _impl, attrs = {})
```

For the details, see
[`dynamic_output_new`](../../api/build/AnalysisActions/#analysisactionsdynamic_output_new),
[`dynamic_actions`](../../api/build/#dynamic_actions) and
[`dynattrs`](../../api/build/dynattrs/) in the API reference.
