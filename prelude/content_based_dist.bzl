# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def make_content_based_dist(ctx: AnalysisContext, name: str, exe: Artifact, copies: dict = {}, symlinks: dict = {}) -> (Artifact, Artifact):
    """
    Package a relocatable executable that depends on adjacent files into a single
    content-based directory, so it is self-contained at a content-addressed
    buck-out path. A content change re-hashes the whole directory to a new path
    while the old one persists -- the property that lets a long-running `buck2
    run` binary survive concurrent rebuilds.

    `exe` is COPIED into the directory. Its self-relative lookups -- an `$ORIGIN`
    RPATH, a `current_exe`-relative `.resources.json`, an inplace-PAR
    `$(dirname $(readlink -f $0))`-relative `#link-tree`, etc. -- only resolve
    inside the directory if the exe physically lives there; a symlinked exe would
    resolve them against its realpath instead. Entries in `copies` are laid out
    as real bytes (use for small generated files the bundle should own, e.g. a
    relocatable resource DB); entries in `symlinks` are symlinks to their source
    artifacts (use for large dependencies, to avoid duplicating bytes).

    THE IMMUTABILITY CONTRACT IS PER-ENTRY: the bundle's content hash captures
    copied entries' BYTES but only symlinked entries' TARGET PATHS. A COPIED
    entry is therefore always trample-safe. A SYMLINKED entry is trample-safe
    IFF its target artifact is itself content-based (immutable path); a
    symlink to a config-based artifact keeps pointing at a path whose bytes a
    rebuild rewrites in place -- without re-hashing this bundle. Callers must
    route mutable inputs through `copies` (or make them content-based) when
    they need whole-bundle immutability.

    `copies` and `symlinks` are dicts of path-in-dir (string, relative to the
    result directory) to a bound `artifact`.

    Returns `(bundle_dir, exe)` where `exe` is the copied exe projected back out
    of the bundle, for use as a `RunInfo` command and/or `DefaultInfo` output.
    """
    # Feature-detected (getattr, not a static reference) so this module still
    # LOADS under a buck2 that predates `assembled_dir` (D107250277) -- the
    # static type-checker rejects unknown globals/methods at module-load time,
    # which would break every rust_binary analysis on older binaries. Reaching
    # this function without the API (i.e. actually opting a target in) is a
    # hard, explicit error instead.
    entry_ctors = getattr(__buck2_builtins__, "assembled_dir", None)
    make_assembled_dir = getattr(ctx.actions, "assembled_dir", None)
    if entry_ctors == None or make_assembled_dir == None:
        fail(
            "content-based dist bundles require a buck2 with " + "`ctx.actions.assembled_dir` (D107250277); this buck2 predates it",
        )

    exe_rel = exe.short_path
    contents = {exe_rel: entry_ctors.copy(exe)}

    # Duplicate destinations would silently overwrite dict entries here --
    # including the exe's own entry -- before `assembled_dir`'s non-overlap
    # validation ever sees them. Reject them loudly instead.
    for path, artifact in copies.items():
        if path in contents:
            fail("make_content_based_dist: duplicate bundle destination `{}` (copies)".format(path))
        contents[path] = entry_ctors.copy(artifact)
    for path, artifact in symlinks.items():
        if path in contents:
            fail("make_content_based_dist: duplicate bundle destination `{}` (symlinks)".format(path))
        contents[path] = entry_ctors.symlink(artifact)
    bundle = make_assembled_dir(
        name + "__dist",
        contents = contents,
        has_content_based_path = True,
    )
    return (bundle, bundle.project(exe_rel))
