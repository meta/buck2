# Dice incrementality

## 1. Introduction

Dice is an incremental compute engine. A user supplies keys with compute functions, sets some
inputs, asks for some outputs, changes some inputs, and asks again. Dice's job is to give correct
answers while doing as little recompute as possible — under concurrent access to multiple historical
states, and, the driving case for this design, under multiple concurrently evolving lineages of
state.

At the center of the engine sits a data structure — the core state — that is the subject of this
document. The core state's primary concern is the sound reuse of computed values across "versions" -
different settings of the inputs. The core state on its own is not a build system as traditionally
understood, as it does not itself manage any kind of execution. In particular, when asked for the
value of a key at a version it returns either the value or "unknown"; computing an appropriate value
in the case of "unknown" is not the core state's responsibility.

The core state should be understood to live in the context of an execution environment which takes
that responsibility on; in particular, the execution environment understands and manages execution of
tasks and reports their results to the core state. In doing so, the execution environment must
additionally collect and report the dependencies - gathered either statically or dynamically - which
ground the definition of "sound reuse."

This doc:

- §2 defines the interface: the objects (keys, revisions, versions, untracked
  inputs, certificates) and the operations over them.
- §3 defines what the structure's answers mean — a justification judgment
  built inductively from the operation history — and states the soundness
  theorem. The theorem is unconditional: it assumes nothing about the caller.
  In particular it needs neither determinism of computes nor completeness of
  change reporting; both classical assumptions turn out to belong to layers
  above this interface.
- §4–§5 give the implementing algorithm — single validity windows, branches,
  and exact reverse-dependency maps — presented as a memoization of §3's
  judgment, with the invariants that make every operation preserve it.
- §6 treats completeness: what reuse the algorithm does and does not promise,
  and why unconditional completeness is deliberately not a goal.
- Appendix A specifies the value layer dice puts on top — interning values to
  revisions, equality, retention and garbage collection. Appendix B covers the
  series-parallel dep structure, an environment payload the core carries but
  never interprets.
  Appendix C discusses cycles: what the inductive judgment says about them,
  and when a different implementation of this interface might prefer the
  coinductive reading.

## 2. The interface

### 2.1 Objects

**Keys.** Opaque identifiers at which values are computed. Formally, naturals, though the only
operation we will ever perform on them is equality comparison. Each key is of one of two different
types, either an injected key or a non-injected key; these might be more traditionally called input
and intermediate/output keys, respectively, and they should be understood in that light.

**Revisions.** An opaque label for one distinct value of one key, minted by the execution
environment. These are also formally naturals and we also require only equality comparison on them.
We will treat them as globally distinct for simplicity, though we will actually not take advantage
of that. In particular, revisions are never compared across keys so separate monotonically
incrementing revision counters per key are an option that an implementation may want to take (and
ours does).

Dice itself provides a somewhat more expressive API in terms of comparable values; the primary
benefit of this API is that it enables somewhat more careful resource management, i.e. deciding
when to discard values. We discuss this in Appendix A.

**Versions and branches.** The structure's subject is change. A **commit** records a set of changes
— specifically a list of injected keys and new values for them — and creates a **version**: the
immutable name of the state reached after a particular chain of commits. A **branch** is a line of
versions extended by commits at its latest version, its **head**; `fork` starts a new branch from a
version of an existing one, so versions form a forest. A version is written `(b, s)` — branch and
seq — where seqs order the commits of one branch and carry no cross-branch meaning. Branch ids are
minted in creation order and a branch is younger than the one it was forked from, so along any
chain `(b, s)` increases lexicographically; versions of different chains have no order.

**Untracked inputs.** Computes can read things the dep graph does not track — a file-content key
reads the disk. To make this common case convenient, the interface gives every key k an implicit
**untracked input** k′ standing for everything outside the graph that k's compute consumes. k' acts
in all ways as an injected key that is an implicit dependency of k, but cannot generally be named in
the API and in exchange is given a significantly simplified representation. All one can do is report
change: a **force-dirty** of k at a commit asserts that k' has changed to a minted revision ("what k
reads out there has changed; to what, nobody can say"). In effect, this forces k to be re-computed
across such changes, matching the need to reflect changes to untracked inputs. When reporting the
result of a computation for k, the k′ revision it observed is reported together with the state of
other deps, preventing inappropriate reuse. Dice's code calls these revisions **ε tokens**; ε below
always means the untracked-input revision a certificate records.

**Certificates.** A **trace** of one completed computation:

```
Cert {
    key:      Key,
    revision: Revision,               // of key's value
    deps:     Set<(Key, Revision)>,   // what the compute read, and what it saw
    epsilon:  Revision,               // of key's untracked input, ditto
}
```

The intended reading: "key's compute read exactly `deps` plus the untracked input, observing each at
the recorded revision, and produced the value named `revision`." Semantically ε is one more dep
edge, `(key′, epsilon)`, kept as its own field only because it is implicit and always present.

A key may evaluate to the same revision under more than one input valuation, so certificates relate
to `(key, revision)` many-to-one: every computational circumstance that produced an equal value is a
distinct trace of the same revision. Early cutoff, seen from the other side, is the common source: a
dep changes, the value does not, and the recompute re-finds the revision under new dep revisions —
likewise under a new ε after a force-dirty, or with a changed dep set.

Dep sets are unordered. Dice's evaluation engine records deps in a richer series-parallel structure,
which the core carries as an opaque payload for the environment's own re-evaluation walks; the
semantics reads it as the set (Appendix B).

