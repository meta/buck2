---
id: modifiers_cli
title: Specify configuration modifiers from the CLI
---

Modifiers can be specified on the command line to override the values set in
`PACKAGE` files or on targets directly. To do so, pass constraint values via
the `-m`/`--modifier` flag:

```sh
# Assuming that `config/BUCK` declares a `build_mode` constraint with the
# values `debug` and `release`.
buck2 build :my_target -m 'root//config:build_mode[debug]'
buck2 build :my_target -m 'root//config:build_mode[release]'
```

Aliases can be used as shorthands (see the [setup how-to](./modifiers_setup.md)
for more information):

```sh
buck2 build :my_target -m debug
```

The flag can be repeated. Modifiers for different constraints all apply. When
two are for the same constraint, the later one wins:

```sh
# `asan` being an alias for a value of another constraint.
buck2 build :my_target -m release -m asan
```

The modifiers apply to every target that the command names.

Additionally, a subset of commands support the `?modifier` syntax, which allows
users to specify modifiers for a target pattern by appending a `?` to the
pattern followed by the modifiers delimited by `+` (see
[?modifier](./question_mark_modifier.md) page for more information):

```sh
buck2 build ':my_target?release+asan'
```

Command-line modifiers are an error until
[modifiers have been set up](./modifiers_setup.md).
