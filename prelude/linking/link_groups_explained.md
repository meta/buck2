# Link Groups Explained

This document explains in detail how Buck2's link groups system works — all the
options, how they interact, and what the resulting binaries look like. It's meant
to be read top-to-bottom, with each section building on the previous.

## What Link Groups Do

Link groups partition a binary's native dependencies into separate shared
libraries. Without link groups, a statically-linked binary is one giant
executable. With link groups, selected subsets of the dependency graph are evicted
into `.so` files, reducing the main binary size.

```
Without link groups:              With link groups:

┌─────────────────────┐           ┌───────────────┐
│      binary         │           │    binary     │
│                     │           │   (smaller)   │
│  lib_a              │           │  lib_ungrouped│
│  lib_b              │           └──────┬────────┘
│  lib_c              │                  │ DT_NEEDED
│  lib_d              │           ┌──────┴─────────────┐
│  lib_ungrouped      │           │                    │
│  ...                │      ┌────▼─────┐        ┌─────▼────┐
└─────────────────────┘      │libgroup_a│        │libgroup_b│
                             │  lib_a   │        │  lib_c   │
                             │  lib_b   │        │  lib_d   │
                             └──────────┘        └──────────┘
```

## The Pipeline

Link groups processing happens in a fixed sequence of phases:

```
┌──────────────────────────────────────────────────────────────┐
│ Phase 0: Parse link_group_map attribute                      │
│          → list[Group] with mappings, attrs, traversals      │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 1: Compute mappings (target → group)                   │
│          For each mapping:                                    │
│            1a. Discovery: find matching targets               │
│                (_find_targets_in_mapping)                     │
│            1b. Assignment: walk subtrees, assign to group     │
│                (_update_target_to_group_mapping)              │
│          → dict[Label, str]  (first-assignment-wins)         │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 2: Generate link group specs                           │
│          → one LinkGroupLibSpec per shared library to create │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 3: Identify public nodes (cross-boundary targets)      │
│          → set[Label] needing --whole-archive                │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 4: Find roots per group                                │
│          → dict[group_name, set[Label]]                      │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 5: Collect linkables per group (DFS from roots)        │
│          → dict[group_name, list[Label]]                     │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 6: Per-target decision logic                           │
│          For each group + binary: decide how each target     │
│          appears on the link line (static, shared, or ref)   │
└───────────────────────┬──────────────────────────────────────┘
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 7: Link shared libraries + assemble symlink tree       │
│          → final binary + .so files + runtime symlinks       │
└──────────────────────────────────────────────────────────────┘
```

### Phase details

**Phase 0: Parse `link_group_map`** (`parse_groups_definitions` in `groups.bzl`)

Parses the `link_group_map` attribute into a `list[Group]`. Each group has a
name, list of `GroupMapping`s (root, traversal, filter, linkage), and optional
`GroupAttrs` (discard_group, linker_flags, enable_distributed_thinlto, etc.).
Deduplicates groups by name (first definition wins). Filters out groups that
don't meet `enable_if_node_count_exceeds` threshold.

**Phase 1: Compute mappings** (`compute_mappings` in `groups.bzl`)

Builds the `dict[Label, str]` mapping from target to group name. Described in
detail under "How Phase 1 works" below. This is the only phase that uses
traversal types and filters.

**Phase 2: Generate link group specs** (caller-side, e.g. `cxx_executable.bzl`)

Creates one `LinkGroupLibSpec` per shared library to be produced. Each spec
has: an output name (the .so filename), a `Group` reference, an optional
`LinkableRootInfo` (providing private linker flags, version scripts, etc.),
and whether it's a real shared lib (with SONAME) or something else like a
Python extension. The specs come from `auto_link_group_specs` on the rule,
which are typically generated by the fbcode macro based on the link group map.

**Phase 3: Identify public nodes** (`get_public_link_group_nodes` in
`link_groups.bzl`)

Finds all targets that are depended on across a link group boundary — i.e., a
target in group A has a dep on a target in group B. These "public" targets
need `--whole-archive` to ensure their symbols are exported from their group's
.so, since the linker can't see cross-.so usage at static link time.

Also transitively includes `exported_deps` of public nodes (since exported
deps form part of the public API surface).

**Phase 4: Find roots per group** (`_find_all_relevant_roots` in
`link_groups.bzl`)

