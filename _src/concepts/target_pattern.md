---
id: target_pattern
title: Target Pattern
---

A _target pattern_ is a string that resolves to a set of
[targets](./glossary.md#target). A target pattern can be used as arguments to
commands, such as `buck2 build` and `buck uquery`. You can also use build target
patterns in the [visibility](./glossary.md#visibility) argument of your build
[rules](./glossary.md#rule).

The simplest build target pattern matches the build target of the same name:

```bash
#
# Matches //apps/myapp:app
#
//apps/myapp:app
```

A build target pattern that ends with a colon matches all build targets in the
build file at the preceding directory path. For example, suppose that the build
file:

```sh
apps/myapp/BUCK
```

defines the rules: `app_v1` and `app_v2`, then the following build target
pattern would match both of those rules:

```bash
#
# Matches //apps/myapp:app_v1 and //apps/myapp:app_v2
#
//apps/myapp:
```

A build target pattern that ends with an ellipsis (`/...`) matches all build
targets in the build file in the directory that precedes the ellipsis and also
_all build targets in build files in subdirectories_. For example, suppose that
you have the following build files:

```bash
apps/BUCK
apps/myapp/BUCK
```

then the following pattern would match all build targets in both of those files:

```bash
#
# Matches (for example) //apps:common and //apps/myapp:app
#
//apps/...
```

A target pattern that does not include a `:` separator matches the target with
the same name as the last element of the path:

```bash
#
# Matches //apps/myapp:myapp
#
//apps/myapp
```

Finally, target patterns can be relative to your current directory. For example:

```bash
#
# If your current working directory is `apps`, matches //apps/myapp:myapp
#
myapp:myapp
```

### Build target patterns are not allowed in the deps argument

Build target patterns cannot be used with the `deps` argument of a build rule.
Buck requires that you specify all dependencies explicitly as either
fully-qualified or relative build targets.

### Target aliases

Buck supports the ability to define _aliases_ for build targets; using aliases
can improve brevity when specifying targets on the Buck command line.

To see which aliases exist, use `buck2 audit config alias`.
