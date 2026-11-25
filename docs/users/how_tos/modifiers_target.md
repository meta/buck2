---
id: modifiers_target
title: Add configuration modifiers to a specific target
---

Modifiers can be added to individual targets via the `modifiers` attribute which
is exposed by all rules (this is handled by Buck2 itself, so rule authors do not
have to add it themselves).

For example, lets assume that we have a `release_package` rule that references
various `dep`s and packages them, and that our
[`PACKAGE` file defaults our build mode to debug](./modifiers_package.md). We
could add the `release` modifier to our artifact target to build all
dependencies in release mode rather than debug:

```python
release_package(
    name = "my_package",
    binaries = [
        ":my_cxx_bin",
        ":my_rust_bin",
    ],
    modifiers = [
        "root//config:build_mode[release]",
    ],
)
```

As another example, imagine that we have a constraint that controls whether we
use real or simulated IO:

```python
constraint(
    name = "io_mode",
    default = "real",
    values = [
        "real",
        "simulated",
    ],
)
```

The default IO mode is `real`, but we would like to override it for a specific
test:

```python
go_test(
    name = "simulated_io_test",
    srcs = ["test.go"],
    modifiers = ["root//config:io_mode[simulated]"],
)
```

The attribute takes effect when the target is one that the command names:
`buck2 test //:simulated_io_test`. A target that is built as a dependency of
another is built in that other target's configuration, and its `modifiers` are
not read. Modifiers on the [command line](./modifiers_cli.md) win over the
attribute.

The attribute has no effect until
[modifiers have been set up](./modifiers_setup.md).
