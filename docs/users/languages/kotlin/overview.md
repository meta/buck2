---
id: overview
title: Overview
---

# Kotlin

Buck2 builds Kotlin with `kotlin_library`, `kotlin_test` and the Java rules, for
example `java_binary`. This page covers the Kotlin toolchain of the prelude's
`system_demo_toolchains`, which projects created with `buck2 init` use.

## Just need an example?

See
**[examples/with_prelude/kotlin](https://github.com/facebook/buck2/tree/main/examples/with_prelude/kotlin)**:

```python
kotlin_library(
    name = "greeting",
    srcs = ["greeting/Greeter.kt"],
    abi_generation_mode = "source_only",
)

kotlin_library(
    name = "app_lib",
    srcs = ["app/Main.kt"],
    abi_generation_mode = "source_only",
    deps = [":greeting"],
)

java_binary(
    name = "app",
    main_class = "com.example.app.MainKt",
    deps = [
        ":app_lib",
        "prelude//toolchains/android/third-party:kotlin-stdlib",
    ],
)
```

`buck2 run //kotlin:app` prints `Hello, Buck2!`.

## Requirements

- **A JDK.** The toolchain uses the JDK in `[java] java_home` of the project's
  `.buckconfig`, or else the JDK whose `java` is on `PATH`:

  ```ini
  [java]
    java_home = /usr/lib/jvm/java-21-openjdk
  ```

  The prelude's own Java and Kotlin tools are built for Java 21. With an older
  JDK, build them for its version instead, for example
  `buck2 build -c java.buck2_java_version=17 //...`.

- **Maven Central** on the first build: the toolchain downloads the Kotlin
  compiler and the libraries of its tools as `mvn:` remote files. To use a
  mirror, pass `-c http.maven_repo_override=https://maven.example.com/maven2`.

## Kotlin version

The toolchain uses Kotlin 2.3.21. To use Kotlin 2.4.21 instead, set it in the
project's `.buckconfig`:

```ini
[kotlin]
  kotlin_version = 2.4.21
```

These are the versions the prelude has the jars of; any other version fails
with `No sha1 for kotlin-stdlib <version>`.

## The Kotlin standard library

`kotlin_library` compiles against the toolchain's Kotlin standard library but
does not package it. A binary that runs Kotlin code depends on
`prelude//toolchains/android/third-party:kotlin-stdlib` itself, as `:app` does
above; without it, the program fails with
`NoClassDefFoundError: kotlin/jvm/internal/Intrinsics`.

## Persistent worker

The toolchain runs Kotlin compiles in a
[persistent worker](../../../rule_authors/persistent_workers.md): one JVM that
compiles every Kotlin target of a build, so the compiles share its startup and
its JIT-compiled compiler. `[build] use_persistent_workers = false` gives each
compile a JVM of its own instead.

## Source-only ABIs

A target with `abi_generation_mode = "source_only"` gets its ABI JAR from
**Kosabi**, a set of compiler plugins that generate it from the target's own
sources. Targets that depend on it compile against that ABI, so they do not wait
for it, or for anything it depends on, to compile. That flattens the build
graph: the more targets use it, the more compile in parallel. See
[Java/Kotlin ABIs](../java/abis.md) for ABIs in general.

Kosabi sees only the target's sources, so a declaration that other targets can
use needs a type that those sources spell out. Give public and protected
functions and properties explicit types when their type would otherwise be
inferred from another target:

```kotlin
// The type of `greet` would come from `Greeter` in another target.
fun greet(): String = Greeter(DEFAULT_NAME).greet()
```

Without the `: String`, the target's source-only ABI action fails with
`Cannot serialize error type`.

The toolchain enables Kosabi by default. `[kotlin] enable_source_only_abi =
false` turns it off, and then `source_only` targets build class ABIs instead.

## Building the Kotlin compiler

[`examples/kotlin_compiler`](https://github.com/facebook/buck2/tree/main/examples/kotlin_compiler)
generates BUCK files for the compiler in
[JetBrains/kotlin](https://github.com/JetBrains/kotlin) 2.4.21, 102 modules,
and builds it with the prelude's Kotlin toolchain, with class or source-only
ABIs.