**Attributes.** The environment may define an attribute of derivations (§4.7): a value the
structure computes for every claim it holds and returns with every `Valid` lookup. Dice's is the
invalidation sources of a value, the most recent changes it depends on.

### 2.2 Operations

```
commit(b: BranchId, assertions: Map<Key, Revision>, dirties: Set<Key>) → Seq
fork(v: Version) → BranchId
new_root() → BranchId
lookup(k: Key, v: Version) → Valid(Revision, Attribute)
                           | Unknown { candidate: Option<Cert>, epsilon: Revision, untracked: Attribute }
write(cert: Cert)
```

The core state in itself is not a concurrent data structure. The operations on it are performed in
some global total order. However, there is also no constraint that operations are done in monotonic
version order or any other order; as such, the execution environment can be highly concurrent,
including across versions and branches.

**`commit`** mints a new version following b's head: each assertion `k ↦ r` declares that k has
revision r there and onward until superseded by a later assertion, and each dirty is shorthand for
asserting a freshly minted revision on that key's untracked input. Assertions are the axioms of §3's
judgment — the structure's ground truth is *defined* by them.

**`fork`** creates a branch whose world is v's and whose subsequent commits diverge from it.

**`new_root`** creates a branch with no parent. Nothing resolves at its initial version except
certificates whose premises hold there, which at first is only those without premises; its world
is what its commits assert.

**`lookup`** is the operation the soundness theorem is about. `Valid(r)` claims that r is justified
for k at v in the sense of §3 — that is the entire contract. `Unknown` claims nothing and is
therefore always a sound answer. The values provided with `Unknown` are service, not contract:
`candidate` is a previously written certificate for k that the caller may try to re-establish,
`epsilon` is the current revision of k's untracked input at v, for stamping the certificate the
caller is presumably about to produce (provided because there is no direct `lookup(k')` operation),
and `untracked` is the attribute of that revision's assertion, a leaf of the derivation of whatever
the caller produces.

**`write`** hands the structure a certificate. Note the signature: no version. This is designed
specifically to make it easy for an execution environment to produce soundly from a direct trace of
the execution. Discovering how to usefully apply this certificate is fully the core state's
responsibility. Though `write` is semantically idempotent, the implementation we describe below
benefits in particular circumstances from the execution re-issuing a `write` identical to a previous
one; we'll refer to this operation as **`revalidate`**.

**Intended use** (not normative): the execution environment serves a read of k at v by calling
`lookup`. On `Valid(r)` it serves the value it associates with `(k, r)`. On `Unknown` with a
candidate it re-establishes the candidate: each dep re-checked recursively by this same protocol, ε
against `epsilon`; if everything still matches the recorded revisions, perform a `revalidate` and
return the revision. Otherwise it computes k, mints or re-finds a revision for the result and
`write`s the trace. Changes to the inputs are discovered and reported in appropriate domain-specific
ways.

### 2.3 Resource Management Operations

In addition to the semantic operations above, an implementation of this API is likely to want to
provide some operations for resource management, for example reporting a branch to no longer be in
use; ours is `delete`, §5.5. One constraint they all share: forgetting claims and values is always
sound (§3.3), but they must still take care to not accidentally reset the revision counter and
cause reuse of revision numbers (or similar bugs).

## 3. Semantics

### 3.1 The recorded history

Fix any point in an execution. Everything the structure has been told so far
is a pair: W, the set of certificates written, and the **recorded history**

```
H = (V, parent: V ⇀ V | None, A: V → ((Key ∪ Key′) ⇀ Revision))
```

where V is the set of versions minted so far and Key′ is the set of untracked inputs. `parent` gives
the version each version follows — `(b, s+1) ↦ (b, s)` for a commit, the fork point for a branch's
initial version, nothing for a root's. A gives each version the assertions made at it: a commit
contributes its assertion map plus `k′ ↦ fresh` for each dirtied k, and a root's initial version
implicitly asserts an initial revision for every untracked input (never-dirtied keys need one for
their certificates to speak of).

`path(v)` is the chain from v to its root along `parent`. For x an injected key or an untracked
input:

```
asserted(x, v)   the revision A assigns x at the nearest version on path(v)
                 that mentions x, if any
```

### 3.2 The judgment

The **justification judgment** `H; W ⊨ x@r at v` is the least relation
closed under:

```
(Assert)   asserted(x, v) = r
           ──────────────────
           x@r at v

(Cert)     (k, r, D, ε) ∈ W
           ∀ (x, r') ∈ D ∪ {(k′, ε)}:  x@r' at v
           ─────────────────────────────────────
           k@r at v
```

One consequence of this definition is worth immediately calling out: This judgement is monotonic
with respect to additional operations on the data structure; additional commits, writes, etc. never
make a previously valid conclusion invalid (though, as stated, this does not extend to any promises
about completeness in the API).

We require the *least* relation specifically so as to disallow cycles/coinductive reasoning. This
decision could reasonably be made differently for some circumstances, though our implementation
would in any case not be capable of taking advantage of it. We discuss all this more in Appendix C.

Justification is relative to the recorded history alone — whether assertions track the outside
world, whether certificates are honest traces of real computes, whether computes are functions: none
of it enters. Those are fidelity properties *of the environment*, and what they buy is §3.4's
subject; the judgment is the interface between the two.

### 3.3 Soundness

**Theorem (lookup soundness).** At any point in any execution, if
`lookup(k, v)` returns `Valid(r)`, then `H; W ⊨ k@r at v` for the recorded
history and certificates at that point.

