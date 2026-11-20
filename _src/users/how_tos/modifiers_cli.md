---
id: modifiers_cli
title: Specify configuration modifiers from the CLI
---

Modifiers can be specified on the command line to override the values set in
`PACKAGE` files or on targets directly. To do so, pass constraint targets via
the `-m`/`--modifiers` flag:

```sh
# Assuming that `//constraints:BUCK` contains the appropriate constraint
# definitions.
buck build :my_target -m //constraints:debug
buck build :my_target -m //constraints:release
```

Aliases can be used as shorthands (see the [setup how-to](./modifiers_setup.md)
for more information):

```sh
buck build :my_target -m debug
```

Additionally, a subset of commands support the `?modifier` syntax, which allows
users to specify modifiers for a target pattern by appending a `?` to the
pattern followed by the modifiers delimited by `+` (see
[?modifier](./question_mark_modifier.md) page for more information):

```sh
buck2 build :my_target?debug+linux
```
