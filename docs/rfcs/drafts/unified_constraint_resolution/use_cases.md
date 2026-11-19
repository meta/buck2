# Use Cases

## Additional use cases of sub-constraints

### Replacing selects in transition attrs

Selects inside transition attrs are another way to express conditionality, but they have the same shortcomings as the [transition-based approaches in the problems tab](problems.md#case-3-use-a-transition-function): anyone writing their own transition can silently override the conditional, and library targets don't pick up the transition at all. Sub-constraints let you push the per-OS or per-config branching into the constraint itself, so the transition reads the constraint directly and the select disappears.

Take the Apple SDK target version. Today `apple_bundle` can carry a `minimum_os_version` attr set with a select:

```python
apple_bundle(
  minimum_os_version = select({
    "cfg//:os[iphoneos]": "14.0",
    "cfg//:os[macos]": "12.0",
  }),
)
```

`target_sdk_version_transition` (in `prelude/apple/user/target_sdk_version_transition.bzl`) reads `attrs.minimum_os_version` and writes the `target-sdk-version` constraint. The per-OS branching can move into the constraint via a sub-constraint per OS:

```python
# cfg//apple/BUCK

constraint(
  name = "version",
  values = ["v12", "v13", "v14", "v15", "v16"],
  sub_constraints = [
    sub_constraint(
      name = "min_ios",
      values = ["v14", "v15", "v16"],
      default = "v14",
    ),
    sub_constraint(
      name = "min_macos",
      values = ["v12", "v13"],
      default = "v12",
    ),
    # min_watchos, min_tvos, ...
  ],
  default = select({
    "cfg//os:iphoneos": select({
      "min_ios[v14]": "v14",
      "min_ios[v15]": "v15",
      "min_ios[v16]": "v16",
    }),
    "cfg//os:macos": select({
      "min_macos[v12]": "v12",
      "min_macos[v13]": "v13",
    }),
    "DEFAULT": "v14",
  }),
)
```

The bundle rule drops `minimum_os_version` entirely. PACKAGE files set `cfg//apple:version/min_ios[v15]`, etc. as modifiers when they need to override the per-OS minimum. The select inside the transition attr can go away entirely.

### Rolling out new conditional constraint behavior incrementally

A common conditional-modifier workflow is gating a new behavior behind a constraint, flipping it on per PACKAGE, and deleting the gate once rollout is done. Sub-constraints capture the same workflow without polluting the public constraint namespace.

For example, gating the [NCCL latest-when-sanitized](main.md#invariant) conditionality behind a rollout sub-constraint looks as follows:

```python
# cfg//nccl/BUCK file

constraint(
  name = "version",
  values = ["stable", "beta", "latest"],
  default = "stable",
  sub_constraints = [
    sub_constraint(
      name = "use_latest_when_sanitized",
      values = ["on", "off"],
      default = "off",
    ),
  ],
  invariant = select({
    "use_latest_when_sanitized[on]": select({
      "cfg//:sanitizer[none]": None,
      "DEFAULT": "latest",
    }),
    "DEFAULT": None,
  }),
)
```

PACKAGE files that want to opt in set `cfg//nccl:version/use_latest_when_sanitized[on]`. Once every project has opted in, the sub-constraint can be removed and the new branch becomes unconditional.

Sub-constraints are like private fields of an object in an object-oriented programming model. Reach for sub-constraints when an extra input dimension is meaningful only inside one constraint's resolution and shouldn't leak into the repo's public API. If the dimension is useful to other selects, define a normal constraint instead.

## Dev vs opt mode

Build mode (`dev`, `opt`) drives defaults for many compile-side constraints: opt level, lto, debug symbols, sanitizer-on-by-default. Today this is scattered across selects in macros or hardcoded in transitions, and there's no clean way for projects to opt into project-specific dev mode defaults without redefining the global mapping.

With conditional constraints, `build_mode` becomes the single global knob. Each downstream constraint encodes its own dev/opt behaviors and exposes a sub-constraint for projects to override.

Example 1: `opt_level` lets projects pick their dev mode optimization level via a sub-constraint, without affecting other constraints derived from `build_mode`.

```python
# cfg//cpp/BUCK

constraint(
  name = "optimization_level",
  values = ["o0", "o1", "o2", "o3"],
  sub_constraints = [
    sub_constraint(
      name = "dev_mode_optimization_level",
      values = ["o0", "o1", "o2"],
      default = "o0",
    ),
  ],
  default = select({
    "cfg//:build_mode[opt]": "o3",
    "cfg//:build_mode[dev]": select({
      "dev_mode_optimization_level[o0]": "o0",
      "dev_mode_optimization_level[o1]": "o1",
      "dev_mode_optimization_level[o2]": "o2",
    }),
  }),
)
```

A project that wants `-O1` in dev for faster iteration sets `cfg//cpp:optimization_level/dev_mode_optimization_level[o1]` in its PACKAGE. The global dev vs opt mapping stays intact for everyone else.

Example 2: sanitizer defaults in fbcode. Internally, we like to enable ASAN on linux dev mode by default to catch bugs. However, many projects are incompatible with ASAN, so users are welcome to override that behavior for their projects to build with no sanitizer.

```python
# cfg//BUCK

constraint(
  name = "sanitizer",
  values = ["asan", "tsan", "ubsan", "none"],
  default = select({
    "cfg//:build_mode[dev]": select({
      "cfg//:os[linux]": "asan",
      "DEFAULT": "none",
    }),
    "DEFAULT": "none",
  }),
)
```

When the `"default"` input value is specified, users get this conditional default behavior for sanitizer. They can opt out by setting any sanitizer constraint as a modifier for their project, or opt back in by setting `cfg//:sanitizer[default]` as a modifier.

Alternatively, sanitizer opts projects in or out of dev-mode ASAN via a yes/no sub-constraint.

```python
# cfg//BUCK

constraint(
  name = "sanitizer",
  values = ["asan", "tsan", "ubsan", "none"],
  sub_constraints = [
    sub_constraint(
      name = "dev_implies_asan",
      values = ["yes", "no"],
      default = "yes",
    ),
  ],
  default = select({
    "cfg//:build_mode[dev]": select({
      "cfg//:os[linux]": select({
        "dev_implies_asan[yes]": "asan",
        "dev_implies_asan[no]": "none",
      }),
      "DEFAULT": "none",
    }),
    "DEFAULT": "none",
  }),
)
```

A project incompatible with ASAN sets `cfg//:sanitizer/dev_implies_asan[no]` in its PACKAGE and stops getting ASAN in dev builds, without changing what `[default]` means for any other project.

## CUDA arches

We support a fleet of CUDA arches in fbcode (`a100`, `v100`, `p100`, `b200`, and more), and we want to turn these into constraints. The catch: arches are additive (a build can target several at once), so a single multi-valued constraint can't represent every combination.

The naive solution is one constraint per arch:

```python
# cfg//cuda/BUCK file

constraint(
  name = "a100",
  values = ["on", "off"],
  default = "off",
)

constraint(
  name = "v100",
  values = ["on", "off"],
  default = "off",
)
```

This shape has two problems. First, arches will become additive in sub-PACKAGEs over time. People enable the arch they need wherever they need it but rarely think to restrict the rest of the set. Second, a free-form on/off mix per arch is too permissive. We'd rather offer a curated list of supported arch sets and have users pick from it.

That gives us two requirements:

1. Set arches as a group, picking from a curated list of supported combinations (e.g. `"default"` or `"default with b200"`), rather than arbitrary combinations.
2. Select on single arches (e.g. `cfg//cuda:b200[on]`) when picking sources or deps, as that is by far the most convenient.

Conditional constraint handles both:

```python
# bzl file

def cuda_arches(cuda_arches: dict[str, list[str]]):
  # Input surface: users pick a curated combination.
  constraint(
    name = "arches",
    values = list(CUDA_ARCH_GROUPS.keys()),
    default = "default",
  )

  # Read surface: one derived constraint per arch, resolving to "on"/"off"
  # based on which group is selected.
  _arch_to_groups = {}
  for group, arches in CUDA_ARCH_GROUPS.items():
    for arch in arches:
      _arch_to_groups.setdefault(arch, []).append(group)

  for arch, groups in _arch_to_groups.items():
    constraint(
      name = arch,
      values = ["on", "off"],
      invariant = select(
        {"cfg//cuda:arches[{}]".format(group): "on" for group in groups}
        | {"DEFAULT": "off"},
      ),
    )

# cfg//cuda/BUCK

cuda_arches({
  "default": ["a100", "v100", "p100"],
  "default_with_b200": ["a100", "v100", "p100", "b200"],
  "b200_only": ["b200"],
})
```

Setting modifiers always goes through arches:

```python
# fbcode/path/to/PACKAGE

set_cfg_modifiers(["cfg//cuda:arches[default_with_b200]"])
```

Selecting always goes through the per-arch constraint:

```python
cuda_library(
  name = "my_kernel",
  srcs = select({
    "cfg//cuda:b200[on]": ["b200_kernel.cu"],
    "DEFAULT": ["generic_kernel.cu"],
  }),
)
```

## Internal-only use cases

See [Use cases (FB-Internal)](https://docs.google.com/document/d/1peCMKrjUphmdXYfCT_vkdfGY948Xqeyz_G5Xg56737M/edit).
