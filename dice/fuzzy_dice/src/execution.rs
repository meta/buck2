/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::cmp;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fs;
use std::fs::File;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossbeam::queue::SegQueue;
use dice::ActivationData;
use dice::ActivationTracker;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceTransaction;
use dice::DynKey;
use dice::UserComputationData;
use dice::introspection::serialize_dense_graph;
use dupe::Dupe;
use parking_lot::Mutex;
use quickcheck::Arbitrary;
use quickcheck::Gen;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use uuid::Uuid;

use crate::computation::ComputationStep;
use crate::computation::EvalVar;
use crate::computation::Expr;
use crate::computation::FuzzEquations;
use crate::computation::FuzzMath;
use crate::computation::FuzzState;
use crate::computation::Unit;
use crate::computation::Var;

/// For a given DiceExecutionOrder that includes some Queries, computes the
/// expected result of each of those queries and the set of vars dice is
/// permitted to recompute during each query.
///
/// The value model records `None` for a query whose evaluation is not
/// well-defined (e.g. depends on a var that has never been set); today's
/// generator does not produce such queries, but the case is preserved
/// defensively.
struct MathAnswerKey {
    values_by_query_index: HashMap<usize, Option<bool>>,
    /// For each query, the upper bound on which vars dice may recompute
    /// (`Evaluated` in `ActivationTracker`) while servicing the query.
    ///
    /// This is intentionally loose relative to what a maximally-precise engine
    /// would do; violations of this bound are unconditional bugs in today's
    /// dice.
    permitted_recomputes_by_query_index: HashMap<usize, HashSet<Var>>,
    /// The Expr to actually inject when executing each `ReinjectEquivalent`
    /// op. See `Operation::ReinjectEquivalent` for the choice of shape.
    reinject_replacements: HashMap<usize, Expr>,
}

/// State maintained while walking the op sequence to build an answer key.
struct AnswerKeyBuilder {
    /// Current equation for each var.
    equations: HashMap<Var, Expr>,
    /// Snapshot of `equations` at the moment each ctx was created. Queries at
    /// ctx `C` see the state captured here, not the cumulative walk state —
    /// out-of-order queries against older ctxs must not observe later
    /// `SetValue`s.
    equations_at_ctx: HashMap<usize, HashMap<Var, Expr>>,
    /// The highest ctx at which each var has been evaluated (i.e. was in the
    /// subgraph of some query). Out-of-order queries at older ctxs do not
    /// update this — dice caches per-version, so an older-ctx eval doesn't
    /// invalidate the newer cache entry.
    last_eval_ctx: HashMap<Var, usize>,
    /// Every var that has ever been in `var`'s subgraph at any evaluated ctx.
    /// Used to decide whether a dirty event on some other var could
    /// legitimately have made `var` recompute.
    ever_subgraph: HashMap<Var, HashSet<Var>>,
    /// Chronological log of `(ctx_id, var)` for each SetValue and ForceDirty.
    dirty_events: Vec<(usize, Var)>,
    /// Chronological log of `(ctx_id, var)` for `Operation::ForceDirty` only. A key
    /// holds one certificate, stamped with the revision of its untracked input at the
    /// time it was last written; a query at a ctx before a later `ForceDirty` of the var
    /// (or of a var in its ever-subgraph) therefore finds a certificate it cannot
    /// revalidate and recomputes, however unchanged the deps are. `permit_recompute`
    /// allows exactly that shape.
    force_dirty_events: Vec<(usize, Var)>,
    /// Once any transient step is enqueued, every subsequent query permits
    /// arbitrary recomputation. Transients skip caching, so the ripple effects
    /// are hard to bound tightly and today's engine already exhibits enough
    /// slop here that the fuzzer explicitly gates them out.
    transients_seen: bool,
    /// The newest ctx so far: the head of dice's one branch.
    head_ctx: usize,
    /// Evaluations below the head whose write may have attached at the head instead; see
    /// `permit_recompute`.
    displaced: Vec<DisplacedEval>,
}

/// A query at a ctx below the head that evaluated `var` under premises equal to the head's but
/// different at some ctx in between. Dice keeps one claim per key, and the write's window is the
/// one containing the head, so whatever claim served the versions in between is gone.
struct DisplacedEval {
    var: Var,
    head: usize,
}

