# Return error in `ProviderCollection[]` on undeclared provider

Currently, `ctx.attrs.foo[UnknownInfo]` returns `None` if `foo` is a provider
collection.

This RFC proposes these changes:

- `ctx.attrs.foo[UnknownInfo]` is an error
- `UnknownInfo in ctx.attrs.foo` is `False`
- `ctx.attrs.foo.get(UnknownInfo)` returns `None`

## Why

Better diagnostics when accessing unknown provider. E. g. when writing:

```python
ctx.attrs.foo[UnknownInfo].bar
```

Currently, the error is:

```
Object of type `NoneType` has no attribute `bar`
```

Instead, the error will be something like:

```
provider collection does not contain `UnknownInfo`,
    defined providers are `FooInfo`, `BarInfo`.
```

## Bazel

In bazel, `[]` on unknown provider is an error, like this:

```
Error: <target //optional_provider:n2> (rule '_sum')
    doesn't contain declared provider 'UnknownInfo'
```
