---
id: jvm_abis
title: Java/Kotlin ABIs
---

# Java/Kotlin ABIs

This topic pertains to building Java code with Buck2.

When compiling a Kotlin or Java rule, Buck2 creates an **Application Binary
Interface (ABI) JAR**, which contains only resources and class interfaces—the
public interface for your module. Buck2 creates this ABI JAR in addition to the
**library JAR**, which contains all of the compiled classes and resources for
the rule.

Since ABI JARs do not contain method bodies or private members, they are smaller
and change less frequently than library JARs. This enables Buck2 to use ABI JARs
in two important ways:

1. **Incremental builds** - ABI JARs help Buck2 more accurately determine which
   rules need to be rebuilt during an incremental build. A Java/Kotlin library
   rule does not necessarily need to be rebuilt if one of its dependencies
   changes, _provided that the public interface of that dependency did not
   change._ This knowledge helps avoid unnecessary rebuilds.

2. **Compilation performance** - ABI JARs can be used on the compiler's
   classpath instead of library JARs. The smaller size of ABI JARs enables the
   compiler to load them faster than library JARs, providing a performance
   boost.

## ABI Generation Modes

Buck2 can create ABI JARs in three different ways, depending on the
`abi_generation_mode` configuration. You can set this globally in `.buckconfig`
or override it per-rule using the `abi_generation_mode` attribute.

### Class ABI Generation

Builds the library JAR first and then strips out unnecessary bits (method
bodies, private members) to create the ABI JAR.

- **Mode value**: `class`
- **Characteristics**: Most conservative approach, always accurate
- **Use when**: You need guaranteed correctness or have complex code that
  doesn't work with other modes

### Source ABI Generation

Hooks into the compiler while it is compiling the library JAR and emits the ABI
JAR partway through the compilation process.

- **Mode value**: `source`
- **Characteristics**: Reduces bottlenecks due to slow rules or low parallelism
  in the build graph
- **Use when**: You want better parallelism but still need compiler validation

### Source-Only ABI Generation

Examines only the text of the source code and uses heuristics to infer things
that can normally be determined only by looking at dependencies.

- **Mode value**: `source_only`
- **Characteristics**: Dramatically increases parallelism and reduces cache
  fetches during incremental builds
- **Use when**: You want maximum build performance and can meet the requirements
  (see below)

## Requirements for Source-Only ABI Generation

Buck2 generates source-only ABI JARs using only the text of the source code for
a rule, without first compiling most of the rule's dependencies. Some details of
an ABI JAR cannot be known for certain from just the source, so Buck2 uses
heuristics to infer those details.

When compiling the library JAR, Buck2 verifies whether the heuristics used for
the ABI JAR were correct. If they were not, Buck2 fails the build with an error.

### Special Attributes

To handle cases where source-only ABI generation needs additional information,
Buck2 provides two attributes:

#### `source_only_abi_deps`

These are dependencies that must be present during source-only ABI generation.

```python
java_library(
    name = "mylib",
    srcs = ["MyClass.java"],
    deps = [
        "//other:dep1",
        "//other:dep2",
    ],
    source_only_abi_deps = [
        "//other:dep1",  # This dep is needed for ABI generation
    ],
)
```

**Important**: Having `source_only_abi_deps` prevents Buck2 from completely
flattening the build graph, reducing the performance win from source-only ABI
generation. These should be avoided when possible. Often only a small code
change is needed to avoid them.

#### `required_for_source_only_abi`

Indicates that this rule must be present on the classpath during source-only ABI
generation of any rule that depends on it. Typically used for rules containing:

- Annotations
- Enums
- Constants
- Interfaces

```python
java_library(
    name = "annotations",
    srcs = ["MyAnnotation.java"],
    required_for_source_only_abi = True,
)
```

**Important**: Having rules marked with `required_for_source_only_abi=True`
prevents Buck2 from completely flattening the build graph. These rules should be
kept small (ideally just containing annotations, constants, enums, and
interfaces) and with minimal dependencies.

### Best Practices for Source-Only ABI

To get the best performance from source-only ABI generation:

1. **Place annotations and constants in their own rules**: All annotations and
   compile-time constants (including enum values) used in the interface of a
   rule must be present during source-only ABI generation. Create small, focused
   rules for these elements with minimal dependencies.

2. **Follow Java naming conventions**: Packages should have names beginning with
   a lowercase letter. Top-level classes should have names beginning with an
   uppercase letter. Buck2 uses these conventions to infer which types might not
   be available during ABI generation.

3. **Reference member types canonically**: When referencing member types (nested
   classes), use the canonical reference to the class where they are defined,
   not a class that inherits them.

4. **Avoid star imports**: Use single-type imports instead of on-demand (star)
   imports. Star imports and `static` imports of types cannot be resolved at
   source-ABI generation time unless the rule containing the type is available.

5. **Minimize class hierarchy depth**: Deep class hierarchies split across
   multiple modules can require additional `source_only_abi_deps`. Keeping
   hierarchies shallow or within single modules improves ABI generation.

## Configuration

### Global Configuration

Set the default ABI generation mode in `.buckconfig`:

```ini
[java]
    abi_generation_mode = source_only
```

Possible values: `class`, `source`, `source_only`

### Per-Rule Configuration

Override the global setting for a specific rule:

```python
java_library(
    name = "mylib",
    srcs = ["MyClass.java"],
    deps = ["//other:dep"],
    abi_generation_mode = "class",  # Override to use class ABI
)
```

## Performance Considerations

The performance benefits of source-only ABI generation increase with:

- **Build graph complexity**: More dependencies = more parallelism opportunities
- **Incremental build frequency**: More frequent small changes = more cache hits
- **Dependency stability**: Stable public interfaces = fewer rebuilds

However, overuse of `source_only_abi_deps` and `required_for_source_only_abi`
can negate these benefits by reducing parallelism. Use them sparingly and keep
affected rules small and focused.