impl MathAnswerKey {
    pub fn new(order: &DiceExecutionOrder) -> Self {
        let mut state = AnswerKeyBuilder {
            equations: HashMap::new(),
            equations_at_ctx: HashMap::new(),
            last_eval_ctx: HashMap::new(),
            ever_subgraph: HashMap::new(),
            dirty_events: Vec::new(),
            force_dirty_events: Vec::new(),
            transients_seen: false,
            head_ctx: 0,
            displaced: Vec::new(),
        };
        let mut values_by_query_index = HashMap::new();
        let mut permitted_recomputes_by_query_index = HashMap::new();
        let mut reinject_replacements = HashMap::new();
        for (idx, op) in order
            .init_vars
            .iter()
            .chain(order.timeline.iter())
            .enumerate()
        {
            match op {
                Operation::SetValue {
                    new_ctx_id,
                    var,
                    expr,
                } => {
                    state.equations.insert(*var, expr.clone());
                    state.dirty_events.push((*new_ctx_id, *var));
                    state
                        .equations_at_ctx
                        .insert(*new_ctx_id, state.equations.clone());
                    state.head_ctx = state.head_ctx.max(*new_ctx_id);
                }
                Operation::ForceDirty { new_ctx_id, var } => {
                    // Equations unchanged: dice's `changed(EvalVar)` doesn't
                    // touch the value store, only marks that particular
                    // computed key stale so its next touch re-runs `compute`
                    // (which then goes through the usual dep-based cutoff).
                    state.dirty_events.push((*new_ctx_id, *var));
                    state.force_dirty_events.push((*new_ctx_id, *var));
                    state
                        .equations_at_ctx
                        .insert(*new_ctx_id, state.equations.clone());
                    state.head_ctx = state.head_ctx.max(*new_ctx_id);
                }
                Operation::ReinjectEquivalent { new_ctx_id, var } => {
                    // Re-inject an Expr whose `Arc<Expr>` compares unequal to
                    // the current one but evaluates to the same boolean. We
                    // compact to `Unit(Literal(current_value))` — this
                    // preserves the query answer, guarantees inequality
                    // (unless the current expr already happens to be that
                    // literal — see the fall-through below), and keeps the
                    // subgraph analysis simple since the new Expr introduces
                    // no dependencies. The var keeps that literal for the rest
                    // of the case, so its former dependencies drop out of the
                    // graph from here on.
                    let current_val = Self::eval(*var, &state.equations);
                    let replacement = match current_val {
                        Some(v) => {
                            let candidate = Expr::Unit(Unit::Literal(v));
                            if state.equations.get(var) == Some(&candidate) {
                                // Already a literal of this value — a single-
                                // element Xor of the same literal is
                                // structurally distinct while evaluating the
                                // same way. dice's Arc<Expr> equality is by
                                // content, so this makes the injection a real
                                // change.
                                Expr::Xor(vec![Unit::Literal(v)])
                            } else {
                                candidate
                            }
                        }
                        // Undefined value: fall back to the current expr
                        // (dice will treat as no-op). The generator does not
                        // produce this case today.
                        None => state
                            .equations
                            .get(var)
                            .cloned()
                            .unwrap_or(Expr::Unit(Unit::Literal(false))),
                    };
                    state.equations.insert(*var, replacement.clone());
                    reinject_replacements.insert(idx, replacement);
                    state.dirty_events.push((*new_ctx_id, *var));
                    state
                        .equations_at_ctx
                        .insert(*new_ctx_id, state.equations.clone());
                    state.head_ctx = state.head_ctx.max(*new_ctx_id);
                }
                Operation::EnqueueStep(_, steps) => {
                    if steps
                        .iter()
                        .any(|s| matches!(s, ComputationStep::ReturnTransient))
                    {
                        state.transients_seen = true;
                    }
                }
                Operation::Query { ctx_id, var } => {
                    // Everything about this query — value, subgraph dice
                    // touches, ever-subgraph updates, and last-eval bookkeeping
                    // — is computed against the equations frozen at ctx
                    // creation. Cumulative walk state would over-approximate
                    // the subgraph for out-of-order queries and wrongly claim
                    // ancestors have been evaluated when dice actually
                    // touched a strictly smaller graph at the queried
                    // version.
                    // `execute_once` skips a query whose ctx was never created (its
                    // creating op was shrunk away); dice touches nothing then, so
                    // neither does the model.
                    let Some(ctx_equations) = state.equations_at_ctx.get(ctx_id) else {
                        continue;
                    };
                    let value = Self::eval(*var, ctx_equations);
                    values_by_query_index.insert(idx, value);
                    // Likewise for a query whose value the model cannot define.
                    if value.is_none() {
                        continue;
                    }
                    // Permitted set considers every known var, not just vars
                    // in `var`'s subgraph at the queried ctx. Dice re-checks
                    // the deps recorded at each key's last evaluation; those
                    // recorded deps may reference vars that are no longer in
                    // the current subgraph (Cond branch changes, equation
                    // rewrites), so restricting to the subgraph produces
                    // false positives.
                    let permitted: HashSet<Var> = state
                        .equations
                        .keys()
                        .copied()
                        .filter(|w| Self::permit_recompute(*w, *ctx_id, &state))
                        .collect();
                    // Everything in `var`'s subgraph at the queried ctx is
                    // what dice will actually touch, so those vars gain an
                    // ever-subgraph entry and (for in-order queries) advance
                    // last-eval.
                    let subgraph = Self::subgraph(*var, ctx_equations);
                    for w in &subgraph {
                        if *ctx_id < state.head_ctx && Self::reattaches_at_head(*w, *ctx_id, &state)
                        {
                            state.displaced.push(DisplacedEval {
                                var: *w,
                                head: state.head_ctx,
                            });
                        }
                    }
                    for w in &subgraph {
                        state
                            .ever_subgraph
                            .entry(*w)
                            .or_default()
                            .extend(Self::subgraph(*w, ctx_equations));
                        // Record last-eval only for in-order queries; older
                        // ctxs use their own cache entry and don't advance
                        // this timeline.
                        let prev = state.last_eval_ctx.get(w).copied().unwrap_or(0);
                        if *ctx_id >= prev {
                            state.last_eval_ctx.insert(*w, *ctx_id);
                        }
                    }
                    permitted_recomputes_by_query_index.insert(idx, permitted);
                }
            }
        }
        Self {
            values_by_query_index,
            permitted_recomputes_by_query_index,
            reinject_replacements,
        }
    }

    fn reinject_replacement(&self, op_index: usize) -> Option<&Expr> {
        self.reinject_replacements.get(&op_index)
    }

    fn value_of_query(&self, query_index: usize) -> Option<bool> {
        self.values_by_query_index
            .get(&query_index)
            .copied()
            .unwrap_or(None)
    }

    fn permitted_for_query(&self, query_index: usize) -> Option<&HashSet<Var>> {
        self.permitted_recomputes_by_query_index.get(&query_index)
    }

    fn resolve_unit(unit: &Unit, exprs: &HashMap<Var, Expr>) -> Option<bool> {
        match unit {
            Unit::Variable(var) => Self::eval(*var, exprs),
            Unit::Literal(lit) => Some(*lit),
        }
    }

    fn eval(var: Var, exprs: &HashMap<Var, Expr>) -> Option<bool> {
        exprs.get(&var).and_then(|expr| match expr {
            Expr::Unit(unit) => Self::resolve_unit(unit, exprs),
            Expr::Cond {
                test,
                then,
                otherwise,
            } => {
                if Self::resolve_unit(test, exprs)? {
                    Self::resolve_unit(then, exprs)
                } else {
                    Self::resolve_unit(otherwise, exprs)
                }
            }
            Expr::Xor(vars) => vars
                .iter()
                .map(|x| Self::resolve_unit(x, exprs))
                .collect::<Option<Vec<_>>>()
                .map(|bools| bools.into_iter().reduce(|x, y| x ^ y).unwrap_or(false)),
        })
    }

    /// The set of vars dice actually evaluates when computing `var` under
    /// `exprs`, including `var` itself. Respects `Cond` short-circuit: only
    /// the branch selected by `test` is traversed. If `test` cannot be
    /// resolved (e.g. depends on a var without an equation) both branches
    /// are included, since dice would try each in turn until one succeeds.
    fn subgraph(var: Var, exprs: &HashMap<Var, Expr>) -> HashSet<Var> {
        let mut out = HashSet::new();
        Self::traverse(var, exprs, &mut out);
        out
    }