This theorem states the central contract of the API and will be proven for our implementation below.
It is one-directional by design: nothing obliges the structure to *find* a justification — `Unknown`
is always permitted — making every form of forgetting trivially sound. Partial completeness of our
algorithm is discussed in §6.

### 3.4 What the environment's fidelity buys

The judgment is a relation, not a function: nothing prevents several
revisions from being justified for the same (k, v). An environment whose
computes are nondeterministic will produce exactly that, and the
structure's only promise is that a `Valid` answer is *some* justified
revision. Two add-on results recover the classical picture for environments
that behave:

**Uniqueness.** Say W is **functional** for k when all of k's certificates
are traces of one deterministic reader — a procedure that reads inputs one
at a time (k′ among them), each choice of what to read next determined by
the revisions seen so far, the emitted revision determined by the whole
valuation. If W is functional for every key and no key is both asserted and
certified, at most one revision is justified per (k, v). *Sketch:*
induction over derivations at fixed v. A derivation's input revisions are
unique by the inductive hypothesis; two traces of one reader over the same
valuation read the same inputs and emit the same revision; the no-mixing
condition rules out an assertion diverging from a certificate.

**From-scratch consistency.** If moreover every certificate is an **honest
trace** — running k's compute against the values behind its recorded input
revisions reproduces the value behind its emitted revision — and assertions
are **complete with respect to the world** — tracked inputs' changes are
asserted, and dirties cover every change to untracked state some compute
reads — then justification implies agreement with recomputation: if k@r is
justified at v, a from-scratch run at v's asserted inputs produces r's
value, by the same induction. This corollary is what an environment
actually wants from the structure, and its hypotheses are where the
classical obligations live. Report too few changes and the structure's
answers stay sound relative to what was recorded — they are the results of
computes against a world that no longer exists.

## 4. The state

The state is a memoization of §3's judgment: a collection of materialized
**claims** — statements of the form "x resolves to `Valid(r)` over this set
of versions" — held in three stores: slots (per key × branch), assertion
histories (per asserted key × branch), and the branch tree that gives
versions their meaning. Nothing else carries validity information. One rule
governs all of it:

**Master invariant (S).** Every materialized claim is justified: if the
resolution procedure of §4.4 yields `Valid(r)` for x at a version v, then
`H; W ⊨ x@r at v`.

Every operation in §5 preserves S, and §3.3's theorem is S read off
at `lookup`. Since `Unknown` is always sound, forgetting claims never
threatens S; every soundness argument below is about creating or reshaping
claims.

### 4.1 Slots

The slot store is a global map `Key → (Branch → Slot)` holding at most one slot per key and branch;
it only deals with non-injected keys, injected keys are treated below.

```
Slot {
    branch: BranchId,
    // the k claim, if any: revision valid on this branch for seqs in [valid_from, valid_until)
    claim: Option<Claim>,
    // the k′ claim: the history of all asserts on the untracked input (on this branch)
    untracked: Map<Seq, Revision>,
}

Claim {
    cert: Cert,
    valid_from: Seq,
    valid_until: Seq | Open,   // Open ⇔ attached
}
```

FIXME(JakobDegen): This is missing a storage representation for injected keys.

A slot is two claims sharing one struct. The **k claim** asserts the key's revision over one
contiguous window `[valid_from, valid_until)`; `Open` means unbounded above, and a claim with
`valid_until = Open` is **attached**, otherwise **closed**. The **k′ claim** is the full history of
k′ on this branch, indexed by this branch's own seqs (§4.3).

It's worth noting early that the presence or absence of slots in the physical slot map is
semantically completely uninteresting; it is always sound to add a slot with no claim and empty
untracked map at any key at and any branch (or to remove such slots). The presence or absence of a
claim, however, is semantically important as will be seen below.

The certificates pinned on claims are the only ones retained; other certificates are forgotten and
degrade responses towards `Unknown`.

### 4.2 Branches

```
Branch {
    parent: Option<(BranchId, Seq)>,   // None for a root
    children: [(BranchId, Seq)],       // live children and their fork seqs
    seq: Seq,                           // per-branch commit counter
    rdeps: RdepMap,                     // complete for this branch
    closed_index: Set<DiceKey>,         // keys whose claim at b has a nonempty closed window
}
```

`head_b` denotes the branch's current head version, `(b, b.seq)`.

`RdepMap` is `Key → set of dependent Keys` — untagged key sets. Each branch's map is complete; we
will describe this requirement more precisely below, but it should be understood as being related to
the attached state; for an attached key, its dependency edges are present in the rdep map.

`closed_index` holds exactly the keys whose claim at this branch has a nonempty closed window. It is
maintained by every operation that writes, opens, or removes a window; it is consulted only at fork
time (§5.4), to avoid an all-nodes scan there.

### 4.3 Injected keys

Dice's injected keys are the keys the execution environment asserts and never certifies. Their
storage materializes the assertion component A of the recorded history:

- Per branch and key, a full **injection history** `Seq → Revision`, appended by commit.
- Unlike with certs, the history of asserted values is complete; this is an important property of
  the API, since unlike non-injected keys, they cannot be recomputed if the core state returns
  `Unknown`. We additionally rely on this property for soundness of the algorithm itself.
- No certificates and no claims. At version (b, s) an asserted key resolves to the greatest entry
  with seq ≤ s in the branch's own history. If there is none — the history is empty, or its first
  entry is later than s — resolution falls through to the parent at the fork seq (rule 3, §4.4),
  and is `Unknown` at a root.

