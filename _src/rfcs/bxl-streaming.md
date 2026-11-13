# [RFC][BXL] Streamed Output in BXL

## 1. Overview and Context

BXL scripts can primarily be divided into **two phases**:

1.  **Phase 1 (Evaluation)**: The BXL script is interpreted. During this phase,
    calls to `ctx.output.ensure(...)` register artifacts that should be
    materialized later, but the actual materialization does not happen yet.
2.  **Phase 2 (Materialization)**: It materializes all artifacts previously
    registered via `ctx.output.ensure(...)`.

Under the current model, all `ctx.output.print(...)` produce their output only
at the end of Phase 2 (i.e., after all artifacts have been materialized). This
make bxl users cannot get the output from bxl as soon as it is ready.

In some use cases, streaming output is desired:

- Users might want partial output or intermediate query results right away,
  rather than waiting for the entire BXL script to finish.
- Users might want an immediate path of an artifact as soon as it is
  materialized, to trigger a follow-up action in a more interactive or
  incremental workflow.

## 2. Goal

We aim to introduce an option that allows streaming output as soon as relevant
artifacts or other results like query results are ready. Specifically, allow
users to output the information that not depends on artifacts immediately in
Phase 1, and stream the output as relevant artifacts are available in Phase 2.

## 3. Proposed API

We introduce new apis `ctx.output.stream/stream_json` that would have
“streaming” behavior:

```
def ctx.output.stream(
    *args,
    sep: str = " ",
    additional_waits: List[bxl.EnsuredArtifact] = []
) -> None:
    """
    Print arguments either immediately (phase 1) or in a streaming manner (phase 2).
    - If no `EnsuredArtifact` appears in args, it is output immediately during phase 1 evaluation.
    - If there are EnsuredArtifact(s) (either in `args`
      or in `additional_waits`), we defer printing until after these artifact(s)
      have been materialized in phase 2. When all required artifacts
      are materialized, the relevant string is printed immediately.
    """
```

**Case 1: Streaming Behavior When No Artifacts Are Involved**:

- `ctx.output.stream("Hello World")`
- Evaluated during Phase 1. The message “Hello World” is emitted immediately

**Case 2: Streaming Behavior When Artifacts Are Involved**:

- `args` have EnsuredArtifacts or `additional_waits` is not empty.
- The print is deferred until those artifacts have all been materalized.

Thus, users can write:

```
ensured0 = ctx.output.ensure(my_artifact0)
ensured1 = ctx.output.ensure(my_artifact1)

# Wait for ens_art to be ready, then print
ctx.output.stream("Artifact path:", ensured0, additional_waits=[ensured1])

```

## 4. Implementation Sketch

- Phase 1 (BXL Evaluation)
  - When we have **Case 1**, we will output the string to stdout by
    `PartialResultDispatcher` and save the output in the cache file associated
    with the bxl key.
  - If we have **Case 2**, we will store the output string and related waits on
    artifacts in the result of bxl key.

- Phase 2 (Materialization)
  - As artifacts materialize, mark them “ready.”
  - After each artifact’s materialization, check which print requests are now
    satisfied (i.e., all required artifacts are “ready”). Those requests are
    immediately printed to stdout.
- If bxl key is get from cache which means we skip the `Phase 1` and no output
  from that phase, get the streaming output cache and print to stdout.

## 5. Alternative Solutions

**Alternative 1: Callback-Based** Api would like this

```
def OutputStream.print(
    *args,
    sep: str = " ",
    additional_waits: List[EnsuredArtifact] = [],
    callback: Optional[Callable[[EnsuredArtifact], None]] = None
) -> None
```

or even attaching a callback directly on `ctx.output.ensure(...)`

```
def OutputStream.ensure(
    artifact: Artifact,
    callback: Optional[Callable[[EnsuredArtifact], None]] = None
) -> EnsuredArtifact:
```

_Advantages:_

- Users can programmatically handle “when artifact is ready, do X” in BXL
  scripts, leading to more flexibility.
- More intuitive to users

_Downsides:_

- When Phase 1 finishes, we do not have BXL runtime in Phase 1. In this case, we
  need to provide a bxl runtime and provide a **limited** context to execute
  callbacks
- More complex to implement and reason about.
- “print once artifacts are ready” use case is already covered by the proposed
  solution.

If user cannot implement what they want to do using the proposed solution, we
can provide this callback later on without introducing any breaking changes.

**Alternative 2: CLI Flag for Streaming**

Instead of use `ctx.output.stream` at the call site, a global CLI flag
`--streaming` could exist. That would make all `ctx.output.print` “streaming”.
However:

- It lacks fine-grained control. Users cannot selectively choose which prints
  are delayed/streamed.
- The proposed solution offers more flexibility by controlling streaming at each
  print call.

## 6. Potential Future Work

1. Prioritized Materialization:
   - A flag or mechanism to ensure certain artifacts are materialized first
     (blocking the remaining artifacts), so streaming prints that depend on them
     can happen sooner.

2. Streaming Build Actions:

- Stream outputs of each build actions in `ctx.build`. With Structured Action
  Errors IDEs can easily parsed the compile errors

3. Streaming Output for Other Lazy APIs:
   - Extend similar streaming logic to apis like
     `ctx.lazy.unconfigured_target_node(patterns like cell//path/to/...)`,
     queries or other lazy operations, so partial results appear earlier in
     logs.

4. Instead of printing only to stdout, support more flexible transport layers
   (e.g. socket file) or structured encodings for streaming data.
