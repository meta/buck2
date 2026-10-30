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

By default the file name must end in `.toml` or `.json`, and `load()` can only
import the name `value` from these files. For TOML, `value` is always going to be
a dictionary with `str` keys (i.e. a
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

## Files with a different extension

Plenty of data files have no usable extension, and some have a misleading one.
`Cargo.lock` is TOML and `Pipfile.lock` is JSON - the extension they share says
nothing about either. Append `?as=` to the load path to say how the file should be
parsed, and the extension is no longer consulted:

```python
load("//foo:Cargo.lock?as=toml", cargo = "value")
load("//foo:Pipfile.lock?as=json", pipfile = "value")
```

The accepted formats are `json` and `toml`. There is no `?as=bzl`: a file that is
Starlark says so by being named `.bzl`.

A `?as=` load only works on files the repo has opted in, via a comma-separated
list of file name globs:

```ini
[buck2]
  load_as_allowlist = *.lock, versions.txt
```

A load whose file matches no glob is an error naming the buckconfig. Restating
what the extension already says (`:bar.toml?as=toml`) asks for nothing the plain
load doesn't, so it needs no entry.

## An unparseable file fails the whole package

A loaded file is a dependency of the build file that loads it, the same as any
`.bzl`: editing it invalidates the build file and everything downstream. That is
the point - the data is tracked and cached like the rest of the build graph.

It also means an unparseable file is a **load-time** error, and a load-time error
fails every target in the package, not just the ones that use the data. A
malformed `Cargo.lock` will make `buck2 build //foo:some-unrelated-target` fail
too.

## Generating the data out of band

The alternative to loading a file during Starlark evaluation is to keep the build
out of it entirely. Run a tool outside the build, by hand or as a CI step, that
transforms whatever you have into a `.json`, `.toml`, or `.bzl` file, and commit
the result. The build then loads a file that is known to parse, in a shape you
chose.

This is also the answer when the data is in some other format, or needs
rearranging before Starlark can use it: convert it once, out of band, rather than
at load time. The cost is that the committed file can go stale, so it is on you
(or on CI) to re-run the tool when the source changes.
