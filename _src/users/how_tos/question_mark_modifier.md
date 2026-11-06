---
id: question_mark_modifier
title: ?modifier CLI Syntax
---

The `?modifier` syntax is an alternative method of specifying CLI level
modifiers, which allows users to specify modifiers for a target pattern by
appending a `?` to the pattern followed by the modifiers delimited by `+`.

```sh
buck2 build repo//foo:bar?debug+linux
```

Since the `?modifier` syntax associates modifiers to target patterns rather than
the command, it allows us to potentially condense multiple command invocations.

```sh
# Using --modifier flags
buck2 build --modifier debug repo//foo:bar
buck2 build --modifier debug --modifier linux repo//foo:bar

# Using ?modifier syntax
buck2 build repo//foo:bar?debug repo//foo:bar?debug+linux
```

### Use in `--target-universe` flag

For command invocations that specify the `--target-universe` flag, the
`?modifier` syntax must be used as the argument of the `--target-universe` flag.

```sh
# Use of `?modifier` syntax in `--target-universe` flag
buck2 build repo//foo:bar --target-universe repo//foo:baz?debug
```

Using the `?modifier` syntax outside of the `--target-universe` flag when it is
specified will throw an error.

```sh
# Command that would fail and throw an error
buck2 build repo//foo:bar?debug --target-universe repo//foo:baz
```

### Incompatible with `--modifier` flag

If the `?modifier` syntax is used in a command invocation, the `--modifier` flag
cannot be used.

```sh
# Commands that would fail and throw an error
buck2 build --modifier debug repo//foo:bar?linux
buck2 build --modifier debug repo//foo:bar --target-universe repo//foo:baz?linux
```

### Commands that support `?modifier` syntax

Currently, the following commands support the `?modifier` syntax:

- `build`
- `cquery` (only in `--target-universe` flag)
- `ctargets`
- `run`
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
buck2 build --show-output repo//foo:bar?debug repo//foo:bar?linux

# Example output
repo//foo:bar?debug buck-out/v2/gen/...
repo//foo:bar?linux buck-out/v2/gen/...
```

#### `?modifier` syntax in `--build-report` flag

```sh
# CLI input
buck2 build repo//foo:bar?debug repo//foo:bar?linux --build-report repo/build_report.json
```

```json
// Build report output in repo/build_report.json with ommited fields
{
   "results": {
       "repo//foo:bar?debug":  {
           "success": "SUCCESS",
           ...
       },
       "repo//foo:bar?linux":  {
           "success": "SUCCESS",
           ...
       },
   }
}

```
