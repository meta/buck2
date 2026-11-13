---
id: labels
title: Labels
---

The term "label" is a little overloaded in Buck2. It can refer to:

- A target identifier composed of its name, cell and path.
- One or more tag-like strings attached to a target.

For the first definition of label, the target label is determined by where it's
defined, as well as its `name` attribute. Labels are exposed in the build API
via the [`Label`](../api/build/Label.md) type, which can be used in rule
implementations and BXL scripts.

For the second definition of label, rules expose a `label` attribute, which is a
list of strings. Those labels could be used to filter targets in query
expressions (e.g., by using `attrfilter` query function). Also specific labels
can be targeted when running `buck2 test` by adding the `--exclude`/`--include`
flags. See the [test execution docs](../rule_authors/test_execution.md) for more
information.
