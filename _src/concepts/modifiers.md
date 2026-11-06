---
id: modifiers
title: Configuration modifiers
---

Modifiers (also referred to as configuration modifiers) are a feature that lets
users add [constraints](../rule_authors/configurations.md) to individual
directories, target definitions and individual `buck2` invocations.

They are the recommended to customize build configurations when building
targets directly. If you need to customize parts of your build graph
(e.g. always build some specific dependencies in release mode),
[configuration transitions](transitions.md) are more appropriate.

## (Open-Source Only) Getting started with modifiers

To add support for modifiers in your project, simply call `set_cfg_constructor`
in your root `PACKAGE` file. Note this is not needed internally as modifiers are
already enabled with all the right paramaters.

See the [dedicated how-to](../../users/how_tos/modifiers_setup) for more
information.

## Per-`PACKAGE` modifiers

Modifiers can be specified in `PACKAGE` files. A per-PACKAGE modifier all
targets in the directory of that PACKAGE file. For example, a modifier specified
in `foo/PACKAGE` will cover all targets in `foo/...` and all targets in
`foo/bar/...`.

See the [dedicated how-to](../../users/how_tos/modifiers_package) for more
information.

## Per-target modifiers

Modifiers can be added to individual targets. See the
[dedicated how-to](../../users/how_tos/modifiers_target) for more information.

## CLI modifiers

Modifiers can be specified on the CLI to build a target with a specific
constraint. For example, this can be used to build a target with a non-default
compiler, or to run a test using a specific sanitizer.

See the [dedicated how-to](../../users/how_tos/modifiers_cli) for more
information.

## More information

Official documentation on modifiers is sparse at the moment, so you might be
interested in reading [the original RFC](../rfcs/implemented/modifiers.md)
for more details.
