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
  -m 'ovr_config//build_mode/constraints:llvm-ir-pgo-profile[buck2-td]' \
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

First export the merged `.profdata` as a Buck target and replace `None` for
`buck2-td` in `profiles.bzl` with that target's label. The historical
`fbcode//buck2/pgo:td_pgo_profile` example is not a checked-in artifact.
Until a real profile is registered, generation works but profile use builds
without one. Do not substitute another product's profile or an empty profile.

After registering the artifact:

```bash
buck2 build @fbcode//mode/opt \
  -m 'ovr_config//build_mode/constraints:llvm-ir-pgo[use]' \
  -m 'ovr_config//build_mode/constraints:llvm-ir-pgo-profile[buck2-td]' \
  fbcode//buck2:buck2
```

Keep the same profile family and other build settings for generation and use.
The family participates in Rust symbol identity, even though generation does
not depend on the profile artifact. Capture a fresh profile with these settings
rather than assuming profiles from the old buckconfig build have matching names.

These are local CLI commands. CI workflows should apply the same constraints
through a transitioned wrapper target; arbitrary CLI modifiers are restricted
on CI.
