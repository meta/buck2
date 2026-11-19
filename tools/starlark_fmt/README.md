# starlark_fmt

An opinionated formatter for Starlark / BUCK files. Drop-in replacement for
buildifier with additional autofix capabilities.

## What It Does

starlark_fmt formats Starlark source files through a three-phase pipeline:

1. **Normalize** — Convert CRLF line endings to LF.
2. **Autofixes** — Structural transforms applied in order:
   - **Remove unused loads** — Deletes load symbols not referenced in the file.
   - **Sort load statements** — Orders loads by path (cell `//` < external
     `fbsource//` < relative `:`), then sorts symbols within each load.
   - **Sort dictionary keys** — Alphabetizes string keys in dict literals
     (multi-pass, up to 5 iterations for nested dicts).
   - **Sort list arguments** — Sorts values in allowlisted rule arguments
     (e.g. `deps`, `srcs`, `visibility`). Uses target-path ordering:
     relative `:` < cell `//` < external `fbsource//`.
   - **Sort keyword arguments** — Orders kwargs by priority from config
     (e.g. `name` first), then alphabetically.
3. **Cosmetic formatting** — Ruff-based Python formatter pass:
   - 160-character line width (matches buildifier default).
   - 4-space indentation.
   - Double quotes.
   - Magic trailing comma respected (trailing comma keeps multi-line form).
   - Spaces around `=` in keyword arguments (`name = "foo"`, not `name="foo"`).
   - At most one blank line between statements.

## Subcommands