    fn traverse(var: Var, exprs: &HashMap<Var, Expr>, out: &mut HashSet<Var>) {
        if !out.insert(var) {
            return;
        }
        let Some(expr) = exprs.get(&var) else {
            return;
        };
        match expr {
            Expr::Unit(u) => Self::traverse_unit(u, exprs, out),
            Expr::Xor(units) => {
                for u in units {
                    Self::traverse_unit(u, exprs, out);
                }
            }
            Expr::Cond {
                test,
                then,
                otherwise,
            } => {
                Self::traverse_unit(test, exprs, out);
                match Self::resolve_unit(test, exprs) {
                    Some(true) => Self::traverse_unit(then, exprs, out),
                    Some(false) => Self::traverse_unit(otherwise, exprs, out),
                    None => {
                        Self::traverse_unit(then, exprs, out);
                        Self::traverse_unit(otherwise, exprs, out);
                    }
                }
            }
        }
    }

    fn traverse_unit(unit: &Unit, exprs: &HashMap<Var, Expr>, out: &mut HashSet<Var>) {
        if let Unit::Variable(v) = unit {
            Self::traverse(*v, exprs, out);
        }
    }

    /// Whether dice's write for `w`, evaluated by a query at `q` while the head was
    /// `state.head_ctx`, attaches at the head rather than covering `q`: the premises of `w`'s
    /// compute (its equation, the values it reads, its untracked input) are the same at the head
    /// as at `q` and differ at some ctx in between, so the certificate covers two separate
    /// intervals and dice installs the one containing the head.
    fn reattaches_at_head(w: Var, q: usize, state: &AnswerKeyBuilder) -> bool {
        let head = state.head_ctx;
        if state
            .force_dirty_events
            .iter()
            .any(|(c, v)| *v == w && q < *c && *c <= head)
        {
            return false;
        }
        let premises = |c: usize| -> Option<(Expr, Vec<Option<bool>>)> {
            let eqs = state.equations_at_ctx.get(&c)?;
            let expr = eqs.get(&w)?;
            let reads = Self::reads(expr, eqs)
                .into_iter()
                .map(|d| Self::eval(d, eqs))
                .collect();
            Some((expr.clone(), reads))
        };
        let at_q = premises(q);
        if at_q.is_none() || at_q != premises(head) {
            return false;
        }
        (q + 1..head).any(|c| premises(c) != at_q)
    }

    /// The vars a compute of `expr` reads: for a `Cond`, the test and the branch it selects.
    fn reads(expr: &Expr, eqs: &HashMap<Var, Expr>) -> Vec<Var> {
        let var_of = |u: &Unit| match u {
            Unit::Variable(v) => Some(*v),
            Unit::Literal(_) => None,
        };
        match expr {
            Expr::Unit(u) => var_of(u).into_iter().collect(),
            Expr::Xor(units) => units.iter().filter_map(var_of).collect(),
            Expr::Cond {
                test,
                then,
                otherwise,
            } => {
                let mut out: Vec<Var> = var_of(test).into_iter().collect();
                let branch = match Self::resolve_unit(test, eqs) {
                    Some(true) => then,
                    Some(false) => otherwise,
                    None => return out,
                };
                out.extend(var_of(branch));
                out
            }
        }
    }

    /// Whether it is legitimate for dice to fire `Evaluated` on `w` for a
    /// query issued at ctx `ctx`.
    ///
    /// Loosening scenarios (returning `true`):
    /// - `transients_seen`: transient values skip caching, so any downstream
    ///   var may need to recompute in ways we don't model in detail.
    /// - Out-of-order query (`ctx < last_eval_ctx(w)`): dice may cache older
    ///   versions independently, and today's engine freely recomputes for
    ///   older ctxs against previously-evaluated newer ctxs.
    /// - `w` was never evaluated before: no cache entry can exist.
    /// - Some var in `w`'s ever-subgraph was mutated in `(last_eval_ctx(w), ctx]`:
    ///   `w` may need to re-check and possibly recompute. `ever_subgraph`
    ///   captures dep sets across all past evaluations of `w`, so we cover
    ///   cases where an equation change removed the dep from the current
    ///   graph but dice's recorded deps still reference it.
    /// - `w`, or some var in its ever-subgraph, has a *later* `ForceDirty` at some
    ///   ctx' > ctx: the key's one certificate may since have been re-stamped under
    ///   the newer revision of its untracked input, in which case it cannot be
    ///   revalidated at `ctx` (see `force_dirty_events`).
    /// - `w`, or some var in its ever-subgraph, was evaluated by an earlier query below
    ///   the head under premises that matched the head's again (see `DisplacedEval`):
    ///   that write attached the key at the head and displaced the claim that served
    ///   the versions in between, `ctx` among them, along with the value it named, so
    ///   dice recomputes there (and dependents miss their recorded revision).
    fn permit_recompute(w: Var, ctx: usize, state: &AnswerKeyBuilder) -> bool {
        if state.transients_seen {
            return true;
        }
        let last = match state.last_eval_ctx.get(&w) {
            None => return true,
            Some(v) => *v,
        };
        if ctx < last {
            return true;
        }
        let ever = state.ever_subgraph.get(&w);
        for (ctx_dirty, dirtied) in &state.dirty_events {
            if *ctx_dirty > last && *ctx_dirty <= ctx {
                if *dirtied == w {
                    return true;
                }
                if ever.is_some_and(|s| s.contains(dirtied)) {
                    return true;
                }
            }
        }
        for (ctx_fd, dirtied) in &state.force_dirty_events {
            if *ctx_fd > ctx {
                if *dirtied == w {
                    return true;
                }
                if ever.is_some_and(|s| s.contains(dirtied)) {
                    return true;
                }
            }
        }
        for displaced in &state.displaced {
            if ctx < displaced.head
                && (displaced.var == w || ever.is_some_and(|s| s.contains(&displaced.var)))
            {
                return true;
            }
        }
        false
    }
}

/// Records `ActivationData::Evaluated` events on `EvalVar` keys. Populated
/// by dice's `ActivationTracker` machinery; consumers snapshot-and-diff around
/// each query to attribute activations to specific queries.
#[derive(Default)]
struct RecomputeCollector {
    events: Mutex<Vec<Var>>,
}

impl RecomputeCollector {
    fn snapshot(&self) -> usize {
        self.events.lock().len()
    }

    fn events_since(&self, snapshot: usize) -> Vec<Var> {
        self.events.lock()[snapshot..].to_vec()
    }
}

impl allocative::Allocative for RecomputeCollector {
    fn visit<'a, 'b: 'a>(&self, _visitor: &'a mut allocative::Visitor<'b>) {}
}

impl ActivationTracker for RecomputeCollector {
    fn key_activated(
        &self,
        key: &DynKey,
        _deps: &mut dyn Iterator<Item = &DynKey>,
        activation_data: ActivationData,
    ) {
        if !matches!(activation_data, ActivationData::Evaluated(_)) {
            return;
        }
        if let Some(eval) = key.downcast_ref::<EvalVar>() {
            self.events.lock().push(eval.key);
        }
    }
}