Determines the DFS starting points for each group in Phase 5. Sources of roots:
- Specs with explicit `root`: use `root.deps`
- Specs without root: extract roots from the group's mappings, plus a full
  graph DFS to discover additional roots for groups with rootless mappings or
  when using shared link strategy
- `MATCH_ALL` targets are added to every group's root set
- `MATCH_DIRECT_DEPS` targets are added to a group if any member has a direct
  dep on them
- Force-static targets (`preferred_linkage = "static"`) are traversed through
  unconditionally during root discovery (they always return `all_deps`)

**Phase 5: Collect linkables per group** (`_collect_all_linkables` →
`collect_linkables` in `link_groups.bzl`)

For each group, does a DFS from that group's roots to discover all reachable
linkable targets. The DFS uses `get_deps_for_link` to decide which edges to
follow: for statically-linked nodes, follow all deps; for shared-linked nodes,
follow only `exported_deps` (since the shared lib satisfies its own internal
deps).

The result is a `dict[group_name, list[Label]]` — the ordered list of targets
that are candidates for each group's link line. This is the most expensive
phase (see Performance Analysis in `link_groups.md`), because it does a
separate full DFS per group with heavy overlap.

**Phase 6: Per-target decision logic** (`get_filtered_labels_to_links_map` in
`link_groups.bzl`)

Called once per group (inside `_create_link_group`) plus once for the binary
itself. For each target in the group's linkable list from Phase 5, decides how
it appears on the link line:

- **Same group** (or `MATCH_ALL` / `MATCH_DIRECT_DEPS`): statically linked
  into this group
- **Different group**: reference that group's .so (added once via `add_link_group`)
- **Ungrouped / `NO_MATCH`**: statically linked into the binary (when linking
  the binary's link line)
- **`preferred_linkage = "shared"`**: linked as a pre-existing shared lib
- **Force-static**: always statically linked, regardless of group assignment
- **`discard_group`**: skipped entirely

Also applies `--whole-archive` to public nodes (from Phase 3) and handles
transformation specs (link info filtering/transformation per target).

**Phase 7: Link shared libraries + assemble symlink tree**
(`_create_link_group` → `cxx_link_shared_library`, then caller assembles
symlink tree)

For each group spec: produces the actual `cxx_link_shared_library` action with
the link inputs from Phase 6, generates stub libraries, extracts symbol files
(global/undefined) for version scripts, and creates shared library interfaces
(on GNU). The caller then assembles the runtime symlink tree mapping sonames
to the built .so files.

## The `link_group_map` Attribute

A `link_group_map` is a list of group definitions. Each definition is a tuple:

```python
link_group_map = [
    # (group_name, mappings, optional_attrs)
    ("group_a", [
        (":lib_a", "tree", None, None),     # root, traversal, filter, linkage
        (":lib_b", "node", None, None),
    ]),
    ("group_b", [
        (":lib_c", "tree", "label:vendor/.*", "shared"),
    ], {"discard_group": False}),
]
```

Each mapping tuple has four fields:

| Field | Type | Description |
|---|---|---|
| root | Label, list[Label], or None | Starting point(s) for target discovery |
| traversal | str | How to expand from the root: `tree`, `node`, `subfolders`, `intersect_any_roots` |
| filter | str, Dependency, or None | Additional predicate on discovered targets |
| linkage | str or None | Override preferred linkage: `"static"`, `"shared"`, `"any"`, or None |

## Traversal Types

The traversal type controls which targets are assigned to a group starting from
the root. Understanding this requires understanding how Phase 1 works internally.

### How Phase 1 works: discovery then assignment

Phase 1 (`compute_mappings`) processes mappings in two passes:

**Pass 1 — Discovery** (`_find_targets_in_mapping`): For each mapping, find
"seed" targets. This is where traversal type and filters are evaluated.
Discovery does NOT assign anything — it just collects candidate `(target,
mapping)` pairs into a per-group list.

**Pass 2 — Assignment** (`_update_target_to_group_mapping`): Iterate groups in
definition order. For each group's discovered targets:
- **tree**: DFS from the seed target, claiming every unassigned target in the
  subtree for this group. **No filter is applied** — everything reachable gets
  claimed.
- **node / subfolders / intersect_any_roots**: Just assign the seed target
  itself. No DFS, no subtree claiming.

All discoveries complete before any assignment begins. Assignment processes
groups in definition order, which is what makes first-assignment-wins
deterministic.

```
Example: discovery finds candidates, assignment claims them

link_group_map = [
    ("a", [(":A", "tree", None, None)]),
    ("c", [(":C", "node", None, None)]),
]

    A ──► B ──► C

Discovery pass:
  group "a": seed = [A]    (tree root, no filter → just return roots)
  group "c": seed = [C]    (node root)

Assignment pass (in group order):
  group "a" first: tree DFS from A
    A → assign to "a"
    B → assign to "a"
    C → assign to "a"     ◄── claimed by tree DFS before group "c" runs!
  group "c" second:
    C → already assigned to "a" → no-op

Result: C is in group "a", NOT "c".
```

If the groups were listed in the opposite order, "c" would be assigned first,
and C would go to "c". When "a"'s tree DFS later reaches C, it would find C
already assigned but would still continue through C's children (because C was
assigned via `node`, not `tree` — see below).

