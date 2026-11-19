# fuzzy_dice

Randomized property test for `dice`. Generates sequences of `SetValue` /
`ForceDirty` / `ReinjectEquivalent` / `EnqueueStep` / `Query` operations,
executes them against `dice`, and checks two properties against a reference
model:

- **Value correctness** — query results match `MathAnswerKey`'s expected value.
- **Recompute discipline** — the set of vars dice `Evaluated` (as reported via
  `ActivationTracker`) is a subset of what the reference model considers
  permissible to recompute at each query.

The recompute bound is a deliberate upper bound on today's engine (comment
`MathAnswerKey::permit_recompute` for the loosening scenarios). It is
constructed to be sound for the current implementation — long fuzz runs
against today's dice produce zero violations — so any future violation is an
unambiguous regression.

## Running

### Quick run (CI)

`buck2 test` picks up the `#[test]` in `src/main.rs` automatically:

```sh
buck2 test //buck2/dice/fuzzy_dice:fuzzy_dice
```

The CI test is bounded (small case count, fixed size, no shrinking) — it's a
smoke test, not a bug hunt. Failures should be reproduced with the deep run.

### Deep run

```sh
buck2 run //buck2/dice/fuzzy_dice:fuzzy_dice -- fuzz 10000 10000
```

Positional args are `max_tests` and `num_tests` (defaults `2_000_000`).

Cases run in-process. This package is built with `panic=unwind` (see
`PACKAGE`), so a dice panic does not take the fuzzer down: a panic hook
records it, the query in flight is abandoned, and the case is reported and
shrunk like any other failure. A query that neither finishes nor panics
within 30s is reported as hung.

Known-buggy generator scenarios are still gated by env vars:

```sh
NOGEN_TRANSIENTS=1 NOGEN_OUT_OF_ORDER=1 \
    buck2 run //buck2/dice/fuzzy_dice:fuzzy_dice -- fuzz 10000 10000
```

When a failure is found the input `DiceExecutionOrder` is written to
`/tmp/fuzzy_dice_cases/<uuid>/input.json`, and the smallest reproduction shrinking
finds beside it as `shrunk.json`; either path is a replay artifact.

Useful flags:

- `--no-shrink` — return the raw failing case rather than running the naive
  shrinker (which re-runs each candidate 100 times).
- `--size N` — pass `N` to `Gen::new`. Default is 10 (small cases).
  quickcheck at the version we're pinned to doesn't accept an explicit RNG
  seed, so this is the only reproducibility knob.

### Replay

```sh
buck2 run //buck2/dice/fuzzy_dice:fuzzy_dice -- replay /path/to/input.json
```

## Operation vocabulary

- `SetValue { new_ctx_id, var, expr }` — commit a new equation for `var`.
- `Query { ctx_id, var }` — evaluate `var` at the stored transaction for
  `ctx_id`. Value and activation events are checked against the reference
  model.
- `EnqueueStep(var, steps)` — push `ComputationStep`s into the per-var queue
  read by `EvalVar::compute` (currently only `ReturnTransient`, gated by
  `NOGEN_TRANSIENTS`).
- `ForceDirty { new_ctx_id, var }` — `changed(vec![EvalVar { key: var, ..}])`.
  Exercises dice's `InvalidateKind::ForceDirty` on the *computed* key;
  equations are untouched, so `EvalVar(var)` must recompute on next touch and
  dependents cutoff on value equality. Analogous to buck2's file-watcher
  invalidation of a computed key whose `compute` reads the environment.
- `ReinjectEquivalent { new_ctx_id, var }` — `changed_to(LookupVar(var), e)`
  where `e` evaluates to the same boolean as the current expr but compares
  unequal as an `Arc<Expr>`. Exercises the injected-value path
  (`InvalidateKind::Update`) and dice's downstream value-equality cutoff.

## Known scope gaps

- Transients (`ComputationStep::ReturnTransient`) and out-of-order queries
  remain gated by the pre-existing `NOGEN_TRANSIENTS` / `NOGEN_OUT_OF_ORDER`
  env vars.
- Out-of-order queries of a var that is not yet set at the queried ctx are
  skipped rather than checked. Dice panics inside a task for such a query,
  and a panicking task leaves its dependents waiting forever, which would
  poison the rest of the run. Detecting the known bug where such a query
  instead returns a stale value therefore stays out of scope.
- Leaf-injected keys, projections, concurrent queries, and delta-debugging in
  the shrinker are out of scope for this iteration.