#[derive(Clone, Debug)]
pub enum ExecutionResult {
    Correct,
    IncorrectResult {
        expected: bool,
        actual: bool,
    },
    /// A query returned an error, or a panic was observed anywhere in the
    /// process while it ran.
    UnexpectedPanic(String),
    /// A query neither finished nor panicked within [`QUERY_TIMEOUT`].
    Hung {
        query_index: usize,
        ctx_id: usize,
        var: Var,
    },
    /// Dice `Evaluated` (recomputed) at least one var during a query that the
    /// reference model considered a permissible cache hit.
    UnnecessaryRecompute {
        query_index: usize,
        ctx_id: usize,
        var: Var,
        unnecessary: Vec<Var>,
    },
}

/// How long a single query may run before it is reported as [`ExecutionResult::Hung`].
/// Queries in fuzz cases take milliseconds; this only has to be short enough that a
/// genuine deadlock does not stall a deep run indefinitely.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Every panic observed in the process since [`install_panic_hook`] ran.
///
/// A panic inside a dice task terminates that task without ever producing a result, so
/// the query depending on it waits forever instead of failing. Watching this log while a
/// query runs is what turns such a panic into a prompt failure.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PANIC_HOOK: Once = Once::new();

/// Whether the generator produces transient computation steps. There is a known dice bug
/// involving transients; gating them off makes other failures easier to find.
pub static GENERATE_TRANSIENTS: AtomicBool = AtomicBool::new(true);
/// Whether the generator produces queries at ctxs older than the newest one.
pub static GENERATE_OUT_OF_ORDER: AtomicBool = AtomicBool::new(true);

/// Records every panic in [`PANICS`], in addition to the default reporting. Idempotent.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            PANICS.lock().push(info.to_string());
            default_hook(info);
        }));
    });
}

/// The message of a caught panic's payload, for reporting.
pub fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}

/// Resolves once a panic has been recorded beyond the first `seen` entries of [`PANICS`].
async fn panic_observed(seen: usize) {
    loop {
        if PANICS.lock().len() > seen {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

enum QueryOutcome {
    Finished(anyhow::Result<bool>),
    Panicked,
    Hung,
}

impl Display for ExecutionResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionResult::Correct => {
                write!(f, "Correct")
            }
            ExecutionResult::IncorrectResult { expected, actual } => {
                write!(f, "Expected `{expected}` but got `{actual}`")
            }
            ExecutionResult::UnexpectedPanic(p) => {
                write!(f, "Expected result but panicked `{p}`")
            }
            ExecutionResult::Hung {
                query_index,
                ctx_id,
                var,
            } => {
                write!(
                    f,
                    "Query #{query_index} of {var} at ctx {ctx_id} did not finish within {QUERY_TIMEOUT:?}"
                )
            }
            ExecutionResult::UnnecessaryRecompute {
                query_index,
                ctx_id,
                var,
                unnecessary,
            } => {
                write!(
                    f,
                    "Query #{query_index} of {var} at ctx {ctx_id} unnecessarily recomputed {unnecessary:?}"
                )
            }
        }
    }
}

impl ExecutionResult {
    /// Two results are considered the same failure for the purpose of
    /// shrinker convergence.
    fn same_shape(&self, other: &ExecutionResult) -> bool {
        match (self, other) {
            (ExecutionResult::Correct, ExecutionResult::Correct) => true,
            (
                ExecutionResult::IncorrectResult {
                    expected: a,
                    actual: b,
                },
                ExecutionResult::IncorrectResult {
                    expected: c,
                    actual: d,
                },
            ) => a == c && b == d,
            (ExecutionResult::UnexpectedPanic(_), ExecutionResult::UnexpectedPanic(_)) => true,
            (ExecutionResult::Hung { var: a, .. }, ExecutionResult::Hung { var: b, .. }) => a == b,
            (
                ExecutionResult::UnnecessaryRecompute { var: a, .. },
                ExecutionResult::UnnecessaryRecompute { var: b, .. },
            ) => a == b,
            _ => false,
        }
    }

    pub fn is_failure(&self) -> bool {
        !matches!(self, ExecutionResult::Correct)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    /// Evaluate the variable at the version immediately after
    /// the SetValue op with this ctx_id.
    Query { ctx_id: usize, var: Var },
    /// Set the injected key corresponding to this variable.
    SetValue {
        new_ctx_id: usize,
        var: Var,
        expr: Expr,
    },
    /// Force the next evaluation of this var to do something other than just
    /// returning the correct result (e.g. force DICE to treat the value as
    /// transient).
    /// For now, this can't change the boolean result of the eval, since
    /// that'll break our ability to check DICE's computations are correct.
    EnqueueStep(Var, Vec<ComputationStep>),
    /// Mark the *computed* `EvalVar(var)` dirty at a new ctx via
    /// `changed(vec![EvalVar { key: var, .. }])`. This is the same shape as
    /// buck2's file-watcher-driven invalidation: force the next touch of a
    /// computed key to re-run its `compute`, letting dice's downstream cutoff
    /// determine whether dependents actually change. Equations are unchanged;
    /// expected values are unchanged. Dice rejects `changed()` on an
    /// `InjectedKey`, which is by design (an injected value has nothing to
    /// recompute) — targeting `LookupVar(var)` here would be incoherent.
    ForceDirty { new_ctx_id: usize, var: Var },
    /// Re-inject `LookupVar(var)`'s value with an Expr that evaluates to the
    /// same boolean but compares unequal as an `Arc<Expr>`. Exercises the
    /// injected-value path where a change is recorded and dice must fall back
    /// on value equality (`InvalidateKind::Update`) for downstream cutoff.
    /// Distinct from `ForceDirty` — that op invalidates the computed key
    /// (`InvalidateKind::ForceDirty`) while this one updates the injected
    /// value.
    ReinjectEquivalent { new_ctx_id: usize, var: Var },
}

fn uuid_serializer<S>(uuid: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&uuid.to_string())
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DiceExecutionOrder {
    #[serde(skip_deserializing, serialize_with = "uuid_serializer")]
    pub uuid: Uuid,
    /// A list of operations that initialize each var, in order, to a literal.
    pub init_vars: Vec<Operation>,
    /// A list of updates, queries, and enqueued "steps" that tweak computation.
    /// Notably, this doesn't introduce new vars, to simplify the shrink() implementation.
    pub timeline: Vec<Operation>,
    /// Are we shrinking an already-found failure? See comment in |execute|.
    pub is_shrinking: bool,
}

impl Debug for DiceExecutionOrder {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string_pretty(self).unwrap())
    }
}