### `tree` — Root + all transitive deps

During assignment, a tree DFS from the seed target claims every unassigned
target in the subtree. No filter is applied — the filter was only used during
discovery to find the seed.

```
link_group_map = [
    ("group_a", [(":lib_a", "tree", None, None)]),
]

Dependency graph:              Group assignment:

    binary                        binary (ungrouped)
    ├── lib_a ◄── root            ├── lib_a ──► group_a
    │   ├── lib_x                 │   ├── lib_x ──► group_a
    │   └── lib_y                 │   └── lib_y ──► group_a
    │       └── lib_z             │       └── lib_z ──► group_a
    └── lib_b                     └── lib_b (ungrouped)
```

If a target is already assigned to another group, the tree DFS does NOT
reassign it, but its behavior at that point depends on HOW the target was
previously assigned:

- **Previously assigned via tree**: the DFS **stops** — that target's entire
  subtree was already claimed, so there's nothing left to do.
- **Previously assigned via node/subfolders/intersect_any_roots**: the DFS
  **continues through** the target into its children, because node assignment
  didn't claim the subtree.

```
link_group_map = [
    ("group_a", [(":lib_a", "node", None, None)]),   # node: just lib_a
    ("group_b", [(":lib_b", "tree", None, None)]),    # tree: lib_b + deps
]

    binary
    ├── lib_a ──► group_a (node)
    │   └── lib_shared
    └── lib_b ──► group_b (tree root)
        └── lib_a ──► already group_a, but tree continues through...
            └── lib_shared ──► group_b (claimed by tree through lib_a)
```

### `node` — Root only

Only the seed target itself is assigned to the group. No DFS, no subtree
claiming. Its deps are NOT assigned (unless another mapping assigns them).

```
link_group_map = [
    ("group_a", [(":lib_a", "node", None, None)]),
]

Dependency graph:              Group assignment:

    binary                        binary (ungrouped)
    ├── lib_a ◄── root            ├── lib_a ──► group_a
    │   ├── lib_x                 │   ├── lib_x (ungrouped)
    │   └── lib_y                 │   └── lib_y (ungrouped)
    └── lib_b                     └── lib_b (ungrouped)
```

### `subfolders` — Per-directory implicit groups

Like `node`, but instead of using the group name directly, generates implicit
group names based on each target's package path.

```
link_group_map = [
    ("evict", [(None, "subfolders", "pattern:foo//...", None)]),
]

Targets:                       Implicit group names:
  foo//bar:lib1     ──►    evict_foo_bar
  foo//bar:lib2     ──►    evict_foo_bar
  foo//baz:lib3     ──►    evict_foo_baz
```

Each unique package path produces a separate shared library. Name format:
`{group_name}_{package/path}` with `/` replaced by `_`, truncated to 246 chars.

### `intersect_any_roots` — Common deps of 2+ roots

Requires at least 2 roots. Finds targets reachable from **more than one** root.

```
link_group_map = [
    ("common", [([":root_a", ":root_b"], "intersect_any_roots", None, None)]),
]

    binary
    ├── root_a
    │   ├── lib_only_a       ← only reachable from root_a, NOT assigned
    │   ├── lib_common       ← reachable from BOTH roots → group "common"
    │   └── lib_shared_dep   ← reachable from BOTH roots → group "common"
    └── root_b
        ├── lib_only_b       ← only reachable from root_b, NOT assigned
        ├── lib_common       ← reachable from BOTH roots → group "common"
        └── lib_shared_dep   ← reachable from BOTH roots → group "common"
```

## Filters

Filters restrict which targets within a traversal are assigned to the group.
Multiple filters on a mapping use AND semantics (all must match).

### Filter types

