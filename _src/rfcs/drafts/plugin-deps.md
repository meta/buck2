## Plugin Deps

### Background on Rust proc macros

Rust proc macros are compiler plugins. They are a special kind of crate that is
compiled to a dylib, which is then loaded by the compiler when another crate
depends on the proc macro. Notably, like all Rust crates, proc macros may also
be re-exported. This means that if there is a dependency chain like
`bin -> lib -> proc_macro`, the proc macro must be made available when compiling
the binary, even though it does not appear directly in the dependencies.

Proc macros have posed a challenge to buck2, for two reasons:

1.  Rust users generally expect to not have to distinguish between proc macros
    and normal crates when specifying their dependencies. This means it is not
    easily possible to make the `lib -> proc_macro` edge an `exec_dep`.
2.  `bin` and `lib` might end up with different exec platforms. This means that
    even if `proc_macro` were to be correctly configured as an exec dep of
    `lib`, that configuration might be wrong for `bin`.

FIXME: Other use cases for this feature

### Plugins deps

This RFC proposes introducing a concept of "plugin deps" to solve this problem.
Plugin deps are deps that can be propagated up the build graph at configuration
time, instead of at analysis time. Here's what this looks like:

First, plugin deps come in "kinds." Plugin kinds can be created like
`MyKind = plugins.kind()`. These act as identifiers that can be used to divide
all the possible plugin deps up however users need to.

Each configured target has plugin lists: There is one list for each plugin kind.
The elements of these list are an _unconfigured_ target, together with a
`should_propagate` bool. The same unconfigured target cannot appear more than
once. In other words, this is a `HashMap<String, HashMap<Target, bool>>`. We
need to describe two things: How to _use_ these list, and how to _create_ them.

### Using a target's plugin lists

Using plugin lists is very simple: The rule sets `uses_plugins = [MyKind]` when
declared. Setting this make the elements of the plugin list for the given kind
appear as exec deps on the configured nodes for this rule. This also means that
the plugins participate in exec dep resolution like all other exec deps.

Analysis will then be able to access a list of the providers for each of the
plugins via `ctx.plugins[MyKind]`.

The `should_propagate` bool that is associated with each element of the list is
ignored at this stage.

### Creating a target's plugin lists

Plugin lists are created by accumulating from two sources:

The first of these is direct plugin deps. They are defined via a new
`attrs.plugin_dep(kind = "foo")`. This attribute (like other deps), is set to a
label when the target is declared. It then resolves as follows:

- In the unconfigured graph: To the appropriate unconfigured target
- In the configured graph: To the label of the unconfigured target. In other
  words, this will still be displayed in `buck2 cquery -A`, but will not appear
  in the deps.
- During analysis: Also to the unconfigured target label.

The target that appears in the `plugin_dep` is added to the `MyKind` plugin list
with `should_propagate` set.

The second way to add to the plugin list is by inheriting from regular deps.
This works as follows: Elements of the plugin lists for which the
`should_propagate` value is true are made available to the immediate rdeps of a
configured target. The rdep can use them by setting `pulls_plugins = [MyKind]`
in the appropriate `attrs.dep()` invocation. This will make the targets appear
in the plugin list for the rdep with `should_propagate` unset. Alternatively,
the rdep can set `pulls_and_pushes_plugins = [MyKind]` to add the targets to the
plugin lists with `should_propagate` set to true. This enables transitive
propagation further up the configured graph.

To decide later: Should we allow plugin rules to appear in regular/exec deps,
with no special behavior? I don't see why not.

### Example: Proc macros

```py
RustProcMacro = plugins.kind()

rust_proc_macro_propagation = rule(
    impl = _propagation_impl,
    attrs = {
        "actual": attrs.plugin_dep(kind = RustProcMacro),
    },
)

rust_library = rule(
    impl = _similar_to_before, # See some notes below
    attrs = {
        "proc_macro": attrs.bool(default = False),  # Same as before
        "deps": attrs.list(attrs.dep(pulls_and_pushes_plugins = [RustProcMacro])),
        # Here we avoid `pulls_and_pushes` because we do not want to make these deps available to rdeps
        "doc_deps": attrs.list(attrs.dep(pulls_plugins = [RustProcMacro])),
    },
    uses_plugins = [RustProcMacro]
)

rust_binary = rule(
    impl = _similar_to_before, # See some notes below
    attrs = {
        "deps": attrs.list(attrs.dep(pulls_plugins = [RustProcMacro])),
        "doc_deps": attrs.list(attrs.dep(pulls_plugins = [RustProcMacro])),
    },
    uses_plugins = [RustProcMacro]
)

def _propagation_impl(ctx):
    return [
        DefaultInfo(default_outputs = []),
        # During analysis for rust libraries, the providers for proc macros will appear in
        # `ctx.plugins`. However, this includes the transitive and direct proc macro deps, as
        # well as the transitive and direct proc macro doc-deps. Analysis needs to be able to
        # distinguish between all of these though.
        #
        # This dummy provider is passed to allow for precisely that. Generally, it will be passed
        # everywhere where the providers of Rust proc macros are currently passed. That ensures that
        # analysis on `rust_library` and `rust_binary` have all the information they need about
        # where the plugin "entered the dependency graph."
        RustProcMacroMarker(ctx.attrs.actual),
    ]

### TARGETS

# Expanded by macro
rust_library(
    name = "p1_REAL",
    proc_macro = True,
)

# Expanded by macro
rust_proc_macro_propagation(
    name = "p1",
    actual = ":p1_REAL",
)

# Expanded by macro
rust_library(
    name = "p2_REAL",
    proc_macro = True,
)

# Expanded by macro
rust_proc_macro_propagation(
    name = "p2",
    actual = ":p2_REAL",
)

rust_library(
    name = "l",
    deps = [":p1"],
    doc_deps = [":p2"],
)

rust_binary(
    name = "b",
    deps = [":l"],
)
```

Analysis for `:l` will see:

1.  `deps` which contains only the `RustProcMacroMarker("p")`
2.  `doc_deps` which contains only the `RustProcMacroMarker("p2")`
3.  `ctx.plugins[RustProcMacro]` which contains the providers of `:p1_REAL` and
    `:p2_REAL`, correctly configured for the execution platform of `:l`.

Analysis for `:b` will see:

1.  `deps` which contain the providers of `l`
2.  `ctx.plugins[RustProcMacro]` which contain the providers of `:p1_REAL`, also
    correctly configured for its own execution platform (which may be different
    from `:l`'s).

    Note that because `rust_library` does not re-push doc deps, `:b` will not
    see `:p2_REAL`.

As a result, the implementation of the `rust_library` rule should not propagate
the providers of its proc macro deps (unlike its regular deps).

There is one downside to this solution: `buck2 build :p` does absolutely none of
the things that the user is probably expecting. They need `buck2 build :p_REAL`.
That's a bit sad. Thankfully directly building proc macros is not that important
a use case?

#### Alias

It is already the case today that we can't use the normal `alias` rule on
toolchains. A similar situation crops up here, where aliasing a target that
pushes plugins causes the plugins to "get lost." The right solution to this is
to probably allow `plugins.ALL` as a special value on `pulls_plugins` and
`pulls_and_pushes_plugins`, and then set that for the alias rule.
