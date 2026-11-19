---
id: writing_rules
title: Writing Rules
---

import { FbInternalOnly } from 'docusaurus-plugin-internaldocs-fb/internal';

This page describes how to write rules for Buck2 and explains the flow for
implementing rules that are already defined in Buck1.

For a list of the API functions available, see the
[Build APIs](../../api/build).

:::note

Rules such as `@fbcode_macros//build_defs:native_rules.bzl buck_genrule` are not
actually rules, they are _macros_ (Starlark functions that eventually call out
the underlying `genrule` _rule_). Macros in Buck2 are mostly compatible with
Buck1 and should be written in the same way.

:::

## Workflow by example

The built-in Buck2 rules are stored in the `prelude` folder in the buck2 repo.
To add a rule for a language, say `pascal`:

1. Look at
   [prelude/decls](https://github.com/facebook/buck2/blob/main/prelude/decls/)
   to see the attributes that are supported in Buck1 and are mirrored into
   Buck2. If `pascal` was an existing rule, you would see what attributes it
   takes (often it will be `pascal_library` and `pascal_binary`).

2. Create a file `pascal.bzl` that will contain your rule implementations. The
   details are explained later, but a dummy rule looks like the following:

   ```python
   def pascal_binary_impl(_ctx: AnalysisContext) -> list[Provider]:
       return [DefaultInfo()]
   ```

3. Create a directory in `fbcode/buck2/tests/targets/rules/pascal` with
   `TARGETS` and whatever source files and test targets you need to test your
   project. Note, Apple tests are currently located at
   `xplat/buck2/tests/apple/...`.

4. Test your code with `buck2 build fbcode//buck2/tests/targets/rules/pascal:`.
   They should succeed with no actual output produced.

5. Now implement the rules (see the rest of this page).

:::note

Before merging a diff, it's important that all your Starlark is warning free (if
you don't want to set up Buck2 for local development, test it in CI).
<FbInternalOnly>If you do set it up locally, see the `README.md` in the root of
`fbcode/buck2`. `buck2 starlark lint <files>` will confirm your Starlark code is
warning free.</FbInternalOnly>

:::

## Concepts and design

A _rule_ for a _target_ uses _attributes_ to declare _actions_, which produce
_artifacts_ that get included in _providers_.

For example, given:

```python
def pascal_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    ...
    binary = ctx.actions.declare_output(ctx.attrs.out)
    ctx.actions.run(["pascalc", ctx.attrs.srcs, "-o", binary.as_output()])
    return [
        DefaultInfo(default_output = binary),
    ]

pascal_binary = rule(impl = pascal_binary_impl, attrs = {
  "out": attrs.string(),
  ...
})
```

In the above snippet:

- **Rule** is `pascal_binary`, which is implemented by `pascal_binary_impl`. The
  rule says how to build things.
- **Target** will be something like
  `fbcode//buck2/tests/targets/rules/pascal:my_binary`. The rule implementation
  `pascal_binary_impl` will be called once per target.
- **Attributes** are the fields on the target (for example, you might have
  `out`, which can be accessed via `ctx.attrs.out`).
- **Actions** are declared by the rule with things like `ctx.actions.run`, which
  takes a command line. Note that the actions are not run by the rule, but
  declared, so that Buck2 can run them later.
- **Artifacts** represent files on disk, which could be source or build outputs
  (`binary` in the above example).
  - For build outputs, the artifact is produced by an action, and the existence
    of the artifact does not imply the build has been run: the artifact
    'remembers' what should be run if it is required.
- **Providers** are returned, which is information that other rules get to use.
  These will often contain artifacts.

The rule implementation takes in a `ctx`, which is the rule context. The two
most important fields are `ctx.attrs`, which picks up the attributes declared by
the rule, and `ctx.actions`, which lets you create new actions to actually do
something.

The output of any actions performed will be materialized in `buck-out`. However,
only the defined outputs of providers are available for dependent rules to
consume and only the actions necessary to produce those outputs being consumed
will be run. By default, the `default_output` of the `DefaultInfo` provider is
built and output during a `buck build`.

### Providers

Providers are the data returned from a rule and are the only way that
information from this rule is available to rules that depend on it. Every rule
must return at least the `DefaultInfo` provider, but most will also return
either `RunInfo` (because they are executable) or some custom provider (because
they are incorporated into something that is ultimately executable).

The `DefaultInfo` provider has a field `default_output`, which is the file that
will be built when someone executes a `buck2 build` on this particular target,
and the file that will be used when someone runs `$(location target)` or uses it
as a source file (such as `srcs = [":my_target"]`.)

The current rule of thumb is that if you can build the `default_output`, the
rule must 'work', and, if usable, should be 'ready'. For example, for a binary,
the executable and runtime libraries it depends on might be returned. For a
library, because neither the static or dynamic library is the 'default', you
merely have to do enough work to ensure that the static and dynamic library
probably work.

Similar to how `DefaultInfo` wraps a list of artifacts and `$(location)` selects
from `DefaultInfo`, `RunInfo` wraps a command line and `$(exe)` selects from
`RunInfo`.

For more information about command lines, see [Run action](#run-action), below.

For libraries, usually you need to pass some information about the library up to
the binary. The _only_ information that dependents on the library get are the
providers, so designing the information that flows around the provider is
critical to designing good rules.

For a hypothetical rule, you may decide you want the name of the library and the
artifact that represents the `.so` file, for which you could define the
following provider:

```python
PascalLibraryInfo = provider(fields=[
    "name",   # The name of the library
    "object"  # An artifact, the .so file that needs linking in
    ]
)
```

Often, you'll grab your dependencies from all your providers:

```python
my_deps = [x[PascalLibraryInfo] for x in ctx.attrs.deps]
```

In many cases, it becomes apparent you need the transitive closure of all
libraries (for example, the libraries and everything they depend upon), in which
case, the standard pattern is to move to a provider of a list of `record` (see
the
[types.md](https://github.com/facebook/starlark-rust/blob/main/docs/types.md)
document in GitHub) and the `flatten/dedupe` functions, defining it as:

```python
PascalLibraryInfo = provider(fields=["links"]) # a list of LinkData

LinkData = record(name = str, object = "artifact")
```

And then consuming it:

```python
my_links = dedupe(flatten([x[PascalLibraryInfo].links for x in ctx.attrs.deps]))
my_info = PascalLibraryInfo(links = my_links)
```

However, this `flatten`/`dupe` pattern can get expensive, especially when you
have a deep dependency graph. To fix that it's recommended to use
[transitive sets](transitive_sets.md).

### Actions

There are several actions you can use to create symlink trees, and so on.

#### Run action

Of the various actions, the `run` action is by far the most important: it's the
one that invokes a command line.

A command line is both a list of string arguments and a list of artifacts they
depend on; with syntactic niceties for adding artifacts to command lines in a
way that ensures the dependencies are usually correct.

Following are examples of command line manipulations:

```python
cmd = cmd_args(["some", "arguments"])
cmd.add("another-arg")
cmd.add(ctx.attrs.src) # An input artifact
out = ctx.actions.declare_output("an output")
cmd.add(out.as_output())
ctx.actions.run(cmd)
```

The action `declare_output` creates a new artifact which is not bound to
anything. You can call `.as_output()` on it when adding it to a command line to
say that this command line doesn't take the artifact as an input but produces it
as an output.

From now on, if `out` is used as a dependency (either to another command line,
or in `DefaultInfo`) then the action will be run to produce that artifact.
Typically, these outputs are declared (`declare_output`), bound in a
`ctx.actions.run` call with `.as_output()`, then either used locally as the
input to another action or returned in a provider.

As another example:

```python
cmd = cmd_args(["cp", input, output.as_output()])
ctx.actions.run(cmd)
```

A command provides both a string (what to write when used) and a list of
artifacts (what must be available when used). Normally, as in the case above,
the artifacts that are used correspond to those on the command line. But imagine
the rule is changed to write the command to a shell script first:

```python
sh = ctx.actions.write("test.sh", ["cp", input, output])
cmd = cmd_args(["sh",sh],hidden=[input, output.as_output()])
ctx.actions.run(cmd)
```

The command has been written to a shell script, which is now run. Beforehand,
all the artifacts used by the command appeared on the command line. Now they
don't. However, the shell script still accesses input and output. To inform the
run command, use the hidden field of the command line to declare the dependency.

For more complicated actions, which perform meaningful logic beyond invoking a
simple command, the tendency is to write custom Python scripts. Python scripts
are used instead of shell scripts as they have better cross-platform
compatibility and fewer hidden corners (especially in error paths).

As an example of a Python helper, see
[make_comp_db.py](https://github.com/facebook/buck2/blob/main/prelude/cxx/tools/make_comp_db.py).

A further advantage of using Python is that these commands can be tested in
isolation, outside of Buck2.

## Debugging

The functions `fail`, `print` and `pprint` are your friends. To get started, a
`buck2 build fbcode//buck2/tests/targets/rules/pascal:` builds everything or
`buck2 run fbcode//buck2/tests/targets/rules/pascal:my_binary` runs a specific
binary that returns a `RunInfo`.

## Testing Rules

A common way to test is to use `genrule` to cause the produced binary to run and
assert some properties from it. If your rule is in Buck1 and Buck2, use a
`TARGETS` file so you can test with both. If your tests are incompatible with
Buck1 (such as if it is a new rule), use `TARGETS.v2`, which will only be seen
by Buck2 and won't cause errors with Buck1.

## New rules

If your rule is **not** already in Buck1, then you can define it wherever you
like, with a preference for it not being in `fbcode/buck2/prelude`.

The only advantage of the `prelude` is that rules can be used without a
corresponding `load`, which is generally considered a misfeature. The attributes
should usually be placed adjacent to the rule itself.

As an example, just below the `pascal_binary_impl` function, you could write:

```python
pascal_binary = rule(
    impl = pascal_binary_impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),
        "src": attrs.source(),
    }
)
```
