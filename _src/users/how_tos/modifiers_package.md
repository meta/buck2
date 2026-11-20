---
id: modifiers_package
title: Add configuration modifiers to entire directories
---

Modifiers can be set in [`PACKAGE` files](../../rule_authors/package.md) using
the prelude's `set_cfg_modifiers`. In this case they will be applied to all
targets affected by that `PACKAGE` file.

For example, assuming that `root//constraints:debug` is an existing constraint
value and [modifiers have been setup](./modifiers_setup.md), the following will
apply it by default to all targets defined in that `PACKAGE`'s directory:

```python
load("@prelude//cfg/modifier/set_cfg_modifiers.bzl", "set_cfg_modifiers")

set_cfg_modifiers([
    "root//constraints:debug",
])
```

`set_cfg_modifiers` takes 2 arguments:

- `cfg_modifiers`, a list of modifiers to set.
- `extra_cfg_modifiers_per_rule`, a dictionary mapping rule names to lists of
  modifiers that should be applied only for that rule. Note this is deprecated.