| Prefix                         | Matches against                | Example                      |
|--------------------------------|--------------------------------|------------------------------|
| `label:REGEX`                  | Target's labels/tags           | `label:vendor`               |
| `tag:REGEX`                    | Same as `label:`               | `tag:third_party`            |
| `target_regex:REGEX`           | Target's `raw_target()` string | `target_regex:caffe2/.*`     |
| `pattern:BUILD_TARGET_PATTERN` | Buck target pattern            | `pattern:fbcode//caffe2/...` |

### Filter interaction with tree traversal

Filters only apply during discovery (pass 1). During tree assignment (pass 2),
the DFS claims all unassigned targets unconditionally — no filter is checked.

During discovery, the DFS visits nodes and checks the filter at each node:

- **Filter matches**: The node is added to the group's matching targets. For
  tree traversal, the DFS **stops descending** from that node (an optimization
  — the subtree will be collected during assignment instead).
- **Filter doesn't match**: The node is **not** added, but the DFS **continues
  descending** into children by default, looking for matches deeper down. This
  means filters can "reach through" non-matching intermediate nodes.

```
link_group_map = [
    ("group_a", [(":root", "tree", "pattern:fbcode//vendor/...", None)]),
]

    root                           Traversal during _find_targets_in_mapping:
    ├── lib_x (no match)           DFS continues ─►
    │   └── vendor/lib_v (match!)  DFS STOPS (subtree handled during assignment)
    │       └── vendor/lib_w       Not visited during discovery, but...
    └── lib_y (no match)           DFS continues ─►

Assignment (step 1b):
  vendor/lib_v ──► group_a
  vendor/lib_w ──► group_a  (discovered via tree assignment from lib_v)
```

### `stop_at_first_non_match`

By default, when a filter doesn't match a node, the DFS **skips that node but
continues descending into its children** — it "reaches through" non-matching
nodes to find matches deeper in the graph. With `stop_at_first_non_match = True`,
the DFS **halts at the non-matching node** and does not visit any of its children.

The tables below describe the **discovery phase** behavior — whether the DFS
descends into a node's children. This interacts with the traversal type because
tree and non-tree traversals have opposite default descend behavior on match:

**For tree traversal** (match stops descending, non-match continues by default):

| Situation                                     | Added? | Descend? |
|-----------------------------------------------|--------|----------|
| Filter matches                                | Yes    | No (subtree handled in step 1b) |
| Filter fails, `stop_at_first_non_match=False` | No     | **Yes** — skip node, keep looking deeper |
| Filter fails, `stop_at_first_non_match=True`  | No     | **No** — halt here |

With tree traversal, matches already stop descending (step 1b handles
the subtree). So `stop_at_first_non_match=True` means *both* match and non-match
stop — the DFS never goes past the first level. Only the roots themselves can
match. This combination is effectively degenerate and not useful in practice.

**For non-tree traversals** like `intersect_any_roots` (match **continues**
descending):

| Situation                                     | Added? | Descend? |
|-----------------------------------------------|--------|----------|
| Filter matches                                | Yes    | **Yes** — keep going |
| Filter fails, `stop_at_first_non_match=False` | No     | **Yes** — skip node, keep looking deeper |
| Filter fails, `stop_at_first_non_match=True`  | No     | **No** — halt here |

This is where `stop_at_first_non_match` is meaningful. The DFS keeps descending
as long as nodes match, and stops at the boundary where they don't — defining
a **contiguous matching region** from the root:

```
intersect_any_roots + filter + stop_at_first_non_match=True:

    root (match) ──► add, continue
    ├── dep_a (match) ──► add, continue
    │   ├── dep_a1 (match) ──► add, continue
    │   └── dep_a2 (no match) ──► HALT, don't look deeper
    └── dep_b (no match) ──► HALT, don't look deeper
        └── dep_b1 (match) ──► never visited!
```

Note: None of the built-in filter types (`label:`, `tag:`, `pattern:`,
`target_regex:`) set this flag. It exists for custom filter targets — rules
that provide a `GroupFilterInfo` provider directly as a dependency in the
`link_group_map`, rather than using a string filter.

## Special Groups

Three pseudo-group names have special semantics. They don't produce shared
libraries — they control how targets are distributed.

### `MATCH_ALL` — Include in every group

Targets assigned to `MATCH_ALL` are **statically linked into every link group**.
This means their symbols are duplicated across all shared libraries.

