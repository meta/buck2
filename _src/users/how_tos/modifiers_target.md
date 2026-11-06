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
        "//constraints:release",
    ],
)
```

As another example, imagine that we have a constraint that controls whether we
use real or simulated IO:

```python
constraint_setting(name = "io_mode")

constraint_value(
    name = "real_io",
    constraint_setting = ":io_mode",
)

constraint_value(
    name = "simulated_io",
    constraint_setting = ":io_mode",
)
```

The default IO mode would be `real_io`, but we would like to override it for a
specific test:

```python
go_test(
    name = "simulated_io_test",
    srcs = ["test.go"],
    modifiers = ["//constraints:simulated_io"],
)
```