pub struct DiceExecutionOrderOptions {
    pub print_dumps: Option<PathBuf>,
}

impl DiceExecutionOrder {
    pub const NSAMPLES_SEARCHING: usize = 1;
    pub const NSAMPLES_SHRINKING: usize = 100;
    const AVG_OPS_PER_VAR: usize = 4;
    const VARS_PER_XOR: usize = 5;

    /// Run the execution up to `samples` times, returning the raw per-sample results.
    /// Aggregation (matching against a previously-found failure, discarding
    /// nondeterministic mismatches) is handled by the caller; a sample that reproduces
    /// `stop_at` is the last one taken, since the shrinker learns nothing from the rest.
    pub async fn execute_raw(
        &self,
        samples: usize,
        options: &DiceExecutionOrderOptions,
        stop_at: Option<&ExecutionResult>,
    ) -> Vec<ExecutionResult> {
        let answer_key = MathAnswerKey::new(self);
        let mut out = Vec::with_capacity(samples);
        for run_count in 0..samples {
            let res = self.execute_once(&answer_key, run_count, options).await;
            if let Some(dump_loc) = self.get_dump_dir(options)
                && !res.is_failure()
                && let Err(e) = fs::remove_dir_all(dump_loc.join(format!("run-{run_count}")))
            {
                eprintln!("could not remove the dumps of a passing run: {e}");
            }
            let reproduces = stop_at.is_some_and(|prev| res.same_shape(prev));
            out.push(res);
            if reproduces {
                break;
            }
        }
        // The dump directory is kept for a failing case only, with the case beside its dumps.
        if let Some(dump_loc) = self.get_dump_dir(options)
            && dump_loc.exists()
        {
            if out.iter().any(|r| r.is_failure()) {
                if let Err(e) = write_case(&dump_loc.join("execution_order.json"), self) {
                    eprintln!("could not write the case beside its dumps: {e:#}");
                }
            } else if let Err(e) = fs::remove_dir_all(&dump_loc) {
                eprintln!("could not remove the dumps of a passing case: {e}");
            }
        }
        out
    }

    async fn execute_once(
        &self,
        answer_key: &MathAnswerKey,
        run_count: usize,
        options: &DiceExecutionOrderOptions,
    ) -> ExecutionResult {
        let collector = Arc::new(RecomputeCollector::default());
        let dice = Dice::builder().build(DetectCycles::Disabled);
        let root = dice.new_root().await;
        let mut dice_ctxs: HashMap<usize, DiceTransaction> = HashMap::new();
        let state = {
            let mut state = FuzzState::new();
            for var in self
                .init_vars
                .iter()
                .chain(self.timeline.iter())
                .map(|op| match op {
                    Operation::Query { var, .. } => *var,
                    Operation::SetValue { var, .. } => *var,
                    Operation::EnqueueStep(var, _) => *var,
                    Operation::ForceDirty { var, .. } => *var,
                    Operation::ReinjectEquivalent { var, .. } => *var,
                })
            {
                state.steps.entry(var).or_insert_with(SegQueue::new);
            }
            Arc::new(state)
        };
        let updater_with_tracker = || {
            let data = UserComputationData {
                activation_tracker: Some(collector.dupe() as Arc<dyn ActivationTracker>),
                ..Default::default()
            };
            dice.updater_with_data(root, data)
        };
        for (step_count, op) in self
            .init_vars
            .iter()
            .chain(self.timeline.iter())
            .enumerate()
        {
            match op {
                Operation::Query { ctx_id, var } => {
                    // Skip queries the reference model can't evaluate: they
                    // would require dice to fabricate a value for an
                    // InjectedKey that isn't injected at this ctx, which
                    // panics inside a dice task and leaves that task's
                    // dependents stuck for the rest of the run. Detecting the
                    // (None, Ok) case — a known dice bug where stale injected
                    // values leak across versions — is deferred to future
                    // work. The model skips its bookkeeping for the same
                    // queries.
                    if let Some(ctx) = dice_ctxs.get_mut(ctx_id)
                        && let Some(expected) = answer_key.value_of_query(step_count)
                    {
                        let snapshot = collector.snapshot();
                        let panics_seen = PANICS.lock().len();
                        let outcome = {
                            let mut computations = ctx.ctx();
                            let eval = computations.eval(state.dupe(), *var);
                            tokio::select! {
                                result = eval => QueryOutcome::Finished(result),
                                _ = panic_observed(panics_seen) => QueryOutcome::Panicked,
                                _ = tokio::time::sleep(QUERY_TIMEOUT) => QueryOutcome::Hung,
                            }
                        };
                        // A panic on a thread the query never joined still
                        // makes the result untrustworthy.
                        let new_panics = PANICS.lock()[panics_seen..].to_vec();
                        if !new_panics.is_empty() {
                            return ExecutionResult::UnexpectedPanic(new_panics.join("\n"));
                        }
                        let new_events = collector.events_since(snapshot);
                        match outcome {
                            QueryOutcome::Panicked => {
                                unreachable!("a panic was observed but not recorded")
                            }
                            QueryOutcome::Hung => {
                                return ExecutionResult::Hung {
                                    query_index: step_count,
                                    ctx_id: *ctx_id,
                                    var: *var,
                                };
                            }
                            QueryOutcome::Finished(Ok(actual)) => {
                                if expected != actual {
                                    return ExecutionResult::IncorrectResult { expected, actual };
                                }
                                if let Some(permitted) = answer_key.permitted_for_query(step_count)
                                {
                                    let unnecessary: Vec<Var> = new_events
                                        .into_iter()
                                        .filter(|v| !permitted.contains(v))
                                        .collect();
                                    if !unnecessary.is_empty() {
                                        return ExecutionResult::UnnecessaryRecompute {
                                            query_index: step_count,
                                            ctx_id: *ctx_id,
                                            var: *var,
                                            unnecessary,
                                        };
                                    }
                                }
                            }
                            QueryOutcome::Finished(Err(e)) => {
                                return ExecutionResult::UnexpectedPanic(format!("{e:#}"));
                            }
                        }
                    }
                }
                Operation::SetValue {
                    new_ctx_id,
                    var,
                    expr,
                } => {
                    let mut ctx = updater_with_tracker();
                    ctx.set_equation(*var, expr.clone()).unwrap();
                    dice_ctxs.insert(*new_ctx_id, ctx.commit().await);
                }
                Operation::ForceDirty { new_ctx_id, var } => {
                    let mut ctx = updater_with_tracker();
                    // Invalidate the computed key, not the injected equation
                    // store. This is the buck2-file-watcher pattern: dice's
                    // `InvalidateKind::ForceDirty` marks `EvalVar(var)` stale
                    // so its next touch runs `compute` again; equations are
                    // untouched, so `compute` returns the same value and
                    // dependents cutoff on equality.
                    ctx.changed(vec![state.eval_var(*var)]).unwrap();
                    dice_ctxs.insert(*new_ctx_id, ctx.commit().await);
                }
                Operation::ReinjectEquivalent { new_ctx_id, var } => {
                    let mut ctx = updater_with_tracker();
                    let reinject = answer_key
                        .reinject_replacement(step_count)
                        .cloned()
                        .expect("answer key missing ReinjectEquivalent replacement");
                    ctx.set_equation(*var, reinject).unwrap();
                    dice_ctxs.insert(*new_ctx_id, ctx.commit().await);
                }
                Operation::EnqueueStep(var, steps) => {
                    let queue = state.steps.get(var).unwrap();
                    for step in steps {
                        queue.push(*step);
                    }
                }
            }

            self.maybe_dump_dice(options, run_count, step_count, &dice)
                .expect("couldn't dump DICE to disk");
        }

        ExecutionResult::Correct
    }