```
link_group_map = [
    ("MATCH_ALL", [(":lib_common", "tree", None, None)]),
    ("group_a",   [(":lib_a", "tree", None, None)]),
    ("group_b",   [(":lib_b", "tree", None, None)]),
]

Result:

┌───────────────┐
│    binary     │
│  (ungrouped)  │
└──────┬────────┘
       │
  ┌────┴─────────────┐
  │                  │
┌─▼──────────┐   ┌───▼─────────┐
│libgroup_a  │   │ libgroup_b  │
│  lib_a     │   │  lib_b      │
│  lib_common│   │  lib_common │  ◄── duplicated in both!
└────────────┘   └─────────────┘
```

Note: `MATCH_ALL` duplicates symbols across groups, which increases binary
size and can cause subtle ODR (One Definition Rule) issues. In most cases,
it's better to place shared utilities in their own named group or leave them
ungrouped in the main binary.

### `NO_MATCH` — Force into the main binary

Targets assigned to `NO_MATCH` are **never** placed in any link group. They are
linked directly into the main binary, regardless of other mappings.

```
link_group_map = [
    ("NO_MATCH",  [(":lib_critical", "node", None, None)]),
    ("group_a",   [(":lib_a", "tree", None, None)]),
]

    binary
    ├── lib_a ──► group_a (tree root)
    │   └── lib_critical ──► NO_MATCH (stays in binary, NOT in group_a)
    └── lib_critical ──► NO_MATCH

Result:

┌──────────────────┐
│      binary      │
│  lib_critical    │  ◄── forced into binary
└──────┬───────────┘
       │
  ┌────▼────────┐
  │ libgroup_a  │
  │  lib_a      │    ◄── lib_critical NOT here despite being lib_a's dep
  └─────────────┘
```

### `MATCH_DIRECT_DEPS` — Follow reverse dependencies

Targets assigned to `MATCH_DIRECT_DEPS` are included in **any group that has a
target with a direct dependency on them**. Like `MATCH_ALL`, this can cause
duplication.

```
link_group_map = [
    ("MATCH_DIRECT_DEPS", [(":lib_vendor", "node", None, None)]),
    ("group_a", [(":lib_a", "node", None, None)]),
    ("group_b", [(":lib_b", "node", None, None)]),
]

    binary
    ├── lib_a ──► group_a
    │   └── lib_vendor  ◄── direct dep of lib_a (group_a) → included in group_a
    └── lib_b ──► group_b
                            lib_b does NOT depend on lib_vendor → NOT in group_b

Result:

┌────────────┐
│   binary   │
└─────┬──────┘
      │
 ┌────┴───────────┐
 │                │
┌▼───────────┐  ┌─▼──────────┐
│libgroup_a  │  │ libgroup_b │
│  lib_a     │  │  lib_b     │
│  lib_vendor│  │            │  ◄── lib_vendor NOT here
└────────────┘  └────────────┘
```

## First-Assignment-Wins Ordering

When multiple groups could claim a target, the group whose assignment runs
**first** wins. Assignment processes groups in the order they appear in
`link_group_map` (see "How Phase 1 works" above). This is a critical ordering
dependency.

```
link_group_map = [
    ("group_first",  [(":lib_a", "tree", None, None)]),   # listed first
    ("group_second", [(":lib_b", "tree", None, None)]),   # listed second
]

    binary
    ├── lib_a ──► group_first (root)
    │   └── lib_shared_dep ──► group_first  (claimed by tree, first wins!)
    └── lib_b ──► group_second (root)
        └── lib_shared_dep ──► already group_first, stays there

Result:

┌─────────────────┐   ┌───────────────────┐
│ libgroup_first  │   │ libgroup_second   │
│  lib_a          │   │  lib_b            │
│  lib_shared_dep │   │                   │  ◄── lib_shared_dep NOT here
└─────────────────┘   └───────────────────┘
```

## Preferred Linkage Override

Each mapping can override the preferred linkage of matched targets:

```python
("group_a", [(":lib", "tree", None, "shared")]),   # force shared linkage
("group_b", [(":lib", "tree", None, "static")]),   # force static linkage
("group_c", [(":lib", "tree", None, None)]),       # use target's own preference
```

When `preferred_linkage = "shared"`, the target is linked as a pre-existing
shared library rather than being statically linked into the group. The group
just references it.

## Force-Static Libraries

Targets with `preferred_linkage = "static"` (and
`ignore_force_static_follows_dependents = False`) get special treatment:

