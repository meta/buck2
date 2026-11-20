---
id: key_concepts
title: Key Concepts
---

import useBaseUrl from '@docusaurus/useBaseUrl';

# Key concepts

Buck2 has a number of fundamental concepts:

- A [**_build rule_**](build_rule.md) describes how to produce an output file
  from a set of input files. Most build rules are specific to a particular
  language or platform. For example, you would use the
  [`cxx_binary`](../../prelude/rules/cxx/cxx_binary) rule to create a C++
  binary, but you would use the
  [`android_binary`](../../prelude/rules/android/android_binary) rule to create
  an Android APK.
- A [**_build target_**](build_target.md) is a string that uniquely identifies a
  build rule. It can be thought of as a URI for the build rule within the Buck2
  project.
- A [**_build file_**](build_rule.md) defines one or more build rules. In Buck2,
  build files are typically named `BUCK`. A `BUCK` file is analogous to the
  `Makefile` used by the Make utility. In your project, you will usually have a
  separate `BUCK` file for each buildable unit of software—such as a binary or
  library. For large projects, you could have hundreds of `BUCK` files.

### Packages

A Buck2 **_package_** is defined by:

- A Buck2 build file (a `BUCK` file) that marks the root of the package
- All files in the same directory as this `BUCK` file
- All files in subdirectories, _unless_ those subdirectories contain their own
  `BUCK` files

In other words, Buck2 packages are hierarchical and non-overlapping: Each `BUCK`
file creates a new package boundary. A package does not include subdirectories
that contain their own `BUCK` files. Those subdirectories with `BUCK` files
become roots of their own separate packages.

For example, in the following diagram, the BUCK file in directory `app-dir-1`
defines that directory as the root of a package—which is labeled **Package A**
in the diagram. The directory `app-dir-2` is part of Package A because it is a
subdirectory of `app-dir-1`, but does not itself contain a BUCK file. Now,
consider directory `app-dir-3`. Because `app-dir-3` contains a BUCK file it is
the root of a new package (**Package B**). Although `app-dir-3` is a
subdirectory of `app-dir-1`, it is _not_ part of Package A.

<img src={useBaseUrl('/img/packages-1.png')} alt='justifyContent'/>

### Cells

A Buck2 **_cell_** is:

- A directory tree containing one or more Buck2 packages
- Configured by a [**`.buckconfig`**](buckconfig.md) file at **its root**
  ```
  [cells]
  cell_name = path_to_cell
  ...
  ```
- Often (but not necessarily) corresponding to a repository

Note that although the cell root should contain a `.buckconfig`, the presence of
a `.buckconfig` file doesn't in itself define a cell. Rather, _the cells
involved in a build are defined at the time Buck2 is invoked_; they are
specified in the `.buckconfig` for the Buck2 _project_ (see below).

### Projects

A Buck2 **_project_** is:

- The entry point for Buck2 builds
- Defined by the `.buckconfig` file in the directory where Buck2 is invoked (or
  in the nearest ancestor directory),
- The container that specifies which cells are part of the build

**_How cells and projects relate._** The project's `.buckconfig` specifies all
cells in the [cells](buckconfig.md#cells) section. The directory containing the
project's `.buckconfig` is automatically considered a cell. While not required,
it's good practice to explicitly list the project cell in the configuration.

### Buck2's dependency graph

Every build rule can have zero or more dependencies. You can specify these
dependencies using, for example, the `deps` argument to the build rule. For more
information about specifying dependencies, consult the reference page for the
build rule you are using. These dependencies form a directed graph, called the
_target graph_. Buck2 requires the graph to be acyclic. When building the output
of a build rule, all of the rule's transitive dependencies are built first. This
means that the graph is built in a "bottom-up" fashion. A build rule knows only
which rules it depends on, not which rules depend on it. This makes the graph
easier to reason about and enables Buck2 to identify independent subgraphs that
can be built in parallel. It also enables Buck2 to determine the minimal set of
build targets that need to be rebuilt.

### Multiple Buck2 projects in a single repository

Buck2 is designed to build multiple deliverables from a single repository—that
is, a _monorepo_—rather than from multiple repositories. Support for the
monorepo design motivated Buck2's support for cells and projects. It is
Facebook's experience that maintaining all dependencies in the same repository
makes it easier to ensure that all developers have the correct version of the
code and simplifies the process of making atomic commits.

### See also

Take a look at the [Concept Map](concept_map.md) for a visualization of how
Buck2 concepts interact with each other. Also see the [Glossary](glossary.md).
