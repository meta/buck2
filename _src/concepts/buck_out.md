---
id: buck_out
title: buck-out
---

# buck-out

Buck2 stores build artifacts in a directory named `buck-out` in the root of your
[project](glossary.md#project). You should not make assumptions about where
Buck2 places your build artifacts within the directory structure beneath
`buck-out` as these locations depend on Buck2's implementation and could
potentially change over time. Instead, to obtain the location of the build
artifact for a particular target, you can use one of the `--show-*-output`
options with the [`buck2 build`](../../users/commands/build) or
[`buck2 targets`](../../users/commands/targets) commands, most commonly
`--show-output`. For the full list of ways to show the output location, you can
run `buck2 build --help` or `buck2 targets --help`.

```sh
buck2 targets --show-output <target>
buck2 build --show-output <target>
```