1. **They follow their dependents**: a force-static lib is statically linked
   into **whatever group or binary depends on it**, regardless of group
   assignment. This applies to both `deps` and `exported_deps` edges — a
   force-static target reached through `exported_deps` is still pulled into the
   depending group.

2. **They can be duplicated**: if multiple groups depend on a force-static lib,
   each group gets its own static copy.

3. **Root discovery sees through them**: normally, root discovery stops at
   shared-linkage boundaries. But force-static nodes always use `all_deps`
   for traversal, so the root discovery DFS continues through them to find
   additional roots on the other side.

```
link_group_map = [
    ("group_a", [(":lib_a", "node", None, None)]),
]

    binary
    └── lib_a ──► group_a (node)
        └── lib_static (preferred_linkage = "static")

Without force-static:
  lib_static is ungrouped → linked into binary

With force-static (default):
  lib_static follows its dependent (lib_a) → linked into group_a
```

## Public Nodes and `--whole-archive`

When a target crosses a link group boundary (depended on by a target in a
different group), it's marked as a **public node**. Public nodes are linked with
`--whole-archive` to ensure all their symbols are available.

```
    group_a contains: lib_a, lib_shared
    group_b contains: lib_b

    lib_b (group_b) ──depends on──► lib_shared (group_a)
                                         ▲
                                    public node!
                                    linked with --whole-archive

Why: Without --whole-archive, the linker might only pull in the object
files from lib_shared that resolve symbols within group_a. But group_b
also needs symbols from lib_shared, and those might not be included.
```

Public nodes are also expanded transitively through `exported_deps` (since
exported deps are part of the public API surface), except for special groups
(`MATCH_ALL`, `NO_MATCH`, `MATCH_DIRECT_DEPS`).

## The `discard_group` Attribute

A group with `discard_group = True` drops all its targets entirely — no shared
library is created, and the targets are removed from the link line.

```python
link_group_map = [
    ("unused", [(":lib_unused", "tree", None, None)], {"discard_group": True}),
    ("group_a", [(":lib_a", "tree", None, None)]),
]

    binary
    ├── lib_a ──► group_a
    │   └── lib_unused ──► "unused" (discarded!)
    └── lib_unused ──► "unused" (discarded!)

Result: lib_unused does not appear anywhere — not in the binary,
not in any shared library. It is completely eliminated.
```

Use case: removing known-unused dependencies from the build to reduce binary
size.

## Group Attributes Reference

| Attribute                          | Default | Description |
|------------------------------------|---------|---|
| `discard_group`                    | `False` | Drop all group targets entirely |
| `enable_if_node_count_exceeds`     | `None`  | Only enable group if binary has more nodes than threshold |
| `enable_distributed_thinlto`       | `False` | Use distributed ThinLTO for the group's .so |
| `linker_flags`                     | `[]`    | Extra flags when linking the group's .so |
| `linker_script`                    | `None`  | Linker script for the group's .so |
| `exported_linker_flags`            | `[]`    | Flags added to dependents linking against the group's .so |
| `no_as_needed`                     | `False` | Wrap with `--no-as-needed` (for runtime-only deps) |
| `requires_root_node_exists`        | `True`  | Fail if root targets don't exist in the graph |
| `prohibit_file_duplicates`         | `False` | Error if duplicate source files appear in public nodes |
| `link_execution_preference`        | `None`  | Control link execution location (local/remote) |

## The Per-Target Decision Logic

This is the core of link groups: for each target in a group's linkable set, the
system decides how it appears on the link line. The decision depends on the
target's group assignment, preferred linkage, and the link strategy.

### Decision tree (`static`/`static_pic` link strategy)

```
For each target in linkables:
│
├── preferred_linkage == shared?
│   ├── YES: target's group is discarded? → SKIP (drop entirely)
│   ├── YES: target is a link group root in a DIFFERENT group?
│   │        → REFERENCE that group's .so
│   └── YES: otherwise → LINK as shared lib directly
│
├── is force-static? (static linkage + follows dependents)
│   ├── YES: group is discarded? → SKIP
│   └── YES: → STATIC LINK into current group/binary
│
├── no group assigned AND building the binary?
│   └── YES: → STATIC LINK into binary (ungrouped target)
│
├── assigned to NO_MATCH AND building the binary?
│   └── YES: → STATIC LINK into binary
│
├── matches current group? (same group, MATCH_ALL, or MATCH_DIRECT_DEPS rdep)
│   └── YES: → STATIC LINK into this group
│
└── assigned to a DIFFERENT group?
    └── YES: → REFERENCE that group's .so
```