For injected keys that are untracked inputs, this injection history is stored directly in the slot
for the parent key k. For other injected keys, storage of the injection history is kept separately.

### 4.4 Resolution

Our core read operation is *resolution*, `resolve: (Key, Version) -> { Valid(r), Unknown }` — the
operation that acts as a counterpart to the judgment. For a non-injected key K:

1. **Own claim.** If K has a claim at b whose window covers s (`valid_from ≤ s < valid_until`, with
   `Open` = ∞): `Valid(claim.cert.revision)`.
2. **Own claim, not covering.** If K has a claim at b that does not cover s: `Unknown`.
3. **No claim.** If K has no claim at b — no slot, or a slot without one — then
   `resolve(K, (b, s)) = resolve(K, parent(b))` — evaluated at the fork-point version — for every s
   on b. Delegation is total across b's timeline: b's world is its parent's fork-point state plus
   b's own recorded divergences. If the parent chain runs out (no parent), `Unknown` — K was never
   computed for this lineage.

For injected keys and untracked inputs, rule 1 consults the branch's history (§4.3) instead of a
claim, with the same delegation to the parent when the history has no entry at or before s.

`resolve` is O(chain length), touches one node, and enumerates no branches. It also reports the
resolving claim — the owning branch, its slot, the covering window — which sections below use
throughout.

We define the *window* for a key K at a branch b to be the interval of sequence numbers in a branch
at which resolution returns valid; the window stored in the claim if K has one at b, otherwise
`[0, head_b + 1)` if resolution is valid or an empty set if not.

The inheritance mechanism is in many ways the crux of this algorithm; it allows branches to be
relatively cheap while making common cases in which reuse is allowed easy to express.

Any operation that changes the k claim of a particular key and branch, including creating or
removing one, is called an installation. Appending to a k′ history is not an installation of k: it
changes the resolution of k at no existing version. The price we pay for inheritance is that an
installation at some branch may not only affect resolution of versions at that branch, but also the
resolution of versions on descendant branches. To ensure that such changes are correct and maintain
key invariants we may in general have to make further changes at the children, and we'll call these
"cascade obligations."

### 4.5 Certificate coverage

For a certificate C of key K, define its **coverage on branch b**:

```
cover_b(C) = { s : resolve(x, (b, s)) = Valid(r) for every premise
                   (x, r) ∈ C.deps ∪ {(K′, C.epsilon)} }
           ∩ [0, head_b]
```

If `s ∈ cover_b(C)` then, by S, every premise of C is justified at (b, s), so by (Cert) so is
`K@C.revision`: installing that claim there preserves S. Coverage is (Cert)'s premise set read off
the materialized state, with S as the induction hypothesis. This is not a statement about what
`resolve` currently returns for K; the state may well say `Unknown` throughout the cover.

`cover_b` is computable cheaply; except for injected premises, the set of seqs at which a particular
premise is valid is one contiguous interval, so `cover_b` is computable as an O(deps) intersection
of windows. Injected keys throw a bit of a wrench in there as revisions may be in use across
non-contiguous key ranges; in practice we just pick the most recent interval and ignore others.

### 4.6 Rdeps and attachment

Before discussing how we handle rdeps and attachment, we'll first state below a number of related
invariants we maintain. These can all be checked to hold vacuously when the state is initialized as
empty and we will argue on each operation below that invariants are maintained as needed, thereby
reconstructing an inductive proof of their correctness.

**Invariant 1 (window-head).** A claim with a nonempty closed window has `valid_until ≤ head_b`;
equivalently, a claim that covers the current head is necessarily Open. This makes "covers the head"
and "attached" interchangeable.

**Invariant 2 (attachment is supported).** Let K be a key and b a branch such that resolution of K
at `head_b` is `Valid` (via a claim there or inheritance). Then for every premise `(x, r)` of the
governing cert — the k′ premise included — x's resolution at `head_b` is `Valid(r)` as well.
Furthermore, the window for K at b is a subset of the set of seqs at which x resolves to r on b.

**Invariant 3 (complete maps).** For every K whose resolution at `head_b` is `Valid`, and every dep
`(D, _)` of the governing cert, `D → K` is present in b's own `RdepMap`. The k' premise is not
included here; we handle it directly on force-dirties. Edges are how commits keep Invariant 2's
promises — they find every dependent that must close. We allow the rdep maps to contain edges
besides the ones required by this invariant.

An operation that sets `valid_until` on some claim to `Open` is called "attaching." The inverse
operation is "detaching."

The key goal behind these invariants is that we wish to make `commit`ting some changes and looking up
the values of unaffected keys take time proportional to the rdeps of the changed keys, not the deps
of the requested ones. This notion of attachment lets us do that, giving attached keys a
representation that implicitly makes them valid at newly committed versions until a change actually
affects them.

### 4.7 Derivation attributes

The environment may have the structure carry an **attribute** of derivations. It supplies a set A
with an element `empty`, a function `assertion: Version × AssertionData → A`, and a commutative,
associative `combine: A × A → A` with `empty` as its identity. The attribute `attr(T)` of a
derivation T of §3.2's judgment is defined inductively: at an (Assert) leaf for an assertion made
at version u with data d, `assertion(u, d)`; at a (Cert) step, the combination of the attributes of
the premises' derivations, the k′ premise included, where the implicit initial assertion of an
untracked input contributes `empty`.

