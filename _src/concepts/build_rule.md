---
id: build_rule
title: Build Rule
---

# Build Rule

A _build rule_ is a procedure for producing output files from a set of input
files in the context of a specified build configuration. Build rules are
specified in [build files](build_file.md)—typically named BUCK.

> **Note:** A build rule must explicitly specify, in its arguments, all of its
> required inputs in order for Buck2 to be able to build the rule's output in a
> way that is deterministic and reproducible.

## Buck2's collection of build rules

Buck2 comes with a collection of built-in build rules for many common build
procedures. For example:

- Compiling Java code against the Android SDK is a common procedure, so Buck2
  provides the [`android_library`](../../prelude/rules/android/android_library)
  build rule
- The final product of most Android development is an APK, so you can use the
  [`android_binary`](../../prelude/rules/android/android_binary) build rule to
  create an APK

## Source files as inputs to build rules

Most build rules specify source files as inputs. For example, a
[`cxx_library`](../../prelude/rules/cxx/cxx_library) rule would specify `.cpp`
files as inputs.

To support specifying these files:

- A `cxx_library` rule provides the `srcs` argument for source files
- For header files (such as in C++), `cxx_library` provides a `headers` argument
- Some rules provide platform-specific variants like `platform_srcs` and
  `platform_headers`, which support groups of source files that should be used
  as inputs only when building for specific platforms

### Package Boundaries and Source File Access

In Buck2, source files are organized within _packages_:

- A **package** is defined by a BUCK file and includes:
  - The directory containing that BUCK file
  - All subdirectories that don't themselves contain BUCK files
- For more details on packages, see the [Key Concepts](key_concepts.md) topic

#### Package Access Rules

Buck2 enforces these rules regarding source file access:

1. **Basic Rule**: A build rule can only use source files from its own package
   - A rule in a BUCK file cannot specify source files from outside its package

2. **Header File Exception**: A rule can access header files from another
   package if:
   - The other package explicitly _exports_ those header files
   - This is done using the `exported_headers` argument
   - See the [`cxx_library`](../../prelude/rules/cxx/cxx_library) documentation
     for details

3. **Accessing Functionality**: To use code from other packages:
   - Use the artifacts produced by build rules in those packages
   - Specify those build rules as _dependencies_
   - Example: A [`cxx_binary`](../../prelude/rules/cxx/cxx_binary) can use a
     `cxx_library` from another package by taking it as a dependency

#### Using Symlinks (Not Recommended)

Symlinks are file system shortcuts that point to other files or directories (for
example, a symlink in `/project/src` might point to a file in
`/shared/common/lib`).

We recommend **against** using symlinks (absolute or relative) to specify input
files to build rules. While symlinks sometimes work in this context, they often
lead to unexpected behavior and errors.

## Dependencies: Output from One Rule as Input to Another Rule

A build rule can use the output from another build rule as one of its inputs by
specifying that rule as a _dependency_.

### How to specify dependencies

Dependencies are specified in several ways:

- **Primary Method**: Most build rules use the `deps` argument to list
  [build target](build_target.md)s they depend on
- **Other Arguments**: Some rules allow dependencies to be specified in other
  arguments:
  - In `srcs`: When a build target (rather than a file path) is listed in `srcs`
  - In rule-specific arguments: Some rules have special dependency arguments
    (like `runtime_deps`)

### Examples:

The output of a [`java_library`](../../prelude/rules/java/java_library) rule is
a JAR file. If a `java_library` rule specifies another `java_library` rule as a
dependency, the JAR file produced by the specified rule is added to the
classpath for the `java_library` that depends on it.

If a [`java_binary`](../../prelude/rules/java/java_binary) rule specifies a
`java_library` rule as a dependency, the JAR file for the specified
`java_library` is available on the classpath for the `java_binary`.

- In addition, the JAR files for any dependencies of the `java_library` rule
  _are also_ made available to the `java_binary` rule
- If those dependencies have dependencies of their own, they are added as well

This exhaustive cascade of dependencies is referred to as the rule's _transitive
closure_.

### Required dependencies are always built first

Buck2 guarantees that any dependencies that a rule lists that are required to
build that rule are built successfully _before_ Buck2 builds the rule itself.

Note that there can be special cases—such as
[`apple_bundle`](../../prelude/rules/apple/apple_bundle)—where a rule's listed
dependencies do not actually need to be built before the rule.

### Visibility

In order for a build rule to take a dependency on another build rule, the build
rule on which the dependency is taken must be _visible_ to the build rule taking
the dependency.

A build rule's `visibility` argument is a list of
[build target pattern](target_pattern.md)s that specify the rules that can take
that rule as a dependency. For more information about the concept of visibility
in Buck2, see the [Visibility](visibility.md) topic.

### Dependencies define a graph

Build rules and their dependencies define a directed acyclic graph (DAG). Buck2
requires this graph to be acyclic to make it possible to build independent
subgraphs in parallel.

## How to handle special cases: genrules and macros

Although Buck2 provides a rich set of built-in build rules for developers, it is
not able to address all possible needs. As an "escape hatch," Buck2 provides a
category of generic build rules called _genrules_.

With genrules, you can perform arbitrary operations using shell scripts. The
genrules supported by Buck2 are:

- [`genrule`](../../prelude/rules/core/genrule)
- [`apk_genrule`](../../prelude/rules/android/apk_genrule)
- [`cxx_genrule`](../../prelude/rules/cxx/cxx_genrule)
- [`jar_genrule`](../../prelude/rules/java/jar_genrule)
- [`js_bundle_genrule`](../../prelude/rules/js/js_bundle_genrule)

### Multiple output files with genrules

In most cases, a build rule produces exactly one output file. However, with
genrules, you can specify an output _directory_ and write arbitrary files to
that directory.

### Macros

You can define functions that generate build rules. In general, this should not
be something that you need to do, but taking advantage of this option might help
you add needed functionality to Buck2 without editing its source code.

## String parameter macros

It is also possible to expand references to other rules within the `cmd`, using
the builtin
[`string parameter macros`](../../rule_authors/string_parameter_macros). All
build rules expanded in the command are automatically considered to be
dependencies of the `genrule()`.
