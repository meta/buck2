---
id: writing_toolchains
title: Writing Toolchains
---

Toolchains are regular rules that:

- Have `is_toolchain_rule = True` passed to the `rule` call.
- Return the provider(s) expected by rules using that toolchain, conventionally
  named `*ToolchainInfo`.

Toolchain rules are instantiated once in the `toolchains//` cell. The location
of the `toolchains` cell is determined by the value of `cells.toolchains` in the
`.buckconfig` file.

Regular build rules reference those toolchain targets as
[`toolchain_dep`](../../api/build/attrs/#toolchain_dep) attrs (often
[`default_only`](../../api/build/attrs/#default_only) ones). See
[Toolchain Deps](configurations.md#toolchain-deps) for more information about
toolchain deps.

## Writing a custom toolchain

The prelude exposes a few demo toolchains with specific configurations (e.g.
hardcoded compiler and linker flags), that expect to find the tools on the
`PATH`. Many users will want more control over those toolchains. Several options
are available:

- Defining a custom toolchain for a language that is supported in the prelude.
  One can then either:
    - Return the `*ToolchainInfo` provider defined in the prelude, to get
      compatibility with all the prelude target rules for free.
    - Return a custom toolchain provider, for use with a custom set of target
      rules (which is a lot more effort).
- Defining a toolchain for a custom language/process. One then has to define a
  toolchain provider for it, which will be used by the rules for that language.

### Writing a prelude-compatible toolchain

People will often first encounter toolchains when they want to switch off of the
demo toolchains that `buck2 init` uses by default. For example, one might want
to tweak which compiler is used, which flags are passed to it, or where it is
fetched from.

The most straightforward thing to do first is to instantiate one of the "system"
toolchain rules that the prelude offers. Studying the toolchains defined by the
`system_demo_toolchains` macro in `@prelude//toolchains:demo.bzl` is a good way
to get started.

For example, here is how one could define a C++ toolchain that builds projects
in C++23, with warnings as errors and optimizations enabled:

```python
load("@prelude//toolchains:cxx.bzl", "system_cxx_toolchain")

system_cxx_toolchain(
    name = "cxx",
    compiler_type = "clang",
    cxx_flags = [
        "-std=c++23",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-O3",
    ],
    visibility = ["PUBLIC"],
)
```

One would typically use [`select`](../concepts/configurations.md) to customize
the toolchain e.g. based on the build mode (debug vs release) or compiler type.

Note that several toolchains require you to also define a `python_bootstrap`
toolchain. This is because those toolchains use Python scripts (e.g. to generate
compilation databases for C++), and the prelude gives you control over the
Python toolchain used for this purpose. The bootstrap Python toolchain can be
different from the toolchain used by `python_*` rules.
`@prelude//toolchains:python.bzl` provides a simple
`system_python_bootstrap_toolchain` in case you do not care about tightly
controlling the Python used when building other languages.

### Writing a toolchain for a custom language

Toolchains for custom languages (and more generally, any custom process/build)
can also easily be written as rules with `is_toolchain_rule = True` which return
any provider struct (conventionally named `*ToolchainInfo`). The
[prelude's toolchains](https://github.com/facebook/buck2/tree/main/prelude/toolchains),
including `system_cxx_toolchain` referenced earlier, can serve as examples.

There is no technical difference between toolchains defined in the prelude
compared to custom one, just like there is nothing special about languages
supported by the prelude.

### Exposing execution dependencies from a toolchain

Toolchains typically expose their tools (compilers, linters, etc.) as
`attrs.exec_dep()` attributes. This lets Buck2 configure them for the execution
platform and include them in
[execution platform resolution](configurations.md#toolchain-deps). Here is an
example:

```python
FooToolchainInfo = provider(fields = {
    "compiler": provider_field(RunInfo),
    "linter": provider_field(RunInfo),
    "compiler_flags": provider_field(list[str]),
})

def _foo_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        FooToolchainInfo(
            compiler = ctx.attrs.compiler[RunInfo],
            linter = ctx.attrs.linter[RunInfo],
            compiler_flags = ctx.attrs.compiler_flags,
        ),
    ]

foo_toolchain = rule(
    impl = _foo_toolchain_impl,
    is_toolchain_rule = True,
    attrs = {
        "compiler": attrs.exec_dep(providers = [RunInfo]),
        "linter": attrs.exec_dep(providers = [RunInfo]),
        "compiler_flags": attrs.list(attrs.string(), default = []),
    },
)
```

This toolchain can then be instantiated in `toolchains//TARGETS`:

```python
foo_toolchain(
    name = "foo",
    compiler = "//tools:foo_compiler",
    linter = "//tools:foo_linter",
    compiler_flags = ["-O2", "-Wall"],
    visibility = ["PUBLIC"],
)
```

A working example of this pattern can be found in
`tests/targets/toolchain_deps/` in the Buck2 repository.

The [Accessing a toolchain in a build rule
implementation](#accessing-a-toolchain-in-a-build-rule-implementation) section
below shows how a build rule would consume this toolchain.

## Accessing a toolchain in a build rule implementation

Before going any further, note that using a toolchain is often unnecessary. It
is perfectly possible for a build rule to reference regular targets as
dependencies, without the added complexity of defining a toolchain for them. For
example, one can add an (often `default_only`) `attrs.exec_dep` to a build
rule's `attrs` to do code generation in a custom rule without creating a
toolchain for it, as long as there is no need to decouple the code generator
being used from the rule implementation.

With that being said, when a rule must be made generic over a toolchain, all
that is needed is to add that toolchain as a `toolchain_dep` attr to a build
rule. Let's consider the following made up rule that would simply call a
compiler on a source file:

```python
# We assume that `FooToolchainInfo` is a struct with a `compiler` field.
def _foo_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    output = ctx.actions.declare_output(ctx.label.name)
    ctx.actions.run(
        [ctx.attrs._foo_toolchain[FooToolchainInfo].compiler, ctx.attrs.source, "-o", output.as_output()],
        category = "foo_compile",
    )
    return [DefaultInfo(default_outputs = [output])]

foo_binary = rule(
    impl = _foo_binary_impl,
    attrs = {
        "source": attrs.source(),
        "_foo_toolchain": attrs.default_only(attrs.toolchain_dep(default = "toolchains//:foo", providers = [FooToolchainInfo])),
    },
)
```

## Writing a hermetic toolchain

One of the benefits of Buck2 is that it makes it quite easy to write a hermetic
toolchain, meaning one that does not look up tools in the environment, but
instead explicitly downloads and tracks them as part of the build.

Doing is typically as follows:

- Download the tools, e.g. using the
  [`http_archive`](../../prelude/rules/core/http_archive) rule (see the
  [Zig-based C++ toolchain in the prelude](https://github.com/facebook/buck2/tree/main/prelude/toolchains/cxx/zig)
  as an example).
- Expose those tools as [`RunInfo`](../../api/build/RunInfo/) providers in rules
  that are referenced as [exec deps](configurations.md#execution-deps)
  in a toolchain implementation.
