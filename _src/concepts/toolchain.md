---
id: toolchain
title: Toolchain
---

A toolchain defines a set of tools, scripts and flags used by certain rules.
Their purpose is to enable reuse of rules across projects that source their
tools (e.g. compilers and linters) differently.

For example, consider `cxx_binary`, which is defined in the prelude. Since
building C++ code is complex, it is desirable to share the same implementation
of `cxx_binary` across projects. However, not all projects will want to source
their C++ compiler, linker, etc. the same way:

- Some projects do not care, and want to pick them from the ambient environment
  (most likely the tools installed system-wide).
- Some projects want to achieve reproducible builds by running the build within
  some sort of virtual environment.
- Some projects want to achieve reproducible builds by downloading tools as part
  of the build itself.
- Some projects want to achieve reproducible builds by accessing tools checked
  into version control.

Defining those in a toolchain lets us decouple those project-specific concerns
from generic build rules.

When running `buck2 init`, Buck2 sets up some demo toolchains via the
`system_demo_toolchains` macro. Those expect to find the relevant tools in the
user's `PATH`.

For more information about defining toolchains, see the
[relevant page in the Rule Authors section](../rule_authors/writing_toolchains.md).
