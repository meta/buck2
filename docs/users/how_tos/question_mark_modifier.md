---
id: question_mark_modifier
title: ?modifier CLI Syntax
---

The `?modifier` syntax is an alternative method of specifying CLI level
modifiers, which allows users to specify modifiers for a target pattern by
appending a `?` to the pattern followed by the modifiers delimited by `+`.

```sh
buck2 build 'repo//foo:bar?debug+asan'
```

The examples on this page use the aliases `debug` and `asan`. A project declares
its aliases when it [sets up modifiers](./modifiers_setup.md); a constraint
value such as `repo//config:build_mode[debug]` can be written in their place.

Since the `?modifier` syntax associates modifiers to target patterns rather than
the command, it allows us to potentially condense multiple command invocations.

```sh
# Using --modifier flags
buck2 build --modifier debug repo//foo:bar
buck2 build --modifier debug --modifier asan repo//foo:bar

# Using ?modifier syntax
buck2 build 'repo//foo:bar?debug' 'repo//foo:bar?debug+asan'
```

### Use in `--target-universe` flag

For command invocations that specify the `--target-universe` flag, the
`?modifier` syntax must be used as the argument of the `--target-universe` flag.

```sh
# Use of `?modifier` syntax in `--target-universe` flag
buck2 build repo//foo:bar --target-universe 'repo//foo:baz?debug'
```

Using the `?modifier` syntax outside of the `--target-universe` flag when it is
specified will throw an error.

```sh
# Command that would fail and throw an error
buck2 build 'repo//foo:bar?debug' --target-universe repo//foo:baz
```

### Incompatible with `--modifier` flag

If the `?modifier` syntax is used in a command invocation, the `--modifier` flag
cannot be used.

```sh
# Commands that would fail and throw an error
buck2 build --modifier debug 'repo//foo:bar?asan'
buck2 build --modifier debug repo//foo:bar --target-universe 'repo//foo:baz?asan'
```

### Commands that support `?modifier` syntax

Currently, the following commands support the `?modifier` syntax:

- `build`
- `cquery` (only in `--target-universe` flag)
- `ctargets`
- `run`
- `test`
- `install`
- `audit providers`
- `audit subtargets`

### Interaction with flags in `build` command

To make it easier to see what output comes from which configuration when using
the `?modifier` syntax. The `?modifier` syntax is also displayed in select flags
in the `build` command.

#### `?modifier` syntax in `--show-output` flag

```sh
# CLI input
buck2 build --show-output 'repo//foo:bar?debug' 'repo//foo:bar?asan'

# Example output
repo//foo:bar?debug buck-out/v2/art/...
repo//foo:bar?asan buck-out/v2/art/...
```

#### `?modifier` syntax in `--build-report` flag

```sh
# CLI input
buck2 build 'repo//foo:bar?debug' 'repo//foo:bar?asan' --build-report repo/build_report.json
```

```json
// Build report output in repo/build_report.json with ommited fields
{
   "results": {
       "repo//foo:bar?debug":  {
           "success": "SUCCESS",
           ...
       },
       "repo//foo:bar?asan":  {
           "success": "SUCCESS",
           ...
       },
   }
}

```
