# Buck2 LLVM IR PGO

This replaces the `rust.pgo_generate_dir` and `rust.pgo_profile` buckconfigs
from D101392776. PGO is off by default. The shared Rust toolchain instruments
the configured Rust dependency graph, excluding bootstrap/sysroot crates and
dependencies transitioned to execution platforms, and the fbcode clang
toolchain does the same for Buck2's C/C++ dependencies.

## Generate

Build the instrumented binary:

```bash
buck2 build @fbcode//mode/opt \
  -m 'ovr_config//build_mode/constraints:llvm-ir-pgo[generate]' \
  --show-full-output \
  fbcode//buck2:buck2
```

Run the resulting binary with `LLVM_PROFILE_FILE` set before starting its
daemon, for example `LLVM_PROFILE_FILE='/tmp/pgo/buck2-%m-%p.profraw'`.
Setting this variable on the build command alone does not configure the
instrumented binary's capture path. Restart an existing daemon before changing
the destination; later clients do not reconfigure its profile runtime.

Exercise the intended training workload with that binary, then invoke its
`debug flush-pgo-profile` command to flush the existing daemon's counters.
Do not rely on daemon shutdown: some exit paths bypass LLVM's exit handler.
Merge the captured `.profraw` files using an `llvm-profdata` version compatible
with the compiler used for capture.

## Use

`//buck2/app/buck2:buck2-bin` takes its profile from `//buck2/facebook/pgo:pgo`.
Check in the merged `.profdata`, set it as that target's `profile`, and list
the target in `LLVM_IR_PGO_TARGETS` in `fbcode/buck2/platform/pgo/registry.bzl`.
Until then, generation works but profile use fails explicitly. Do not
substitute another product's profile or an empty profile.

After registering the profile:

```bash
buck2 build @fbcode//mode/opt \
  -m 'ovr_config//build_mode/constraints:llvm-ir-pgo[use]' \
  fbcode//buck2:buck2
```

Keep the other build settings the same for generation and use: a profile only
matches the Rust symbol names of the configuration it was captured in.

These are local CLI commands. CI workflows should apply the same constraints
through a transitioned wrapper target; arbitrary CLI modifiers are restricted
on CI.