    pub fn get_dump_dir(&self, options: &DiceExecutionOrderOptions) -> Option<PathBuf> {
        options
            .print_dumps
            .as_ref()
            .map(|loc| loc.join(self.uuid.to_string()))
    }

    fn maybe_dump_dice(
        &self,
        options: &DiceExecutionOrderOptions,
        run_count: usize,
        step_count: usize,
        dice: &Arc<Dice>,
    ) -> anyhow::Result<()> {
        if let Some(loc) = self.get_dump_dir(options) {
            let dump_path = loc
                .join(format!("run-{run_count}"))
                .join(format!("step-{step_count}"));

            fs::create_dir_all(dump_path.parent().unwrap())?;
            let mut dump_loc = File::create(&dump_path)?;
            serialize_dense_graph(
                &dice.to_introspectable(),
                &mut serde_json::Serializer::pretty(&mut dump_loc),
            )?;
        }
        Ok(())
    }
}

/// Writes `case` as pretty JSON to `path`, creating its directory, for `replay`.
pub fn write_case(path: &Path, case: &DiceExecutionOrder) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    serde_json::to_writer_pretty(File::create(path)?, case)?;
    Ok(())
}

/// The verdict of an execution's samples, as the shrinker reads it.
pub enum Verdict {
    /// Every sample passed.
    Passed,
    /// A sample reproduced the failure being shrunk, or is the first failure seen.
    Failed(ExecutionResult),
    /// Samples failed, but none like the failure being shrunk: nondeterminism, which the
    /// shrinker discards rather than follows.
    Nondeterministic,
}

/// Combines the per-sample results of an execution against the failure being shrunk, if any.
pub fn aggregate_samples(
    samples: &[ExecutionResult],
    previous: Option<&ExecutionResult>,
) -> Verdict {
    let mut ignored = false;
    for r in samples {
        if !r.is_failure() {
            continue;
        }
        match previous {
            None => return Verdict::Failed(r.clone()),
            Some(prev) => {
                if r.same_shape(prev) {
                    return Verdict::Failed(r.clone());
                }
                ignored = true;
            }
        }
    }
    if ignored {
        Verdict::Nondeterministic
    } else {
        Verdict::Passed
    }
}

impl Arbitrary for DiceExecutionOrder {
    fn arbitrary(g: &mut Gen) -> Self {
        let mut last_used_var = 0;
        let mut last_ctx_id = 0;
        let mut ctx_ids = vec![];
        let mut active_vars = vec![];
        let mut init_vars = vec![];
        let mut timeline = vec![];
        let mut var_values = Vec::<bool>::arbitrary(g);
        var_values.push(true); // Ensure non-empty.
        for val in var_values.iter() {
            last_ctx_id += 1;
            last_used_var += 1;
            ctx_ids.push(last_ctx_id);
            let var = Var(last_used_var);
            init_vars.push(Operation::SetValue {
                new_ctx_id: last_ctx_id,
                var,
                expr: Expr::Unit(Unit::Literal(*val)),
            });
            active_vars.push(var);
        }

        let select_var =
            |g: &mut Gen, vars: &[Var]| -> Unit { Unit::Variable(*g.choose(vars).unwrap()) };

        let arbitrary_expr = |g: &mut Gen, vars: &[Var]| -> Expr {
            if vars.is_empty() {
                return Expr::Unit(Unit::Literal(bool::arbitrary(g)));
            }
            // Semi-randomly select the expr type.
            match usize::arbitrary(g) % 100 {
                0..=33 => Expr::Unit(Unit::Literal(bool::arbitrary(g))),
                34..=66 => Expr::Xor({
                    let mut vec = Vec::new();
                    for _ in 0..Self::VARS_PER_XOR {
                        vec.push(select_var(g, vars))
                    }
                    vec
                }),
                _ => Expr::Cond {
                    test: select_var(g, vars),
                    then: select_var(g, vars),
                    otherwise: select_var(g, vars),
                },
            }
        };

        let gen_transients = GENERATE_TRANSIENTS.load(Ordering::Relaxed);
        let gen_out_of_order = GENERATE_OUT_OF_ORDER.load(Ordering::Relaxed);

        for _ in 0..Self::AVG_OPS_PER_VAR * g.size() {
            let i = usize::arbitrary(g) % active_vars.len();
            // Semi-randomly select a next op.
            timeline.push(match usize::arbitrary(g) % 100 {
                0..=40 => Operation::Query {
                    ctx_id: {
                        if gen_out_of_order {
                            *g.choose(&ctx_ids).unwrap()
                        } else {
                            cmp::max(ctx_ids.len() - 1, 0)
                        }
                    },
                    var: active_vars[i],
                },
                41..=50 if gen_transients => Operation::EnqueueStep(
                    *g.choose(&active_vars).unwrap(),
                    vec![ComputationStep::ReturnTransient],
                ),
                51..=57 => {
                    last_ctx_id += 1;
                    ctx_ids.push(last_ctx_id);
                    Operation::ForceDirty {
                        new_ctx_id: last_ctx_id,
                        var: *g.choose(&active_vars).unwrap(),
                    }
                }
                58..=60 => {
                    last_ctx_id += 1;
                    ctx_ids.push(last_ctx_id);
                    Operation::ReinjectEquivalent {
                        new_ctx_id: last_ctx_id,
                        var: *g.choose(&active_vars).unwrap(),
                    }
                }
                _ => {
                    last_ctx_id += 1;
                    ctx_ids.push(last_ctx_id);
                    Operation::SetValue {
                        new_ctx_id: last_ctx_id,
                        var: active_vars[i],
                        // Avoid circularity by forcing a key to only depend on earlier keys.
                        expr: arbitrary_expr(g, &active_vars[0..i]),
                    }
                }
            });
        }
        DiceExecutionOrder {
            uuid: Uuid::new_v4(),
            is_shrinking: false,
            init_vars,
            timeline,
        }
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        Box::new(DiceExecutionOrderShrinker::new(self.clone()))
    }
}

