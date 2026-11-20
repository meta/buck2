---
id: compilation_database
title: Compilation databases
---

# Compilation databases

Compilation databases are JSON files that Clang-based tooling (e.g.
clangd and clang-tidy) consume to know what flags (in particular related
to include paths) are used to build C++ files.

## Generating compilation databases

You can generate compilation databases for consumption by tools such as clangd
and clang-tidy by running the following BXL script:

```sh
buck2 bxl prelude//cxx/tools/compilation_database.bxl:generate -- --targets ...
```

The script will generate a compilation database for all source and header inputs
to the targets listed on the command line. The path to the database is printed
to `stdout`. Note that files that are referenced by multiple targets will have
multiple associated entries in the database, which may not be desirable in all
circumstances. For example, clang-tidy runs analysis for each entry sequentially
when the file being linted has several entries.

It is common to symlink the resulting data at the root of the repository:

```sh
ln -sf $(buck2 bxl prelude//cxx/tools/compilation_database.bxl:generate -- --targets ...) $(git rev-parse --show-toplevel)
```

Since the path to the script is rather long, consider setting up an alias in
your repository:

```python
# `comp_db.bxl`

load("@prelude//cxx/tools/compilation_database.bxl:generate", "generate")

gen = generate
```

```sh
ln -sf $(buck2 bxl comp_db.bxl:gen -- --targets ...) $(git rev-parse --show-toplevel)
```

Providing better ergonomics for BXL scripts (such as enabling something like
`buck2 comp_db`) is being discussed
[here](https://github.com/facebook/buck2/issues/86).

## Tools jumping to `buck-out`

You may notice that your tools (e.g. clangd's Go To Definition feature)
jump to symlinks into `buck-out`, rather than into the source tree.

This is often problematic, because `buck-out` is not under source
control (so all VCS-related editor tools fail), and text editors
typically handle those symlinks poorly (e.g. VS Code will keep separate
tabs for the symlink and the source header, and Vim will reuse an
existing symlink buffer even when explicitly opening the source header).

The fix for this is to use header maps instead. To enable header maps,
set `header_mode = HeaderMode("header_map_only")` in your
`CxxToolchainInfo`.

Note that header maps are not supported by GCC. This should not be a
problem since compilation databases are Clang technology, but you can
setup a [build constraint](../../rule_authors/configurations.md) and
conditionally set the header mode based on it if needed.
