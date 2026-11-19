# Validation

Validate the exact candidate revision. A passing historical receipt is not a
gate for later source edits. Debug and optimized builds are complementary:
debug assertions catch UI/resource-lifetime errors that optimized tests miss.

## Cargo gates

From the Buck2 workspace:

```sh
cargo build --locked -p retrace
cargo check --locked -p retrace --all-targets
cargo doc --locked -p retrace --no-deps
cargo test --locked -p retrace --lib --test cli_acceptance
cargo test --locked -p retrace --test gui_snapshots
```

Run clippy with the workspace's `lint_levels.bzl` denial/allow lists (the same
lists used by `test.py`), including tests and binaries. Run rustfmt on every
changed Rust file. Fixtures are read from checked-in `testdata/` under Cargo;
missing fixture environment variables must not silently skip assertions.

Retrace intentionally remains in the open-source workspace, including its GUI.
The wgpu/windowing dependency cost is accepted; unused eframe glow/persistence
features are not requested. Remote Meta integrations are compile-time gated;
offline parsing and analyses must run in Cargo without those services.

Pixel comparisons are a separate, explicit renderer-specific gate. Use the
checked-in Linux baselines and inspect every changed image, not an automatic
baseline refresh. The Cargo `gui_snapshots` target always runs its portable
interaction assertions; `RETRACE_PIXEL_SNAPSHOTS=1` additionally enables pixel
comparisons with the pinned Linux software-renderer environment. Running the
target without that opt-in does not validate the images. A Linux pass does not
establish macOS or Windows support;
run the CLI/library gates and interactive reopen/scroll/cascade checks there.

## Performance and boundary checks

Compare preserved baseline and candidate binaries on the same immutable input.
Record artifact sizes/hashes, CPU/worker budget, cache state, wall/user/system
time and peak RSS. Separate conversion, parse/index, graph stitch, report and
GUI-first-view work; do not add overlapping phase savings. Check semantic
outputs before claiming a speedup. Include action-heavy, query-only, no-touch,
partial and legacy inputs, and exercise cancellation during expensive stages.

Each review unit must compile and test with only its predecessors. Shared APIs,
fixtures, documentation and their tests must arrive together. After folding
fixes into owners, rerun boundary checks and the assembled head; preserve
review identities and distinguish tested revisions from later metadata edits.