struct DiceExecutionOrderShrinker {
    pos: usize,
    seed: DiceExecutionOrder,
}

impl DiceExecutionOrderShrinker {
    fn new(seed: DiceExecutionOrder) -> Self {
        Self {
            pos: seed.timeline.len(),
            seed,
        }
    }
}

impl Iterator for DiceExecutionOrderShrinker {
    type Item = DiceExecutionOrder;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos == 0 {
            return None;
        }

        self.pos -= 1;
        Some(DiceExecutionOrder {
            uuid: Uuid::new_v4(),
            is_shrinking: true,
            init_vars: self.seed.init_vars.clone(),
            timeline: [
                &self.seed.timeline[0..self.pos],
                &self.seed.timeline[self.pos + 1..],
            ]
            .concat(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_key() {
        let order = DiceExecutionOrder {
            uuid: Default::default(),
            init_vars: vec![],
            timeline: vec![
                Operation::SetValue {
                    new_ctx_id: 1,
                    var: Var(1),
                    expr: Expr::Unit(Unit::Literal(true)),
                },
                Operation::SetValue {
                    new_ctx_id: 2,
                    var: Var(1),
                    expr: Expr::Unit(Unit::Literal(false)),
                },
                Operation::Query {
                    ctx_id: 1,
                    var: Var(1),
                },
                Operation::Query {
                    ctx_id: 2,
                    var: Var(1),
                },
                Operation::Query {
                    ctx_id: 1,
                    var: Var(2),
                },
            ],
            is_shrinking: false,
        };
        let answer_key = MathAnswerKey::new(&order);
        // Query at ctx=1 sees state as of ctx=1: var(1)=true.
        assert_eq!(Some(true), answer_key.value_of_query(2));
        // Query at ctx=2 sees state as of ctx=2: var(1)=false.
        assert_eq!(Some(false), answer_key.value_of_query(3));
        // Var(2) was never set — dice would panic on this query.
        assert_eq!(None, answer_key.value_of_query(4));
    }

    #[test]
    fn permitted_recomputes_after_force_dirty() {
        let order = DiceExecutionOrder {
            uuid: Default::default(),
            init_vars: vec![
                Operation::SetValue {
                    new_ctx_id: 1,
                    var: Var(1),
                    expr: Expr::Unit(Unit::Literal(true)),
                },
                Operation::SetValue {
                    new_ctx_id: 2,
                    var: Var(2),
                    expr: Expr::Xor(vec![Unit::Variable(Var(1))]),
                },
            ],
            timeline: vec![
                Operation::Query {
                    ctx_id: 2,
                    var: Var(2),
                },
                Operation::ForceDirty {
                    new_ctx_id: 3,
                    var: Var(1),
                },
                Operation::Query {
                    ctx_id: 3,
                    var: Var(2),
                },
            ],
            is_shrinking: false,
        };
        let answer_key = MathAnswerKey::new(&order);
        // The initial query (op index 2) touches Var(1) and Var(2) for the
        // first time — they're both allowed to compute.
        let first = answer_key.permitted_for_query(2).unwrap();
        assert!(first.contains(&Var(1)));
        assert!(first.contains(&Var(2)));
        // Second query (op index 4) at ctx=3, after force-dirty on Var(1);
        // Var(2) transitively reaches Var(1), so both remain permitted.
        let second = answer_key.permitted_for_query(4).unwrap();
        assert!(second.contains(&Var(1)));
        assert!(second.contains(&Var(2)));
        // Expected values unchanged: Var(2) = Var(1) = true at both ctxs.
        assert_eq!(Some(true), answer_key.value_of_query(2));
        assert_eq!(Some(true), answer_key.value_of_query(4));
        // ForceDirty leaves the equation store alone, so no reinjection
        // replacement is recorded for op index 3.
        assert!(answer_key.reinject_replacement(3).is_none());
    }

    #[test]
    fn reinject_equivalent_produces_replacement_and_preserves_value() {
        let order = DiceExecutionOrder {
            uuid: Default::default(),
            init_vars: vec![Operation::SetValue {
                new_ctx_id: 1,
                var: Var(1),
                expr: Expr::Unit(Unit::Literal(true)),
            }],
            timeline: vec![
                Operation::Query {
                    ctx_id: 1,
                    var: Var(1),
                },
                Operation::ReinjectEquivalent {
                    new_ctx_id: 2,
                    var: Var(1),
                },
                Operation::Query {
                    ctx_id: 2,
                    var: Var(1),
                },
            ],
            is_shrinking: false,
        };
        let answer_key = MathAnswerKey::new(&order);
        // Value is preserved across the reinjection.
        assert_eq!(Some(true), answer_key.value_of_query(1));
        assert_eq!(Some(true), answer_key.value_of_query(3));
        // Replacement is recorded and evaluates to the same value; because
        // the current expr is already `Literal(true)`, the replacement uses
        // the single-element Xor form to force Arc<Expr> inequality.
        let replacement = answer_key.reinject_replacement(2).cloned().unwrap();
        assert_eq!(replacement, Expr::Xor(vec![Unit::Literal(true)]));
    }

    #[test]
    fn permitted_recomputes_repeat_query_without_dirty() {
        let order = DiceExecutionOrder {
            uuid: Default::default(),
            init_vars: vec![Operation::SetValue {
                new_ctx_id: 1,
                var: Var(1),
                expr: Expr::Unit(Unit::Literal(true)),
            }],
            timeline: vec![
                Operation::Query {
                    ctx_id: 1,
                    var: Var(1),
                },
                Operation::Query {
                    ctx_id: 1,
                    var: Var(1),
                },
            ],
            is_shrinking: false,
        };
        let answer_key = MathAnswerKey::new(&order);
        // The second query re-queries an already-evaluated var at the same
        // ctx with no dirty events in between — recomputation is not
        // permitted.
        let second = answer_key.permitted_for_query(2).unwrap();
        assert!(!second.contains(&Var(1)));
    }

    fn tracker_user_data(collector: &Arc<RecomputeCollector>) -> UserComputationData {
        UserComputationData {
            activation_tracker: Some(collector.dupe() as Arc<dyn ActivationTracker>),
            ..Default::default()
        }
    }

    /// Engine-level MUST test for `Operation::ForceDirty`.
    ///
    /// The fuzz property is upper-bound only (`permit_recompute`): if
    /// `ForceDirty` silently regressed to a no-op — the hazard class of the
    /// `Arc::ptr_eq` identity in `EvalVar::PartialEq` — every fuzz run would
    /// stay green while dice's `InvalidateKind::ForceDirty` path went
    /// unexercised. This test asserts both sides of the contract:
    ///
    /// - Positive: after `ForceDirty(Var(1))`, a query touching `Var(1)`
    ///   fires `Evaluated` on `EvalVar(Var(1))`.
    /// - Control: a version bump from an unrelated `SetValue` does NOT
    ///   re-evaluate `EvalVar(Var(1))`.
    #[tokio::test]
    async fn force_dirty_reevaluates_eval_var() -> anyhow::Result<()> {
        // Positive: ForceDirty must reach the engine and mark EvalVar stale.
        {
            let collector = Arc::new(RecomputeCollector::default());
            let dice = Dice::builder().build(DetectCycles::Disabled);
            let root = dice.new_root().await;
            let state = Arc::new(FuzzState::new());
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(1), Expr::Unit(Unit::Literal(true)))?;
            u.set_equation(Var(2), Expr::Xor(vec![Unit::Variable(Var(1))]))?;
            let ctx1 = u.commit().await;
            assert!(ctx1.ctx().eval(state.dupe(), Var(2)).await?);
            let baseline = collector.snapshot();
            // Exactly what `execute_once` does for `Operation::ForceDirty`.
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.changed(vec![state.eval_var(Var(1))])?;
            let ctx2 = u.commit().await;
            assert!(ctx2.ctx().eval(state.dupe(), Var(2)).await?);
            let events = collector.events_since(baseline);
            assert!(
                events.contains(&Var(1)),
                "ForceDirty must re-evaluate EvalVar(Var(1)); tracker saw {events:?}"
            );
        }
        // Control: a pure version bump on an unrelated var must not
        // re-evaluate Var(1). Distinguishes ForceDirty's effect from the
        // baseline transaction-boundary noise.
        {
            let collector = Arc::new(RecomputeCollector::default());
            let dice = Dice::builder().build(DetectCycles::Disabled);
            let root = dice.new_root().await;
            let state = Arc::new(FuzzState::new());
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(1), Expr::Unit(Unit::Literal(true)))?;
            u.set_equation(Var(2), Expr::Xor(vec![Unit::Variable(Var(1))]))?;
            u.set_equation(Var(3), Expr::Unit(Unit::Literal(false)))?;
            let ctx1 = u.commit().await;
            assert!(ctx1.ctx().eval(state.dupe(), Var(2)).await?);
            let baseline = collector.snapshot();
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(3), Expr::Unit(Unit::Literal(true)))?;
            let ctx2 = u.commit().await;
            assert!(ctx2.ctx().eval(state.dupe(), Var(2)).await?);
            let events = collector.events_since(baseline);
            assert!(
                !events.contains(&Var(1)),
                "unrelated SetValue must not re-evaluate Var(1); tracker saw {events:?}"
            );
        }
        Ok(())
    }