Dice's attribute is a value's invalidation sources: per source priority, the version of the most
recent assertion in the derivation. Its `combine` keeps the later version per priority, which is
well-defined because all assertions in one derivation at v are in force at v, and so lie on
path(v).

The structure maintains `attr(T(k, v))` on every claim with a nonempty window, where T(k, v) is
the **materialized derivation**: the one that justifies each premise through the claim or history
entry that resolution finds. `lookup` returns it with `Valid`; for an injected key it returns
`assertion` of the entry in force. The attribute is a function of the derivation, not of the
certificate: one certificate installed on two branches whose assertions differ carries two
attributes, which is why the structure, not the environment, computes it.

**Constancy across a window.** A window lies within the cover, and the cover is the intersection
of the premises' intervals (§4.5): the window of a claim, which has one derivation by induction, or
one interval of a history, over which one entry is in force. So every premise has one derivation
across the window, and the attribute is the same at every version of it; and the same again at
every version of a branch that inherits the claim, since inheritance means no commit of the
inheriting branch has touched a premise.

**Where it is computed.** At install, in the same pass as the cover: the pieces of §4.5 carry the
attribute of the premises considered so far, intersection combines, and the installed window takes
its piece's attribute. Every other way a claim comes to be copies a claim whose derivation it
shares: closing an inherited claim (§5.3), re-homing a child's view across an installation at its
parent (§5.2), and the copies `delete` makes (§5.5). A fork's residuals and a write's shadows have
empty windows and no attribute worth the name.

**What `delete` does to it.** `delete` moves the assertions a child inherited to the child's first
version. Attributes computed before refer to the deleted branch's versions for those assertions,
and the structure leaves them be: those versions remain distinct from all others and keep their
place in the order of §2.1, so an attribute that orders versions along a chain, like dice's, is
unaffected, and an environment that needs more can carry an identity in its assertion data.

## 5. Operations

### 5.1 `lookup(K, (b, s))`

Returns:

- **`Valid(r)`** if `resolve(K, (b, s))` yields `Valid(r)`, with the attribute (§4.7) of the
  resolving claim, or of the resolving assertion for an injected key.
