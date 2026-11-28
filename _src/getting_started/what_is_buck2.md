---
id: what_is_buck2
title: What is Buck2?
---

Welcome to Buck2! If you're new here, this page will give you a brief overview
of what Buck2 is and the key reasons you might consider using it.

## What is Buck2?

Buck2 is a build system developed and used by Meta. Think of a build system as a
tool that automates the process of creating executable binary from source code.
This includes compiling code, linking libraries, and packaging everything
together.

Here are a few key things to know about Buck2:

- **Supports Many Languages**: Buck2 can build projects written in a variety of
  programming languages, including C++, Rust, Python, Go, OCaml, Erlang, and
  more.
- **Designed for Large Monorepos**
- **Open Source**: You can find its source code and contribute at
  [https://github.com/facebook/buck2](https://github.com/facebook/buck2).
- **Fast**: 2x as fast as Buck1 🚀
- **Correctness**: Buck enforces hermeticity to ensure builds are correct by
  default.
- **Extensible**: Allows developers to easily extend and customize their build
  process. Buck2 runs on various operating systems including Windows, Linux, and
  macOS, and can build for these platforms as well as for Android, iOS, and
  others.

## Why Use Buck2? Key Advantages

- **Performance:**
  - **Faster Parallel Builds:** Buck2 is architected to build different parts of
    your project simultaneously (in parallel) whenever possible, significantly
    speeding up the overall build process.
  - **Low Incremental Build Time:** After you make a small change to your code,
    Buck2 is very efficient at only rebuilding what's necessary. This leads to
    faster iterations when you're developing and testing.

- **Determinism and Reproducibility:**
  - **Hermetic Builds:** Buck2 aims for "hermetic" builds. This means that
    builds are self-contained and don't depend on external factors or
    pre-installed tools on your machine that aren't explicitly declared. This
    ensures that if you build the same code, you get the same result, every
    time, regardless of where or when it's built.

- **Transparency:**
  - **Dependency Comprehension:** Buck2 has a clear way of defining and
    understanding the dependencies between different parts of your code. You can
    use queries to explore and understand these relationships, which is
    invaluable in large projects.

- **Correctness at Scale:** By ensuring builds are reproducible and dependencies
  are explicit, Buck2 helps maintain correctness even as your codebase grows and
  becomes more complex.

- **Language Extensibility:** Buck2's rule system is designed to be extensible,
  allowing support for new languages and tools to be added.

- **Remote Execution and Caching:** Buck2 supports distributing build actions
  across multiple machines (remote execution) and caching build results. This
  can dramatically speed up builds, especially for lage teams and projects, as
  work done by one developer can benefit others.

In essence, Buck2 is designed to make the build process faster, more reliable,
and more understandable, especially for large and complex software projects. For
a more in-depth look at these advantages, you can visit
[Why Use Buck2?](../../about/why/).