starlark_fmt requires a `--config` flag pointing to a JSON configuration file
(see [Configuration](#configuration) below).

```
starlark_fmt --config <CONFIG_PATH> <SUBCOMMAND>
```

| Subcommand | Description |
|------------|-------------|
| `fmt <FILES...>` | Format files in-place. Supports `@argfile` for file lists. |
| `lint <FILES...>` | Emit JSON `LintMessage` records to stdout (for `arc lint` integration). |
| `diff <FILE>` | Show a colored diff of what would change. `--show-all` includes unchanged lines. |
| `stdin [--path FILE]` | Read from stdin, write formatted output to stdout. `--path` provides context for config matching. |

Add `--timing` before the subcommand for a per-phase timing breakdown.

Multiple files are processed in parallel via rayon.

## Suppressing Formatting

### `# fmt: off` / `# fmt: on`

Disable **all** formatting and autofixes for a region:

```python
# fmt: off
hand_tuned_dict = {
    "z": 3,
    "a": 1,
}
# fmt: on
```

Everything between `# fmt: off` and `# fmt: on` is left untouched. If
`# fmt: on` is omitted, suppression extends to end-of-file.

### `# @unsorted-dict-items`

Prevent dictionary key sorting for a specific dict. Place the comment on the
line before the dict or inline on the opening `{`:

```python
# @unsorted-dict-items
priority_order = {
    "critical": 1,
    "high": 2,
    "low": 3,
}

also_unsorted = {  # @unsorted-dict-items
    "z": 1,
    "a": 2,
}
```

### `# @unused`

Preserve a specific load symbol that would otherwise be removed as unused.
This annotation goes **on the symbol itself** in a multiline load:

```python
load(
    "//foo:defs.bzl",
    "used_rule",
    "unused_but_needed",  # @unused
)
```

Note: a statement-level `# @unused` comment on a single-line load does **not**
work. The annotation must be per-symbol in a multiline load.

### `# keep sorted`

Opt-in sorting for standalone list assignments (lists not inside a rule call).
By default, standalone lists are **not** sorted. Add `# keep sorted` before the
assignment or before the first element:

```python
# keep sorted
ALL_TARGETS = [
    "//bar:bar",
    "//foo:foo",
]
```

### `# starlark-fmt: sort-by = ...`

Opt in a particular keyword argument to sorting elements that the default
list-argument sorter does not handle by placing a single-line JSON sort key
immediately above the argument. This does not require the argument to appear in
`IsSortableListArg`, and overrides any `ListSortKeys` or `SortableBlacklist`
entry for that argument:

```python
some_rule(
    # starlark-fmt: sort-by = {"first_of": ["string", {"tuple_item": 0}]}
    items = [
        ("z", True),
        "m",
        ("a", False),
    ],
)
```

Available sort keys are:

| Sort key | Extracted value |
|----------|-----------------|
| `"string"` | The value of a string literal. |
| `"call_name"` | A simple or dotted function name. |
| `{"tuple_item": N}` | The string literal at tuple index `N`. |
| `{"call_keyword": "name"}` | The string literal passed to the named call argument. |
| `{"first_of": [...]}` | The first enclosed sort key that applies to the element. |

For example, calls can sort by an explicit `name` and fall back to their
function name:

```python
some_rule(
    # starlark-fmt: sort-by = {"first_of": [{"call_keyword": "name"}, "call_name"]}
    items = [zebra(), factory(name = "alpha"), module.bravo()],
)
```

Every list element selected by an inline directive must produce a string key.
Invalid inline directives, missing keys, and dynamic key values fail formatting
with a source location rather than partially reordering the list. Equal keys
preserve source order and are not deduplicated. `# do not sort` and `# fmt: off`
take precedence. A commented list with multiple elements on one physical line
is rejected before editing; put each element on its own line so comments can
move with their element.

The same sort-key language can be configured globally with `ListSortKeys`.
An argument name applies to every call, while a `<callee>.<argument>` selector
takes precedence for that callee:

```json
{
  "ListSortKeys": {
    "items": {"first_of": ["string", {"tuple_item": 0}]},
    "some_rule.items": {
      "first_of": [{"call_keyword": "name"}, "call_name"]
    }
  }
}
```

If a configured key does not apply to every element, that list is left
unchanged.

Configured sort keys respect `SortableBlacklist`. Resolution order is
`# fmt: off` / `# do not sort`, an inline `sort-by`, `SortableBlacklist`, a
`<callee>.<argument>` sort key, an argument-only sort key, then legacy
`IsSortableListArg` sorting.

### Config blocklist

The JSON config file can blocklist specific `<callee>.<argument>` selectors
from list sorting via the `SortableBlacklist` map. For example, to prevent
sorting `genrule.srcs`:

```json
{
  "SortableBlacklist": {
    "genrule.srcs": true
  }
}
```

## Configuration

starlark_fmt reads a JSON config file passed via `--config`. The production
config lives at `tools/third-party/buildifier/tables.json`. The schema:

```json
{
  "IsSortableListArg": {
    "deps": true,
    "srcs": true,
    "visibility": true
  },
  "SortableBlacklist": {
    "genrule.srcs": true
  },
  "NamePriority": {
    "name": -99,
    "visibility": 50
  },
  "ListSortKeys": {
    "plugins": {
      "first_of": [{"call_keyword": "name"}, "call_name"]
    }
  }
}
```

| Field | Purpose |
|-------|---------|
| `IsSortableListArg` | Set of arg names whose list values should be sorted in rule calls. |
| `SortableBlacklist` | Set of `<callee>.<argument>` selectors to exclude from list sorting. |
| `NamePriority` | Priority ordering for kwargs. Lower numbers sort first. Args not listed default to priority 0 and sort alphabetically among themselves. |
| `ListSortKeys` | Optional argument or `<callee>.<argument>` selectors mapped to structural sort keys. |

## Running Tests

### Rust unit tests

```bash
buck2 test fbcode//buck2/tools/starlark_fmt:starlark_fmt-unittest
```

This runs all `#[cfg(test)]` modules across the crate, covering the parser
infrastructure, each autofix pass, formatting, and fuzz tests.

### Cram integration tests

```bash
buck2 test fbcode//buck2/tools/starlark_fmt:tests
```

This runs 9 `.t` files under `tests/` that exercise the CLI end-to-end.

### Run everything

```bash
buck2 test fbcode//buck2/tools/starlark_fmt:
```

## arc lint Integration

The `lint` subcommand outputs one JSON `LintMessage` per line with
`original` / `replacement` fields, enabling `arc lint` to show diffs and
auto-apply fixes. Lint exits 0 even when files have errors (errors are
reported as `LintMessage` records with severity `error`).

## Building

```bash
buck2 build fbcode//buck2/tools/starlark_fmt:starlark_fmt
```

The binary is also distributed cross-platform (macOS aarch64/x86_64, Linux
aarch64/x86_64, Windows) via DotSlash.
