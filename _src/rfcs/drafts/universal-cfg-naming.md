# Universal Configuration Naming Function

_tl;dr:_ This RFC proposes using a single naming function to generate names for
all configurations.

## Context

NOTE: The configuration name consists of a readable string followed by the hash
of the configuration. The readable string is technically the `PlatformInfo`
name. For sake of ease of writing, this doc uses configuration name and platform
name interchangeably to describe this concept.

Currently, there are 3 ways to create and name a configuration.

1. A `platform` target defines a configuration, and the platform target label
   becomes the platform name.
2. A transition function defines the configuration and generates a name for the
   configuration.
3. When a modifier is used, the cfg constructor function for modifiers defines
   the configuration and its name. There is currently a single naming function
   that generates all modifier-based configuration names.

Modifiers are intended to replace platforms, so in the future all configuration
names will be generated. Unfortuately, most of the generated names today used
today in transitions are not very good. Problems that I've seen in practice
include:

1. Configuration names barely contain any useful information about the
   configuration. This happens a lot in transitions. For example, the android
   split CPU architecture transition names the generated configurations "x86_64"
   and "arm64", which tells very little about the configuration beyond the CPU
   architectures it splits on.
2. Transition function incorrectly retains the old configuration name that is no
   longer relevant, misleading the user about what this configuration actually
   does. I've seen this happen where a configuration has py3.8 in name but the
   python version constraint stored is actually py3.10.

## Proposal

Register a single Starlark function to define all configuration names. This
Starlark function would accept a `ConfigurationInfo` and return a string for the
name of the `ConfigurationInfo`.

```python
# Example
def name(cfg: ConfigurationInfo) -> str:
   # ...
```

`PlatformInfo` is no longer available in Starlark. Any place that previously
uses a `PlatformInfo` will now use `ConfigurationInfo` instead. Buck2 will
invoke this function each time it encounters a new `ConfigurationInfo` to define
its name.

This function will attempt to provide a useful name based on the constraints in
the configuration, which mitigates the issue of short or misleading
configuration names. There are some risks that there will be high amount of code
complexity in a function if all configurations are named by one function.

This function will most likely be registered via a `set_cfg_name` function or
something callable from root PACKAGE file or potentially prelude.