    /// Engine-level MUST test for `Operation::ReinjectEquivalent`.
    ///
    /// Same rationale as the `ForceDirty` test — the fuzz property is
    /// upper-bound only, so we need a direct assertion that dice's
    /// `InvalidateKind::Update` on `LookupVar` actually forces `EvalVar` to
    /// re-check and re-run. Uses the same replacement shape as the fuzzer
    /// (`Xor([Literal(current_value)])` when the current expr is already the
    /// candidate literal).
    #[tokio::test]
    async fn reinject_equivalent_reevaluates_eval_var() -> anyhow::Result<()> {
        // Positive: value-preserving re-injection must still bump the version
        // and force EvalVar(Var(1)) to recompute.
        {
            let collector = Arc::new(RecomputeCollector::default());
            let dice = Dice::builder().build(DetectCycles::Disabled);
            let root = dice.new_root().await;
            let state = Arc::new(FuzzState::new());
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(1), Expr::Unit(Unit::Literal(true)))?;
            u.set_equation(Var(2), Expr::Xor(vec![Unit::Variable(Var(1))]))?;
            let ctx1 = u.commit().await;
            assert!(ctx1.ctx().eval(state.dupe(), Var(2)).await?);
            let baseline = collector.snapshot();
            // Matches the ReinjectEquivalent replacement shape from
            // `MathAnswerKey::new` when the current expr is Literal(true).
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(1), Expr::Xor(vec![Unit::Literal(true)]))?;
            let ctx2 = u.commit().await;
            assert!(ctx2.ctx().eval(state.dupe(), Var(2)).await?);
            let events = collector.events_since(baseline);
            assert!(
                events.contains(&Var(1)),
                "ReinjectEquivalent must re-evaluate EvalVar(Var(1)); tracker saw {events:?}"
            );
        }
        // Control: re-injecting the *identical* Expr short-circuits dice's
        // `on_injected` equality check (no version bump); Var(1) must not
        // re-evaluate. Also confirms that the positive result above depends
        // on the injected value comparing unequal, not on any other side
        // effect of `set_equation`.
        {
            let collector = Arc::new(RecomputeCollector::default());
            let dice = Dice::builder().build(DetectCycles::Disabled);
            let root = dice.new_root().await;
            let state = Arc::new(FuzzState::new());
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(1), Expr::Unit(Unit::Literal(true)))?;
            u.set_equation(Var(2), Expr::Xor(vec![Unit::Variable(Var(1))]))?;
            let ctx1 = u.commit().await;
            assert!(ctx1.ctx().eval(state.dupe(), Var(2)).await?);
            let baseline = collector.snapshot();
            let mut u = dice.updater_with_data(root, tracker_user_data(&collector));
            u.set_equation(Var(1), Expr::Unit(Unit::Literal(true)))?;
            let ctx2 = u.commit().await;
            assert!(ctx2.ctx().eval(state.dupe(), Var(2)).await?);
            let events = collector.events_since(baseline);
            assert!(
                !events.contains(&Var(1)),
                "identical re-injection must not re-evaluate Var(1); tracker saw {events:?}"
            );
        }
        Ok(())
    }
}