### How groups reference each other

When a target belongs to `group_b` but is encountered while building `group_a`'s
link line, the system doesn't link the target — it adds a reference to
`group_b`s shared library instead. This creates the `DT_NEEDED` relationship.

```
Building group_a's link line:

  lib_a (group_a)  → static link (archive)
  lib_x (group_a)  → static link (archive)
  lib_y (group_b)  → REFERENCE libgroup_b.so  ← not the target itself!
  lib_z (group_a)  → static link (archive)

Result: libgroup_a.so has DT_NEEDED libgroup_b.so
```

## Stub Libraries

Before any groups are linked, the system creates **empty stub shared libraries**
for each group. These serve as placeholders during linking — when `group_a`
references `group_b`, it links against the stub for `group_b`. After all groups are
linked with real content, the stubs are replaced.

```
Step 1: Create stubs
  libgroup_a.so (empty)
  libgroup_b.so (empty)

Step 2: Link group_a → references stub libgroup_b.so
Step 3: Link group_b → references stub libgroup_a.so

Each group uses --ignore-undefined-symbols because the stubs
don't have real symbols yet.
```

## Symbol Handling

After all link groups are created, the system generates linker flags for the
main binary:

1. **Undefined symbols** (`-u symbol`): Forces the main binary to pull in
   symbols that link groups need but that might otherwise be dead-stripped.

2. **Dynamic list** (`--dynamic-list`): Exports symbols from the main binary
   to the dynamic symbol table so link group `.so` files can find them at
   runtime.

## Runtime Symlink Tree

At runtime, the dynamic linker needs to find all shared libraries. The symlink
tree handles the case where a target was originally a shared library but got
merged into a link group:

```
Original shared lib:      After link groups:

  lib_c.so (standalone)     lib_c.so → symlink to libgroup_a.so

If binary has DT_NEEDED lib_c.so, the symlink resolves
to the link group that now contains lib_c's code.
```

## Common Patterns

### Pattern: Evict large dependencies

```python
link_group_map = [
    ("caffe2", [(None, "tree", "pattern:fbcode//caffe2/...", None)]),
    ("velox",  [(None, "tree", "pattern:fbcode//velox/...", None)]),
]
```

This evicts all caffe2 and velox targets into separate shared libraries,
reducing the main binary size. The `None` root + pattern filter means: find all
targets matching the pattern in the full graph.

### Pattern: Per-directory splitting

```python
link_group_map = [
    ("libs", [(None, "subfolders", "pattern:fbcode//large_dep/...", None)]),
]
```

Automatically creates one shared library per directory under `large_dep/`,
avoiding the need to enumerate every directory.

### Pattern: Common utilities shared across groups

```python
link_group_map = [
    ("MATCH_ALL", [(":lib_common", "tree", None, None)]),
    ("group_a",   [(":lib_a", "tree", None, None)]),
    ("group_b",   [(":lib_b", "tree", None, None)]),
]
```

`lib_common` (and its deps) are statically linked into both `group_a` and
`group_b`. This avoids cross-group references at the cost of code duplication.

### Pattern: Force specific targets into the binary

```python
link_group_map = [
    ("NO_MATCH", [(":critical_lib", "node", None, None)]),
    ("everything_else", [(None, "tree", "pattern:fbcode//...", None)]),
]
```

`critical_lib` is kept in the main binary even though `everything_else` would
otherwise claim it.

## Interaction Matrix

How the different options interact with each other:

| Feature                      | `MATCH_ALL`      | `NO_MATCH`     | `MATCH_DIRECT_DEPS` | force-static         | discard |
|------------------------------|------------------|----------------|---------------------|----------------------|---------|
| Creates a .so?               | No               | No             | No                  | N/A                  | No      |
| Can duplicate symbols?       | Yes (all groups) | No             | Yes (rdep groups)   | Yes                  | No      |
| Stops shared propagation?    | No               | Yes            | No                  | No                   | N/A     |
| Is a public node?            | Excluded         | Excluded       | Excluded            | N/A                  | N/A     |
| Appears in link-groups-info? | In every group   | In binary only | In rdep groups      | In dependent's group | Nowhere |

## `enable_if_node_count_exceeds`

This attribute makes a group conditional on the binary's dependency graph size.
It's used for groups that are only useful for large binaries:

```python
link_group_map = [
    ("large_dep", [(":lib", "tree", None, None)],
     {"enable_if_node_count_exceeds": 10000}),
]
```

If the binary has fewer than 10,000 nodes in its linkable graph, this group is
silently disabled and its targets stay in the main binary.

## `no_as_needed`

By default, the linker may drop `DT_NEEDED` entries for shared libraries that
are not directly referenced by any symbol. `no_as_needed` prevents this:

```python
link_group_map = [
    ("runtime_only", [(":lib", "tree", None, None)],
     {"no_as_needed": True}),
]
```

This wraps the group's .so with `--no-as-needed` / `--as-needed` flags,
ensuring it's always loaded at runtime even if no static symbol references it
(useful for libraries that register themselves via constructors).

## Python dlopen-enabled Libraries

When link groups are used by a Python binary, additional link group specs are
auto-generated for two categories of native dependencies: **dlopen-enabled
libraries** and **Python native extensions**. These are created by
`_get_root_link_group_specs` in `python/linking/native.bzl` and prepended to
any user-defined link group specs, giving them higher precedence.

### How libraries opt in

C++ and Rust libraries set `supports_python_dlopen = True` in their rule
definition. This causes the rule to emit a `DlopenableLibraryInfo` marker
provider (an empty provider with no fields). Prebuilt C++ libraries default
to `supports_python_dlopen = True`.

### What happens at Python binary analysis time

When `process_native_linking` builds a Python binary, it calls
`merge_cxx_extension_info` which scans first-order deps
(`ctx.attrs.deps + ctx.attrs.preload_deps`) and separates them into:

- **`dlopen_deps`**: deps with `DlopenableLibraryInfo` — C++/Rust libs that
  should be their own standalone .so
- **`shared_only_libs`**: prebuilt libs that only provide shared linkage
  (detected by having `MergedLinkInfo` but no `LinkableRootInfo`)
- **`unembeddable_extensions`**: Python C extensions that can't be statically
  embedded

### Auto-generated specs

For each `dlopen_dep`, a `LinkGroupLibSpec` is created with:

```
LinkGroupLibSpec(
    name = <lib name>,
    is_shared_lib = True,        # gets SONAME, stub library, SharedLibraries packaging
    root = <LinkableRootInfo>,   # private linker flags, version scripts
    group = Group(
        name = <lib name>,
        mappings = [GroupMapping(
            roots = [<lib label>],
            traversal = Traversal("node"),  # only the library itself, not transitive deps
        )],
    ),
)
```

For each unembeddable Python extension, a similar spec is created but with
`is_shared_lib = False` (no SONAME, no stub library).

### `is_shared_lib` controls three behaviors in Phase 7

| Behavior                  | `is_shared_lib = True` (dlopen libs) | `is_shared_lib = False` (extensions) |
|---------------------------|--------------------------------------|--------------------------------------|
| SONAME on .so             | Yes                                  | No                                   |
| Stub library generated    | Yes (other groups can link against)  | No                                   |
| Packaged as SharedLibrary | Yes (in runtime symlink tree)        | No (just an artifact)                |

Stub libraries are empty .so files created before the real link groups are
linked. They allow other groups to declare `DT_NEEDED` entries against the
dlopen lib. The real .so replaces the stub at runtime.

Symbol export is also critical: after linking each dlopen group's .so, the
system extracts its undefined and global symbols. These are fed back to the
main executable link as `-u <symbol>` flags (forcing the linker to keep symbols
needed by dlopen libs) and `--dynamic-list` entries (exporting symbols to the
dynamic symbol table so dlopen'd libs can resolve them at runtime). Without
this, the main executable's linker would discard symbols that nothing in the
static link references, even though dlopen'd libs need them at runtime.

### Shared-only libraries

Prebuilt libraries that only provide shared linkage get `Group` records (not
`LinkGroupLibSpec`s) with `preferred_linkage = Linkage("shared")`. This forces
them to be linked dynamically rather than attempting static linkage that would
fail. These groups don't produce their own .so — they just ensure the existing
.so is referenced correctly.

### Link ordering edge case

Python dlopen deps are not part of the main dependency graph rooted at the
executable. The normal DFS in `collect_linkables` (Phase 5) may not find them
through the executable's deps. The `extra_link_roots` mechanism adds dlopen
deps as additional starting points for the linkable collection, and there's a
fixup in `collect_linkables` for ordering correctness.