- If resolution is `Unknown`, but a **candidate** cert exists — the witnessing cert of the nearest
  claim on the resolution chain, whatever its window, or else any other claim on the node - returns
  the cert together with `resolve(K′, (b, s))` as the `epsilon` field and the attribute of that
  assertion as `untracked`. The caller attempts to establish `(b, s) ∈ cover_b(cert)` by
  re-checking each premise's resolution at `(b, s)` against the cert's recorded revisions; success
  is a successful revalidation (§2.2's `revalidate`).
- If resolution is `Unknown` with no candidate cert, return `Unknown`. The caller runs a full
  recompute.

### 5.2 `write`

Let K be the key on the cert. Build a candidate list of branches b at which we will attempt to
perform a write, initialized to all branches in any ancestor-first order.

Pop branches off the front of the candidate list until it is empty. When popping a branch, check if
it is attached (possibly via inheritance). Skip it if so; attached branches may not be detached.
Otherwise, for the popped branch b:

**1. Install**: Compute `cover_b(C)`. If the cover is non-empty, install one interval of it (§4.5)
as the key's claim at b, with the attribute of the derivation there (§4.7), creating the slot if
there is none; where the cover reaches the head, attach it as well by inserting the cert's dep edges
`D → K` into the rdeps map.

The soundness of resolution of K on versions in b is immediate from the definition of `cover_b(C)`,
since it precisely matches the conditions of the soundness judgement by induction. If attaching,
after inserting the deps into the rdeps map, the attachment invariants being maintained is also
immediate, again primarily by the definition of `cover_b(C)`.

The install or attach may be skipped if it is deemed not profitable; we implement the policy of
installing if the window of the new cert would be more recent (higher end) than the existing window.

**2. Discharge cascades and fill-in**: If an install is performed it creates cascade obligations
which must now be discharged. For each immediate child c of b:

 1. If c has a claim, it does not inherit and can be skipped.
 2. Compute c's resolution before and after the install; both follow from the outgoing and the new
    claim at b and c's branch point.
 3. If c was previously not valid (equivalently, not attached) and remains unknown under the newly
    installed claim, skip.
 4. If c was previously valid and remains valid with the relevant claims carrying the same exact
    cert before and after, nothing changes for c: skip. Sameness is full identity of the witnessing
    cert, not equality of revisions: an equal-value recompute with a different dep set yields a
    different cert, and c's rdep map holds edges only for the one it has been inheriting.
 5. If c was previously not valid (equivalently, not attached) and the newly installed claim covers
    the branchpoint of c, attempt to perform fill-in: Compute `cover_c(C)`. If it is the entire
    range `[0, head_c]`, attach K at c by only writing to the rdeps map (do not create a claim).
    Queue up the children of c for recursive treatment as per this loop. Evict c from the candidate
    list.
 6. Otherwise, give c a claim that exactly matches its previous resolution, creating the slot if
    needed; if the previous resolution was unknown, such a claim can be manufactured by giving it
    valid range `[0, 0)`, reusing the cert from the previous resolution chain when one exists (it is
    likely a better candidate than the new one) and the new cert otherwise. This may seem
    pessimistic, but not to worry: `c` will soon appear again as a candidate in its own right.

### 5.3 `commit(b, changes)`

Let `s'` be `b.seq + 1`. Per change:

- **`changed(K)`**: Mint a fresh revision `eps` for K′ and append `(s', eps)` to K′'s history at b,
  creating a slot (but no claim) for K at b if needed. Since K′ edges are not kept in the rdep map
  (Invariant 3), the BFS's first hop is handled directly: if K resolves `Valid` at `head_b`, close
  it by hand exactly as the BFS below would, and continue the walk from K; if K is `Unknown`,
  nothing valid depends on it (Invariant 2) and there is nothing to walk.
- **`changed_to(K, r)`**: If `r` equals K's current resolution at b, the assertion is a no-op.
  Otherwise append `(s', r)` to K's assertion history at b and run the BFS from K.

If every change was a no-op, the commit mints no version: `b.seq` is left alone and the current head
seq is returned. Callers that re-assert their inputs on every use rely on this — equal inputs must
yield an equal version, because the execution environment shares in-flight work between
transactions by version equality. Otherwise `b.seq` becomes `s'`.

**BFS.** Walk b's own `RdepMap` from the changed keys. For each key J reached whose resolution at
`head_b` is currently `Valid`:

- If J's resolution is via an own open claim: detach by setting `valid_until = s'`.
- If J's resolution is inherited: give J a claim at b with window `[0, s')` and the inherited
  claim's cert — this is detach applied to a copy — creating the slot if J has none at b. The
  cascade rule creates no obligations here, as the resolution of child keys will not be affected.
- Add J to `b.closed_index`.

The parts of the rdep map that are visited are discarded.

The walk stops at keys whose head resolution is not Valid-open; such keys are reached only through
extra edges in the rdeps map or repeat visits.

**Theorem (BFS completeness).** After the BFS from a set of changed
keys, every K whose open-ended resolution at `head_b` transitively
depended on a changed key has had that resolution closed (own claim
detached, or inherited claim shadowed) — Invariant 2 is restored at the
new head.

*Proof.* Let K be such a key, its resolution governed by cert C with some
dep D whose resolution changed. By Invariant 2, D was Valid-open at
`head_b` before the change; by Invariant 3, `D → K` was in b's `RdepMap`.
When the BFS reaches D and closes it, every rdep of D — K included — is
enqueued. The walk visits K; K's resolution at `head_b` is still
Valid-open at arrival (nothing else closes it), so the BFS closes it.
Induction on chain length; Invariant 3 supplies the edge at each step. ∎

The drain is justified the same way: by Invariant 2, a drained entry's
dependents were just detached, so Invariant 3 no longer requires the
edges.

Force-dirty and early cutoff compose: a force-dirtied K that recomputes to
an equal value re-finds its revision (Appendix A) and attaches under the
new ε; dependents' certificates reference `(K, revision)` — not K's ε — so
they revalidate untouched.

### 5.4 `fork(from = (b, s))`

`s` is any seq of b (`s ≤ b.seq`).

**1. Create c.** `parent = (b, s)`, `seq = 0`, empty `closed_index`; register `(c, s)` in
`b.children`.

Resolution at c is immediately sound by the inheritance criterion without any additional
work.

**2. Initialize c's rdep map.** The rdep map for c needs to be correctly initialized. In particular,
it needs `D → K` edges for every key that resolves attached at c and has deps. We begin by computing
a set of candidates for keys `K` that might be found on the rdep side of such an edge.

**Lemma (candidates).** If c holds no claims, every key that resolves attached at c and has deps
appears on the dependent side of b's rdep map or in `b.closed_index`.

*Proof.* Case on K's resolution at c. Via a claim owned by b: if open, K is attached at b, and
Invariant 3 puts its `D → K` edges in b's map — K appears on the dependent side; if closed, the
window covers s and K ∈ `b.closed_index`. Not via a claim owned by b: either `Unknown`, and there is
nothing to show, or Valid through an ancestor — but then K also resolves attached at b (rule 3
delegates at the fork point, so the resolution is constant across b's seqs), and Invariant 3, which
covers inherited attachments, again puts K's edges in b's own map. ∎

We construct our set of candidates by iterating the set of keys appearing on the RHS of the rdep map
of b or in the closed set of b and taking all those keys as candidates which resolve valid at c (or,
equivalently, at (b, s)).

As mentioned above, the valid resolution of each candidate at c is already sound; however, with the
state as it is now, all of the keys in the candidate set would resolve attached at c, while their
deps are missing from the rdeps map. That inconsistency must be corrected, but note that it cannot
be corrected in naive fashion, since even if we inserted all the deps of the candidates into the
rdeps map, the support invariant of the rdeps map may be violated.

We pause at this point to discuss one technicality: The candidate set of this map does not, as
constructed, contain all keys that resolve attached at c; it is missing exactly those which resolve
attached at c and have no deps - we will call these the trivial keys. This is acceptable
specifically because the above concern does not apply to trivial keys: the supported invariant for
them is vacuously met, as is the requirement for dependency edges to be present in the rdep map. If
not treated carefully though this detail may be a cause of bugs.

We'll correct the above issue by partitioning the set of candidates into a set of attached keys and
a set of residual keys.

The set of attached keys must, as expected, be supported in the sense of being closed downward among
the union of itself, the trivial keys, and the injected keys (whose histories ground directly).
Furthermore, we require this support to be inductive, or equivalently the attached set to be
acyclic, for the reasons given above and in the Appendix. The rdeps map for c is then initialized
from all the edges within the attached set.

The residual keys are those that are not attached; we must prevent them from resolving attached. To
do so, we install an empty claim with window `[0, 0)` for the branch c at each of the residual keys
(the cert may be taken from the previous resolution). This changes the resolution of K to invalid
and unattached at c. It does not create any cascading obligations as c cannot yet have children.

There are multiple candidate algorithms for constructing the attached (and implicitly residual) sets
which one might consider. One is trivial but imprecise: Make all candidates residual. A fully
precise algorithm can also be constructed by first collecting all deps and then topologically sorting:

```py
state[k] = Undecided for all candidates
for k in candidates:
    pending[k] = 0
    for (d, r) in pinned_cert(k).premises():                 // deps ∪ {k′}
        match resolve(d, (b, s)):
            Valid(r2) if r2 == r and d is a candidate:  pending[k] += 1; waiters[d].push(k)
            Valid(r2) if r2 == r:                       pass    // ground: injected, trivial, or attached
            otherwise:                                  state[k] = Residual; break
    if state[k] == Undecided && pending[k] == 0: ready.push(k)

while k = ready.pop():
    state[k] = Attached                                       // emit k's pinned-cert edges into c's map here
    for w in waiters[k]:
        if state[w] == Undecided && --pending[w] == 0: ready.push(w)

for k in candidates:
    if state[k] == Undecided: state[k] = Residual
```

There is an in-between option, the copy algorithm: We can take as the attached set those candidates
whose resolution at (b, s) is through an open window or through inheritance, and as the residual set
those resolving through a closed window in a claim owned by b. (Partitioning by which enumeration a
candidate came from would be wrong: a closed-covering key may also appear on the rdep-map side
through extra edges; the resolution decides.) The correctness of this partition can be seen from the
window nesting clause of Invariant 2 (details omitted here). The implementation of this algorithm
can be made particularly efficient in combination with the fact that we allow extra edges in the
rdeps map, since that means c's rdeps map in this algorithm can be constructed as an exact copy of
b's.

Finally, these approaches may be combined into the semi-precise algorithm: first apply the copy
algorithm's map construction, but instead of treating all closed-window resolvers as residual, run
the precise algorithm's worklist with exactly those keys as the candidate set — the match's ground
arm then covers everything resolving through open windows, inheritance, and histories. This is the
algorithm we implement.

### 5.5 `delete(b)`

Deletion forgets b's claims, edges and histories, so that the values only b referenced can be
released (§2.3). Since a child resolves through its parent (§4.4), b's children are first given
what they resolved through b, and then handed to b's parent, or made roots if b has none.
Resolution at every version of every surviving branch is thereby unchanged, so S holds without
further argument. For a child c forked at (b, s):

- Every key K with a claim at b and none at c gets a claim at c with the same certificate. If b's
  window covered s, c resolved `Valid` through it at every seq of c (delegation is total across
  c's timeline, §4.4), so the new window is open; c was attached, and by Invariant 3 c's own
  `RdepMap` already holds the certificate's edges, so Invariants 2 and 3 carry over as they stood.
  Otherwise c resolved `Unknown` with that certificate as its candidate, and an empty window keeps
  it so.
- For every history at b, of an injected key or of an untracked input, the entry in force at s, if
  there is one, is copied to c at c's first seq, unless c's own history already starts there. c's
  history then answers, with the same revision, for the seqs c used to resolve through b.

A key c has a claim of, and a history c owned from its first seq on, delegated nothing to b and
need nothing. Descendants of c resolve through c and are unaffected.

The price is duplication. A claim that b's children shared through inheritance becomes one copy
per child, and a write that would have reached them through one installation at b is instead
cascaded into each child's own claim. What resolves where does not change.

## 6. Completeness

FIXME(JakobDegen): Give some useful completeness results.

## Appendix A: The value layer

The API above handles keys and revisions as opaque integers and leaves to its user the task of
actually mapping key and revision numbers to appropriate domain specific objects. For keys, dice
handles this in a
maximally naive way; there's a global append-only table that stores the mapping. For values,
however, something a bit smarter is needed, as values often represent large, expensive objects and
need to be discarded and garbage collected in a timely manner. This appendix describes the extension
to the above API that solves that problem and some related ones.

The core of the approach is as follows: On each node, next to the table of slots we store a revision
table mapping revisions to the value associated with the revision. The lifetime of the entries in
the revision table is by reference count from the slots. In other words, the revision table contains
entries for exactly those revisions which are named by the cert on some slot of that key. Whenever
certs get removed or overwritten, the revision table needs to be checked and an entry may need to be
discarded.

FIXME(JakobDegen): This is all written terribly, the algorithm above is designed specifically with
this retention behavior in mind, putting it in the appendix makes no sense, figure out how to do
this right.

`lookup` is changed to return the value in addition to the revision. Note that the set of values we
retain is the minimum needed to always make that possible; retaining fewer would require pessimizing
some lookups.

`write` is changed to have two variants, a simple one accepting a fully prepared cert as described
above, and a fused one that accepts a cert-like object holding a value in place of a revision. On a
fused write, the core state needs to perform an interning step to compute a revision for the value.
It does so as follows:

 1. First, check the `(ε, dep revisions)` provided to the fused write and compare them against any
    stored certs to see if they have an identical trace. If so, stop and replace the entire fused
    write operation with a simple write of that cert, discarding the new value. The replacement is
    sound with no assumptions — that cert is already written, and re-issuing it adds nothing. The
    discard is a deliberate policy rather than a checked identification: the value first interned
    for a trace is canonical for its revision, and a nondeterministic compute that produces a
    different value for an identical trace has that value dropped in favor of the stored one. For
    keys that provide no comparison function this is also the only dedup step.
 2. Second, we allow values to provide a comparison function to check if two are identical. Use this
    comparison function to attempt to check if any existing values in the revision table is
    identical to the provided one[^1]; if so, discard the new value and proceed with a cert
    constructed with the discovered revision.
 3. Otherwise, mint a new revision for the value and use that.

Finally, `write` is then adjusted to return the revision as well as the value associated with that
revision in the revision table if any and the provided value otherwise. In the second and third
cases the value returned is identical in the sense of value comparison to the value just provided,
though in the second case it may be a different instance; returning the stored instance prevents
retaining two copies. In the first case the value returned is whatever is interned for the
discovered revision, which coincides with the value just provided exactly to the extent that
computes of the key behave deterministically.

[^1]: Depending on how expensive these comparisons are, it may be appropriate to pick just one most
    likely candidate instead of checking all.

## Appendix B: Series-parallel deps

§2.1 reads a certificate's deps as an unordered set; the recorded object is
richer. Dice's execution environment records the deps of a compute as a
series-parallel graph — sequential composition where one read's result
determined the next read, parallel composition where reads were requested
together — with per-edge revisions stored parallel to the structure.

The structure exists for the execution environment, not the core:

- A CheckDeps walk (§5.1) re-verifies deps *in series order*, because an
  early premise's failure makes later premises moot — under a changed
  early dep, a recompute may read entirely different keys — and
  re-verifies parallel groups concurrently. The order is a scheduling
  matter only; the success criterion is §4.5's coverage, which is
  order-free.
- The core carries the structure opaquely inside the pinned certificate
  and never branches on it; every core-side use flattens to the edge
  multiset.
- The one place shape is observable is Appendix A's identity step: its
  per-edge compare includes the structure, so two traces equal as sets but
  recorded with different shapes count as distinct. That over-distinguishes,
  which is always sound: a missed identification only forfeits reuse, never
  makes an answer wrong.

## Appendix C: Cycles and the choice of induction

FIXME(JakobDegen): Section still LLM slop but it's fine.

§3.2 takes the justification judgment to be the *least* relation closed
under its rules. W is just a set, so nothing stops certificates from
referencing each other cyclically, and the least reading settles what a
cycle means: by itself, nothing. In

```
W = { (k, r, {(j, q)}, ε_k),   (j, q, {(k, r)}, ε_j) }
```

no derivation of either fact exists anywhere — each needs the other first —
so a structure honoring §3.3's theorem answers `Unknown` for both, unless
some other certificate or assertion grounds one of them, in which case both
follow, well-foundedly.

The objection to the greatest relation (the coinductive reading, under
which the pair would justify itself) is *not* about unbounded validity. A
certificate with no deps, on a key never asserted or dirtied, justifies its
revision everywhere forever — inductively, and rightly: that compute read
nothing, so nothing can invalidate it. The objection is to *unfounded*
validity, and it takes an honest history to see the difference. Let j's
compute read X and then, when X = x2, also read k; let k's compute read Z
and then, when Z = z2, also read j:

1. At (X, Z) = (x1, z2): compute j — trace `(j, q, {(X, x1)})`; compute k —
   trace `(k, r, {(Z, z2), (j, q)})`.
2. Assert Z ↦ z3: k recomputes and re-finds r — trace `(k, r, {(Z, z3)})`
   (early cutoff).
3. Assert X ↦ x2: j recomputes, reads k — alive through the step-2 trace —
   and re-finds q: trace `(j, q, {(X, x2), (k, r)})`.
4. Assert Z ↦ z2: the step-2 trace dies, and W now holds a
   mutually-referencing pair whose other side conditions all hold: k@r via
   step 1's trace needs j@q; j@q via step 3's trace needs k@r.

Every write here was honest, deterministic, and made only after its deps
had been served — early cutoff is what weaves honest traces into such
islands. Inductively, both keys are `Unknown` at step 4; the execution
environment recomputes, j reads k reads j, and its cycle detection reports
the true shape of the step-4 world: under (x2, z2), the computation
genuinely cycles.

Under the coinductive reading the pair would stay "valid" — and that,
precisely and completely, is the objection: **for an execution environment
that defines cycles as errors, the coinductive reading masks exactly those
errors** behind values carried over from worlds that no longer exist. Dice
is such an environment, so this document takes the least relation; §3.4's
from-scratch corollary confirms the fit, since its induction gives every
justified fact a terminating recomputation as counterpart — the thing a
coinductive-only fact lacks.

Nothing deeper is wrong with coinduction. Look at what the island holds at
step 4: given k = r's value, j's compute emits q; given j = q's value, k's
compute emits r. The pair is a *fixpoint* of the step-4 recursive system,
witnessed by honest traces. An execution environment that assigns cyclic
computations fixpoint semantics — Salsa's cycle recovery is the nearby
example — could soundly want the greatest relation: "valid" would then mean
"a trace-witnessed solution of the current system", and the island is
exactly that. A different implementation of §2's interface could adopt that
reading. This implementation could not: §§4–6 are built on well-founded
support — attach's precondition, the commit BFS, and the fork support
filter all assume a claim's premises resolve *before* the claim — so
coinductive islands are not establishable here, quite apart from being
unwanted.

No acyclicity precondition appears anywhere in this document, for soundness
or for termination: the state's own operations never recurse through W
(§4.5's cover computation intersects already-materialized windows; `lookup`
walks one node's slot chain), so a cycle in W is inert until an execution
environment recomputation actually runs into it — in the environment, where
cycle handling lives.
