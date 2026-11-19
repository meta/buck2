# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":rust_toolchain.bzl", "PanicRuntime", "RustExplicitSysrootDeps")

# Information that determines how dependencies should be collected
DepCollectionContext = record(
    advanced_unstable_linking = field(bool),
    include_doc_deps = field(bool),
    # Is the target a proc-macro target? This is ignored if `include_doc_deps`
    # is set, since doc tests in proc macro crates are not built with
    # `--extern proc_macro`
    is_proc_macro = field(bool),
    # From the toolchain, if available
    explicit_sysroot_deps = field(RustExplicitSysrootDeps | None),
    # Only needed if `advanced_unstable_linking` is set
    panic_runtime = field(PanicRuntime),
    # From the toolchain, if available
    linker_deps = field(list[Dependency], []),
)
