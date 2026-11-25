---
id: modifiers_package
title: Add configuration modifiers to entire directories
---

To apply [modifiers](../../concepts/modifiers.md) to every target in a
directory and the directories below it, call the prelude's `set_cfg_modifiers`
in the directory's [`PACKAGE` file](../../concepts/package_files.md):

```python
load("@prelude//cfg/modifier:set_cfg_modifiers.bzl", "set_cfg_modifiers")

set_cfg_modifiers([
    "root//config:build_mode[release]",
])
```

This assumes that [modifiers have been set up](./modifiers_setup.md), and that
`root//config:build_mode` is a constraint with a value `release`.

The modifiers of a `PACKAGE` file are applied after those of the `PACKAGE`
files above it, so for the same constraint the nearest file wins. A target's
own [`modifiers` attribute](./modifiers_target.md) and modifiers on the
[command line](./modifiers_cli.md) win over both.

`set_cfg_modifiers` takes 2 arguments:

- `cfg_modifiers`, a list of modifiers to set.
- `extra_cfg_modifiers_per_rule`, a dictionary mapping rule names to lists of
  modifiers that should be applied only for that rule. Note this is deprecated.

`buck2 audit package-values //some/package` prints the modifiers that `PACKAGE`
files give a package.
