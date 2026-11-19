---
id: custom_macros
title: Macros
---

# Macros

It is possible to define Starlark functions that have the side-effect of
creating build targets. Such functions are called _macros_.

## On this page

- [How to define a macro](#defining)
- [Compound build targets: macros that expand to multiple targets](#compound-targets)
- [How to view expanded macros](#viewing)

## How to define a macro {#defining}

We require that you define and maintain your macros in files that are external
to your build files. These files must have an extension; we recommend that you
use the extension, `.bzl`.

To make your macros accessible to a build file, import them using the `load()`
function.

In the following example, the macro `java_library_using_guava`, defined in the
file `java_macros.bzl`, invokes a macro named `java_library` that depends on the
Google Guava libraries.

**`java_macros.bzl`**

```python
def java_library_using_guava(
    name,
    srcs=[],
    resources=[],
    deps=[],
    visibility=[]):
  java_library(
    name = name,
    srcs = srcs,
    resources = resources,
    deps = [
      # This assumes this is where Guava is in your project.
      '//third_party/java/guava:guava',
    ] + deps,
    visibility = visibility,
  )
```

Instantiating this macro looks the same as defining a built-in build rule. In
the following code, we assume that `java_macros.bzl` is stored in the
subdirectory `libs/java_libs/team_macros`.

```python
#
# load the macro from the external file
#
load("//libs/java_libs/team_macros:java_macros.bzl", "java_library_using_guava")

#
# Calling this function has the side-effect of creating
# a java_library() rule named 'util' that depends on Guava.
#
java_library_using_guava(
  name = 'util',
  # Source code that depends on Guava.
  srcs = glob(['*.java']),
)
```

## Compound build rules: macros that expand to multiple rules {#compound-targets}

:::caution

While this is supported we strongly recommend you instead write user
defined rules since complex macros have performance and maintainability
drawbacks.

:::

You can also create more sophisticated macros that expand into multiple build
rules. For example, you could create a macro that produces targets for both
debug and release versions of an APK:

```python
def create_apks(
    name,
    manifest,
    debug_keystore,
    release_keystore,
    proguard_config,
    deps):

  # This loop will create two android_binary rules.
  for type in [ 'debug', 'release' ]:
    # Select the appropriate keystore.
    if type == 'debug':
      keystore = debug_keystore
    else:
      keystore = release_keystore

    android_binary(
      # Note how we must parameterize the name of the
      # target so that we avoid creating two build
      # targets with the same name.
      name = '%s_%s' % (name, type),
      manifest = manifest,
      keystore = keystore,
      package_type = type,
      proguard_config = proguard_config,
      deps = deps,
      visibility = [
        'PUBLIC',
      ],
    )
```

As in the previous example, instantiating this macro _looks_ the same as
specifying a single rule:

```python
create_apks(
  name = 'messenger',
  manifest = 'AndroidManifest.xml',
  debug_keystore = '//keystores:debug',
  release_keystore = '//keystores:prod',
  proguard_config = 'proguard.cfg',
  deps = [
    # ...
  ],
)
```

However, instantiating this macro actually creates _two_ targets. For example,
if you instantiated this macro in the build file, `apps/messenger/BUCK`, it
would create the following rules:

```
//apps/messenger:messenger_debug
//apps/messenger:messenger_release
```

Note, though, that in this scenario, the following is **NOT** a target:

```
//apps/messenger:messenger              # MACRO, NOT A TARGET
```

Therefore, the following commands do not work, which could be confusing for
developers who don't realize that `messenger` is a macro rather than a target.

```
buck build //apps/messenger:messenger    # FAILS
buck targets --type create_apks          # FAILS
```

## How to view expanded macros {#viewing}

Use `buck targets` to view the resulting targets after expanding all macros. The
following invocation of `buck targets` show the resulting targets from the
preceding example, but not the macro that created them.

```
buck targets fbsource//fbandroid/apps/messenger/...
```
