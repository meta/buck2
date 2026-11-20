<!-- Source: https://docs.google.com/document/d/1h5JGzXN11__wezO3sfaRdypn6SM1q36YvxPQe8d5bAs -->

# [RFC] `?modifier` syntax

`?modifier` syntax for target patterns on CLI allows building with multiple combinations of modifiers. This enables building in multiple configurations at the same time from the same CLI invocation

## Basic syntax

The following is copied from the command-line modifiers section of the original [modifiers RFC](https://docs.google.com/document/d/1yfb2mwnsphGiz9FhftkIGwMorK8is1qq3f_gYOEmkMA/edit?tab=t.0).

Modifiers from `?` syntax are specified as `buck2 build <target pattern>?<modifiers separated by plus signs>`.

For example, `buck2 build repo//foo:bar?prelude//constraints/sanitizer:asan` applies asan modifier on the command line. `buck2 build repo//foo:bar?prelude//constraints/os:linux+prelude//constraints/sanitizer:asan` will apply linux and asan modifiers.

Modifiers can be specified for any target pattern, so `buck2 build repo//foo/...?asan` and `buck2 build repo//foo:?asan` are both valid.

When specifying a subtarget and modifier with `?`, subtarget should go before the modifier, ex. `buck2 build repo//foo:bar[comp-db]?asan`. This configures `repo//foo:bar` against `asan` modifier and then builds just the `comp-db` subtarget.

It is prohibited to specify both `--modifier` flag and `?` on CLI. This restriction may be removed in the future after implementation of this RFC provided we see good motivation for it.

`?modifier` syntax is only allowed on CLI and certain parts of BXL that are CLI-like. It is only meant to express convenient configurations on CLI. It will be strictly prohibited on any non-CLI surfaces like BUCK files.

## `--show-output`

Buck’s build commands accept a set of `--show-output` flags (ex. `--show-output` and `--show-full-output`) that prints the output location of targets specified on CLI. For example, invoking `buck2 build fbcode//buck2:buck2 –show-output` prints

```python
fbcode//buck2:buck2 buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2
```

Likewise, invoking build in a different mode like `buck2 build fbcode//buck2:buck2 -m opt` will print a different path

```python
fbcode//buck2:buck2 buck-out/v2/gen/fbcode/b706492dec65e54c/buck2/buck2
```

With `?`-syntax, users would be able to invoke `buck2 build fbcode//buck2:buck2 fbcode//buck2:buck2?opt` in the same invocation. For that, we propose to use the following output structure.

```python
fbcode//buck2:buck2 buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2
fbcode//buck2:buck2?opt buck-out/v2/gen/fbcode/b706492dec65e54c/buck2/buck2
```

This preserves modifiers in the exact same way that is specified from the CLI invocation, which allows users to differentiate which path belongs to dev modifier and which path belongs to opt modifier, without needing to understand very much about modifiers.

## Build Report

Current build report for `buck2 build fbcode//buck2:buck2` looks as follows. For readability, we will skip irrelevant fields.

```python
{
  "results": {
    "fbcode//buck2:buck2": {
      "success": "SUCCESS",
      "outputs": {
        "DEFAULT": [
          "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
        ]
      },
      "other_outputs": {},
      "configured": {
        "cfg#57b1cdd23074b8c3": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
            ]
          },
          "other_outputs": {}
        }
      },
      "errors": []
    }
  },
  # other fields
}
```

When `?`-syntax is used, we will also preserve the modifiers in the build report results key. For example, this is what the build report looks like with `buck2 build fbcode//buck2:buck2 fbcode//buck2:buck2?opt`.

```python
{
  "results": {
    "fbcode//buck2:buck2": {
      "success": "SUCCESS",
      "outputs": {
        "DEFAULT": [
          "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
        ]
      },
      "other_outputs": {},
      "configured": {
        "cfg#57b1cdd23074b8c3": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
            ]
          },
          "other_outputs": {}
        }
      },
      "errors": [],
      "target_label": "fbcode//buck2:buck2"
    },
    "fbcode//buck2:buck2?opt": {
      "success": "SUCCESS",
      "outputs": {
        "DEFAULT": [
          "buck-out/v2/gen/fbcode/b706492dec65e54c/buck2/buck2"
        ]
      },
      "other_outputs": {},
      "configured": {
        "cfg#b706492dec65e54c": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/b706492dec65e54c/buck2/buck2"
            ]
          },
          "other_outputs": {}
        }
      },
      "errors": [],
      "target_label": "fbcode//buck2:buck2"
    }
  },
  # other fields
}
```

The keys in `results` are `fbcode//buck2:buck2` and `fbcode//buck2:buck2?opt`. Since `fbcode//buck2:buck2?opt` is not a proper target label, we add a key called “target_label” which will display the target label without any modifiers (in this case `fbcode//buck2:buck2`). We will also add a ?`modifiers` key that will resolve to a list of all modifiers applied after `?`.

### Alternate Design

A possible alternate design is that we add a `per_target_modifiers` section of the build report similar to the `configured` section.

```python
{
  "results": {
    "fbcode//buck2:buck2": {
      "success": "SUCCESS",
      "outputs": {
        "DEFAULT": [
          "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
        ]
      },
      "other_outputs": {},
      "configured": {
        "cfg#57b1cdd23074b8c3": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
            ]
          },
          "other_outputs": {}
        },
        "cfg#b706492dec65e54c": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/b706492dec65e54c/buck2/buck2"
            ]
          },
          "other_outputs": {}
        }
      },
      "errors": [],
      "modifiers": {
        "null": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/57b1cdd23074b8c3/buck2/buck2"
            ]
          },
          "other_outputs": {}
        },
        "opt": {
          "errors": [],
          "success": "SUCCESS",
          "outputs": {
            "DEFAULT": [
              "buck-out/v2/gen/fbcode/b706492dec65e54c/buck2/buck2"
            ]
          },
          "other_outputs": {}
        }
      },
    }
  },
  # other fields
}
```

The main reason to prefer approach #1 is that it better accommodates tools that wrap a user’s buck build invocation and do extra processing on the build report. With approach #2, if a user passes in `fbcode//buck2:buck2?opt` as the target pattern to build, then the wrapper tool needs to understand that `?opt` specifies a modifier and that it needs to do a string split on `?` to find the correct section of the build report. With approach #1, it can look up `fbcode//buck2:buck2?opt` directly from the build report without understanding that a modifier was used.

The benefit of approach #2 is that the `configured` section looks more understandable in this approach than the previous approach, and in general it leads to a shorter build report.

## Target Universe

A [target universe](https://buck2.build/docs/concepts/glossary/#target-universe) is a set of configured targets and their transitive deps that Buck looks up from to resolve unconfigured target labels. For example, `buck2 build fbsource//xplat/folly:singleton --target-universe=fbcode//buck2:buck2` will build all configured variants of `fbsource//xplat/folly:singleton` in transitive deps of `fbcode//buck2:buck2`. This section applies to all commands that can explicitly use the `--target-universe` flag like `audit providers` and `aquery`.

### Explicit target universe

If `--target-universe` flag is specified on CLI, then `?` can only be used in `--target-universe` flag. In other words,

- `buck2 build fbsource//xplat/folly:singleton --target-universe=fbcode//buck2:buck2?asan` is allowed.
- Likewise, `buck2 build fbsource//xplat/folly:singleton --target-universe=fbcode//buck2:buck2+fbcode//buck2:buck2?asan` is allowed.
- `buck2 build fbsource//xplat/folly:singleton?asan --target-universe=fbcode//buck2:buck2?asan` is not allowed.
- Likewise, `buck2 build fbsource//xplat/folly:singleton?asan --target-universe=fbcode//buck2:buck2` is also not allowed.

The above examples all look at builds, but the same principles apply to other commands that can use `--target-universe` like `audit providers` and `cquery`.

This behavior also roughly matches how `--modifier` flag works with `--target-universe` today, which is that `--modifier` is only applied to resolve the target universe, and not the target patterns specified.

This is probably not the most ideal behavior, but it is probably the more restrictive behavior, meaning it will be easier for us to change this behavior in the future if we find a use case that needs it.

## Cquery

In cquery, ?-syntax will *only* be allowed in `--target-universe`. This means that

- `buck2 cquery fbsource//xplat/folly:singleton --target-universe=fbcode//buck2:buck2?asan` is allowed
- `buck2 cquery fbsource//xplat/folly:singleton?asan –target-universe=fbcode//buck2:buck2?asan` and `buck2 cquery fbsource//xplat/folly:singleton?asan --target-universe=fbcode//buck2:buck2` are disallowed.
- Additionally, `buck2 cquery fbsource//xplat/folly:singleton?asan` is *disallowed*. The reason for this is that cquery [infers a target universe](https://buck2.build/docs/bxl/explanation/bxl_cquery_vs_cli_cquery/#cli-buck2-cquery) from all target literals specified in the query when explicit `--target-universe` is not specified. Thus `buck2 cquery fbsource//xplat/folly:singleton?asan` naturally expands to `buck2 cquery fbsource//xplat/folly:singleton?asan --target-universe=fbsource//xplat/folly:singleton?asan`.

### Possible relaxation

Not being able to specify `?` in cquery outside of `--target-universe` is rather unintuitive behavior, and it’s ironic that a `cquery` command cannot easily customize configurations of targets. It’s possible that we may allow  `buck2 cquery fbsource//xplat/folly:singleton?asan` and by extension `buck2 cquery fbsource//xplat/folly:singleton?asan --target-universe=fbsource//xplat/folly:singleton?asan` in the future.

One possible relaxation is that if a literal in a query expression is specified with a `?modifier`, then that target literal will resolve in the target universe according to its modifiers applied instead of matching configured targets with the same unconfigured target label in the target universe.

Take the example of `buck2 cquery set(fbcode//buck2:buck2 fbsource//xplat/folly:singleton)`. This will print multiple variants of `fbsource//xplat/folly:singleton` in different configurations because `fbsource//xplat/folly:singleton` shows up in deps of `fbcode//buck2:buck2` and will be configured once in its default configuration. Let’s assume the output looks something like this.

```python
fbcode//buck2:buck2 (cfg1)
fbsource//xplat/folly:singleton (cfg1)
fbsource//xplat/folly:singleton (cfg2)
```

If a user invokes `buck2 cquery set(fbcode//buck2:buck2 fbsource//xplat/folly:singleton?asan)`, then `fbsource//xplat/folly:singleton?asan` will not resolve to any copies of folly in deps of `fbcode//buck2:buck2`, so output will look like this.

```python
fbcode//buck2:buck2 (cfg1)
fbsource//xplat/folly:singleton (cfg3)
```

Note that all literals with modifiers applied still go through target universe resolution. If a user invokes `buck2 cquery “set(fbcode//buck2:buck2?asan fbsource//xplat/folly:singleton)”`, then `fbsource//xplat/folly:singleton` will continue to be resolved in the target universe of `fbcode//buck2:buck2?asan`, so the output will look like

```python
fbcode//buck2:buck2 (cfg4)
fbsource//xplat/folly:singleton (cfg2)
fbsource//xplat/folly:singleton (cfg4)
```

To list the exact behavior in this scenario,

- `buck2 cquery fbsource//xplat/folly:singleton?asan --target-universe=fbsource//xplat/folly:singleton?asan` is allowed.
- `buck2 cquery fbsource//xplat/folly:singleton?asan –target-universe=fbcode//buck2:buck2` is not allowed because `fbsource//xplat/folly:singleton?asan` is not directly specified in the target universe.

We may consider relaxing to this behavior in the future if there are demands for it.

### Another possible relaxation

Additionally, it’s possible that we allow `buck2 cquery fbsource//xplat/folly:singleton?asan –target-universe=fbcode//buck2:buck2 `to resolve `fbsource//xplat/folly:singleton` to its configuration with asan applied possibly outside of target universe of `fbcode//buck2:buck2` in the future. Unfortunately, this is unintuitive in a couple ways.

- `fbsource//xplat/folly:singleton?asan` likely resolves to a configured target outside of the target universe of `fbcode//buck2:buck2?asan`. If we were to respect `asan` modifier when resolving `fbsource//xplat/folly:singleton`, then the build command will build nothing. If we don’t respect `asan` modifier on `fbsource//xplat/folly:singleton`, then we will be ignoring the modifiers specified and building all folly singletons that show up in deps of buck2. Neither behavior would be intuitive for the average user.
- With :`foo?modifiers`, Buck will actively configure `:foo` outside of the target universe. With just `:foo`, buck will not attempt to do that. This behavior is inconsistent and a bit unintuitive.

## Commands that receive unconfigured targets

Commands that operate on unconfigured targets like `utargets` and `uquery` should error when `?modifier` is specified. This can be relaxed in the future if it is found that it is better to ignore modifiers instead.

## BXL

By default, passing a target with `?modifier` to `cli_args.target_label()` should be an error.

We can introduce a `cli_args.configured_target_label()` to allow passing in configured target label with `?modifier`.

## Testing

Testing should understand `fbcode//foo:test?opt+asan` as target `fbcode//foo:test` with `opt` and `asan` modifiers applied, similar to `fbcode//foo:test -m opt -m asan`.

## Logging

### Target patterns

Target patterns scuba column will not include any `?modifier` used.

### CLI modifiers

CLI modifiers column today tracks a list of modifiers used on CLI with `--modifier` flag. This is insufficient for understanding how modifiers are used with `?modifiers` since a user can now supply multiple lists of modifiers on CLI. To deal with this, we will introduce a new column called “Per-Target CLI Modifiers” (or “CLI ?Modifiers”) that will track `?modifiers` used as a normvector of strings, where each string is a *list* of modifiers from `?modifiers`.

For example,

- `buck2 build fbcode//buck2:buck2?asan+opt` will populate a normvector of `[“asan,opt”]`.
- `buck2 build fbcode//buck2:buck2?asan+opt fbcode//buck2:buck2?dev` will populate a normvector of `[“asan,opt”, “dev”]`.
- `buck2 build fbcode//buck2:buck2?asan+opt fbcode//buck2:buck2` will populate a normvector of `[“”, “asan,opt”]`.
- `buck2 build fbcode//buck2:buck2` will set the column to null (?).

Note when `?modifiers` is used, the CLI modifiers column will be null. Likewise, when `-m modifier` is used, the Per-Target CLI Modifiers column will be set to null.
