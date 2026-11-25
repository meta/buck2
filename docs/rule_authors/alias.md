---
id: alias
title: Alias
---

The `alias` rule creates another name by which an existing rule can be referred
to. There are two variants: [versioned_alias](#versioned_alias) and
[configured_alias](#configured_alias), which are detailed below.

## alias

The `alias` rule has the following relevant attributes:

- `name` - (required) what the `actual`'s label should be aliased as.
- `actual` - (required) a target label.

**Example**

```python
filegroup(
    name = "foo",
    srcs = ["foo.txt"],
)

alias(
    name = "other_foo",
    actual = ":foo",
)
```

## versioned_alias

The `versioned_alias` rule has the following relevant attributes:

- `name` - (required) what the `actual`'s label should be aliased as.
- `versions` - (required) a map of versions to their respective versioned target
  labels.

Under the hood, any versioned parameters from the `versioned_alias`'s underlying
`actual` are translated into their `select`-based equivalents, which rely on
constraint settings added to the target platform. The `select` keys are the
constraint values `ovr_config//third-party/<project>/constraints:<version>`,
where `<project>` is the name of the directory that contains the build file.

<OssOnly>

The open-source prelude does not have these constraint targets, and in a project
made with `buck2 init` the `ovr_config` cell is the bundled prelude. So
`versioned_alias` cannot be used in such a project.

</OssOnly>

**Example**

```Python
versioned_alias(
    name = "foo",
    versions = {
        # Target labels for foo versions
        "1.1": "//path/to/lib/1.1:foo",
        "1.2": "//path/to/lib/1.2:foo",
    },
    visibility = [
        "PUBLIC",
    ],
)
```

## configured_alias

The `configured_alias` rule has the following relevant attributes:

- `name` - (required) what the `actual`'s label should be aliased as.
- `actual` - (required) a target label.
- `platform` - (required) the platform to build the aliased target with.
- `fallback_to_unconfigured_alias` - if `True` and `platform` is `None`, the
  alias behaves like `alias`. Defaults to `False`.

:::note

`configured_alias` is a macro. It sets the `configured_actual` attribute of the
underlying rule to the pair of `actual` and `platform` (mapped to a configured
dep under the hood so the providers can be simply forwarded), or sets
`fallback_actual`, an unconfigured dep, when it falls back to an unconfigured
alias. The `actual` attribute of the underlying rule is not used under the hood
(to keep compatibility of output format with Buck1 queries).

:::

Outside of simply pointing at another target, this target has one other useful
feature - it contains a platform argument.

This makes the alias rule useful for two distinct scenarios:

- **Configuration switching during the build**. For example, there is an iOS
  target that needs to build a dependency for WatchOS so it can include it in
  the bundle. This can be represented by the iOS target having a dependency on
  an alias of the Watch app with `platform = "//the/desired/watchos:platform"`.
- **Using a target to refer to another in a non-standard configuration**. For
  example, if you want to have an experimental version of an app, you could
  represent that as an alias with an 'experimental' configuration pointing to
  the original target.

**Example**

```Python
configured_alias(
    name = "foo-with-platform1",
    actual = "//lib:foo",
    platform = "//some_config:platform1",
    visibility = ["PUBLIC"],
)
```
