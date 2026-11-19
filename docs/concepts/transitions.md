---
id: transitions
title: Configuration transitions
---

Configuration transition is a mechanism for changing the configuration when
depending on a target.

Currently, Buck2 has incoming and outgoing transitions:

- **Incoming** transitions are specified per-target and take effect when
  depending on that target.
- **Outgoing** transitions are specified on an attribute and take effect on
  dependencies that appear in that attribute.

## Using incoming transitions

Assuming that a transition rule called `:my_transition` exists
(see the page on [configuration transitions for rule
authors](../rule_authors/configuration_transitions.md) for how to define
one), it can be used in the following way:

```python
cxx_library(
    name = "lib1",
    exported_headers = ["lib1.h"],
    exported_deps = [":lib2"],
    incoming_transition = [":my_transition"],
)

cxx_library(
    name = "lib2",
    exported_headers = ["lib2.h"],
)
```

When building `lib1`, whatever the current configuration is will be
passed to `my_transition`, which will return a new configuration. This
new configuration will be used to build both `lib1` and `lib2`.

The transition is given the current configuration and returns a new one.
For example, it could:

- Add a new constraint to the existing configuration (e.g. add a
  `release` constraint so that it and all its dependencies are built in
  an optimized mode).
- Return a blank configuration with no constraints at all (useful to
  deduplicate work when actions produce identical outputs, such as when
  downloading artifacts).
- Make up an arbitrary configuration based on the constraints currently
  set.

## Using outgoing transitions

Outgoing transitions are used by rule authors, not end users.

See the page on [configuration transitions for rule
authors](../rule_authors/configuration_transitions.md) for more
information.
