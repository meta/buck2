# Bazel Build Event Service TODO

This tracker records the remaining Buck2-to-Bazel Build Event Protocol
conversion work. The list is intentionally mutable: update it as code findings
make goals clearer, obsolete, or more urgent.

## P0/P1: BuildBuddy reuse blockers

- [x] Emit target artifacts through `TargetComplete.output_group`.
  - [x] Add named sets for target outputs.
  - [x] Reference the named sets from the target completion event.
  - [x] Prefer URI-backed files when artifact upload is configured.
  - [x] Upload local file outputs through the configured artifact upload backend
        when Buck2 can resolve them from the workspace.
  - [x] Include directory/tree outputs in the default output group and
        `TargetComplete.directory_output`.
  - [x] Stream oversized local files through the configured artifact upload
        backend instead of synthesizing URIs for bytes that were not uploaded.
- [x] Make action and test logs render in BuildBuddy.
  - [x] Keep inline file contents as a fallback for debugging.
  - [x] Add an optional bytestream upload path for BEP `File.uri`.
  - [x] Use URI-backed `stdout`, `stderr`, `test.log`, and `test.xml` when
        upload is enabled.
- [x] Emit BuildBuddy-usable command metadata.
  - [x] Emit canonical `StructuredCommandLine` with option-list sections.
  - [x] Emit `OptionsParsed`.
  - [x] Populate workspace status and build metadata with repo, user, host,
        command, target pattern, remote cache, remote executor, and CI values
        when Buck2 knows them.
- [x] Model tests as Bazel test events.
  - [x] Aggregate Buck2 testcase events into one `TestResult` per target run.
  - [x] Emit a matching `TestSummary` with run/shard/attempt counts.
  - [x] Synthesize a minimal `test.xml` from Buck2 testcase details.
- [x] Emit invocation-level metrics, configuration, and tool-log events.
  - [x] Emit `BuildMetrics` from `InvocationRecord` and aggregated Buck2
        metrics.
  - [x] Emit `Configuration` and `WorkspaceInfo` events when known.
  - [x] Emit `BuildToolLogs` with a compact Buck2 invocation summary when file
        upload is enabled.

## P2: Semantic parity

- [x] Populate `PatternExpanded.children` with configured targets when the
      resolved target list is available.
- [x] Clean up target lifecycle semantics.
  - [x] Avoid final target completion on analysis-only events when later build
        or test completion exists.
  - [x] Deduplicate configured and completed events for each target/configuration.
- [x] Add structured failures.
  - [x] Populate `failure_detail` on failed actions, targets, and
        invocations.
  - [x] Confirm latest Bazel BEP still has no direct `failure_detail` field
        for test result or summary payloads; keep test details in
        `TestResult.status_details` and generated `test.xml`.
  - [x] Emit `Aborted` for skipped, cancelled, or failed announced children that
        do not receive a normal terminal event.
- [x] Improve remote execution diagnostics.
  - [x] Populate `ActionExecuted.strategy_details` with Buck2 execution
        strategy, action digest, cache-hit type, queue time, remote session,
        and use-case details when they are available.
- [x] Improve action-cache statistics.
  - [x] Populate `BuildMetrics.ActionSummary.action_cache_statistics` with
        conservative hit and miss counters derived from Buck2 invocation
        action counters.

## P3: Remaining Bazel BEP parity

- [x] Emit `Fetch` events for external downloads.
  - [x] Emit HTTP `Fetch` events for materialized `download_file` artifacts.
  - [x] Emit `Fetch` events for git-backed external cell population.
  - [x] Confirm the remaining materialization methods do not currently expose a
        stable external resource URL: CAS downloads name CAS data, local copies
        name local artifacts, and writes are generated content.
- [x] Emit `TestProgress` for active test attempts when Buck2 exposes a live
      progress resource URI.
  - [x] Emit progress for remote test attempts when RE reports live log stream
        resource names.
  - [x] Confirm local and non-streamed test payloads currently carry final
        inline stdout/stderr and generated XML, but no stable live log URI.
- [x] Emit `TargetSummary`.
- [x] Emit `ConvenienceSymlinksIdentified`.
- [x] Emit `ExecRequestConstructed` for `buck2 run` once the client has
      constructed the post-build argv.

## BuildBuddy follow-up issues

- [ ] Render inline BEP `File.contents` as a fallback when no URI is present.
- [ ] Recover command metadata from `OptionsParsed`, `BuildMetadata`, or the
      original command line when canonical option-list chunks are missing.
- [ ] Match `ActionExecuted` events to targets by label/configuration when the
      BEP stream lacks target graph children.
- [ ] Refresh target detail pages while in-progress BES indexing catches up.

## Validation

- Latest Bazel reference checked with `$ask-bazel`: `1a04a7b58cb55b80f4968fe5485fab55dca8f608`
  (`Use released bazel_ci_rules 2.0.0.`) in
  `/home/nb/work/bazelbuild/bazel-codex-LPet`.
- [x] `cargo build --bin buck2`
- [x] `./bootstrap/buck2 build //:buck2`
- [x] Run a local BuildBuddy comparison between a real Bazel invocation and a
      Buck2 invocation using `event_format = "bazel"`.
  - Buck2 build:
    `http://localhost:8080/invocation/da913e3c-01ac-4f3f-8429-1b81568c322c`
  - Bazel build:
    `http://localhost:8080/invocation/c5a26a43-fc30-4d0c-a826-8df6c9768324`
  - Buck2 remote-execution validation:
    `http://localhost:8080/invocation/d69928c7-2e6f-4567-97e0-1987302957fb`
  - Buck2 test validation:
    `http://localhost:8080/invocation/9eff2b96-cc2e-4c23-b025-be136bed2493`
- [x] Capture overview, target detail, action log, and test detail screenshots.
  - Overview: `output/playwright/bes-compare/buck2-overview.png`
  - Target detail: `output/playwright/bes-compare/buck2-target-detail.png`
  - Action execution list:
    `output/playwright/bes-compare/buck2-action-execution-list.png`
  - Action detail: `output/playwright/bes-compare/buck2-action-detail.png`
  - Test detail: `output/playwright/bes-compare/buck2-test-detail.png`
