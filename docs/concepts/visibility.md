---
id: visibility
title: Visibility
---

Visibility determines whether a [target](./glossary.md#target) can reference
another target in its [attributes](./glossary.md#attribute). In a large project,
you may want to prevent developers from 'reaching across' the project and
pulling in additional code. Reducing the visibility of targets can help prevent
that type of behavior.

There are two types of visibility attributes available, each of which takes a
list of [target patterns](./glossary.md#target-pattern). (Note: `visibility` and
`within_view` arguments may be defined using
[package() rules](../../rule_authors/package_files/#package)):

- `visibility` - determines which other targets can depend on a target.

  This allows for controlling the products/features that may consume your code
  or the clients which your team may choose to support.

- `within_view` - determines which other targets a target can depend on.

  On an individual target, this is very similar to `deps`. If any of the
  target's `deps` are not `within_view`, the target cannot be built. For this
  reason, on an individual target, `within_view` is less useful since for each
  additional `dep`, you must consider updating the within_view entries.

  However, the utility of within_view is expanded when used in conjunction with
  [package() rules](../../rule_authors/package_files/#package) which allow
  defining both `visibility` and `within_view` attributes for multiple targets
  in a scalable manner.

In general, if a target is not listed, then there may be no dependency
relationships as both attributes act as allowlists. However, there are some
exceptions:

- _Empty or Unset `within_view` List_: If the `within_view` list is empty or
  unset, it does not impose any restrictions on which targets the current target
  can depend on.
- _Empty or Unset `visibility` List_: If the `visibility` list is empty or
  unset, then only targets defined in the same
  [BUCK file](./glossary.md#buck-file) can depend upon the current target.
- _Special Value: `'PUBLIC'`_: `visibility` can be set to a special value
  `'PUBLIC'` which makes a build rule visible to all targets. (Example below)

In case of logically-conflicting lists, `within_view` takes precedence over
`visibility`. If `//foo:bar` defines `//hello:world` in its `visibility` list,
but `//hello:world` does not define `//foo:bar` in its `within_view` list, then
`//hello:world` may not depend on `//foo:bar`.

## Examples

A common library like Guava should be able to be included by any build rule:

```python
prebuilt_jar(
  name = 'guava',
  binary_jar = 'guava-14.0.1.jar',
  visibility = ['PUBLIC']
)
```

It is common to restrict the visibility of Android resources to the Java code
that uses it:

```python
android_resource(
  name = 'ui_res',
  res = 'res',
  package = 'com.example',
  visibility = ['//java/com/example/ui:ui']
)
```

Or it may be simpler to make it visible to the entire directory in case
additional build rules are added to `java/com/example/ui/BUCK`:

```python
android_resource(
  name = 'ui_res',
  res = 'res',
  package = 'com.example',
  visibility = ['//java/com/example/ui:']
)
```

Also, it is common to limit code for testing to be visible only to tests. If you
define all of your Java unit tests in a folder named `javatests/` in the root of
your project, then you could define the following rule to ensure that only build
rules under `javatests/` can depend on JUnit:

```python
prebuilt_jar(
  name = 'junit',
  binary_jar = 'junit-4.11.jar',
  visibility = ['//javatests/...']
)
```

Finally, restricting the view of a target can be useful for preventing
dependency creep:

```python
java_library(
  name = 'example',
  visibility = ['PUBLIC',],
  within_view = ['//foo:bar','//hello:world']
)
```

## Auditing visibility

Use [`buck2 audit visibility`](../../users/commands/audit#buck2-audit-visibility) to verify that no `visibility` constraints
are violated in the transitive dependency graph of the given targets.

```sh
buck2 audit visibility //myproject/...
```
