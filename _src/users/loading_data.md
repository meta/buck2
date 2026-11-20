---
id: loading_data
title: Loading Data in Starlark
---

You can load static data from within `BUCK` or `.bzl` files, as long as that
static data is stored in a JSON or TOML file in a package.

```python
load("//foo:bar.toml", "value")

some_rule(
    name = "my_rule",
    data = value,
)
```

This is useful for example if the data being loaded is generated outside of the
build process, but is necessary for the build itself - like URLs and checksums
being passed into
[`http_archive`](https://buck2.build/docs/prelude/rules/core/http_archive/).

The file name must end in `.toml` or `.json`, and `load()` can only import the
name `value` from these files. For TOML, `value` is always going to be a
dictionary with `str` keys (i.e. a
[TOML table](https://toml.io/en/v1.0.0#table)), and for JSON it maps to whatever
value is in the file.

If a name other than `value` is needed, `load()` allows aliasing:

```python
load("//foo:bar.toml", my_more_specific_name = "value")

print(my_more_specific_name)
```

TOML types map unambiguously to JSON, and JSON types map unambiguously to
Starlark like you might expect:

- `null` maps to `None`, and `true`/`false` maps to `True`/`False`
- numbers map to `int` or `float`
- strings map to `str`
- arrays map to `list`
- objects map to `dict`
- tables (in TOML) map to `str`-keyed `dict`
