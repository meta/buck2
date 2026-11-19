# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Shared schema for the [native_build_commands] sub-target, used by both
# android_binary_native_library_rules.bzl and meta_only/gatorade.bzl. This lives in a leaf module
# because gatorade.bzl is loaded by android_binary_native_library_rules.bzl and so cannot import the
# entry helper back from there.

# Whether to capture per-source compile commands (the expensive [native_build_commands][compile]
# kind). Off by default; enable with `-c cxx.emit_native_build_commands=true`. Follows the
# FORCE_SINGLE_CPU config-backed-attr pattern. Single source of truth for both the cxx_library attr
# that carries compile commands onto the linkable graph (cxx/attrs.bzl) and the app-side compile
# emission (android_binary_native_library_rules.bzl).
EMIT_NATIVE_BUILD_COMMANDS = read_root_config("cxx", "emit_native_build_commands") in ("True", "true")

# Command "kinds" collected by the [native_build_commands] sub-target. Each kind also gets its own
# filtered sub-target, e.g. `TARGET[native_build_commands][relink]`. Note `mergemap` (the one
# whole-graph merge-map PLAN computation) is distinct from `merge` (the per-soname merge link that
# combines constituent libraries), and `link` is reserved for a plain per-library link on apps that
# neither merge nor relink.
NATIVE_BUILD_COMMAND_KINDS = [
    "compile",
    "mergemap",
    "merge",
    "link",
    "relink",
    "early_gatorade",
    "middle_gatorade",
    "late_gatorade",
    "bolt",
]

# The Gatorade phases exposed as top-level product sub-targets on Android app targets: building
# `TARGET[early_gatorade]` runs that phase and outputs the artifacts its gatorade invocation(s)
# produce, as a directory (like [native_libs]). Kept in this leaf module as the single source of
# truth for both the app rule that registers them and apk_genrule.bzl that forwards them through the
# redex/repack/resign wrapper. (Unrelated to NATIVE_BUILD_COMMAND_KINDS above, which is the separate
# [native_build_commands] JSON schema; these names coincide with those Gatorade kinds only by
# convention.)
GATORADE_PHASE_SUBTARGETS = [
    "early_gatorade",
    "middle_gatorade",
    "late_gatorade",
]

# One entry in the [native_build_commands] JSON. `argv` is embedded as an ArgLike via
# write_json(with_inputs = False) so an entry renders its command without materializing the produced
# artifact, and `argsfile` lets a consumer expand the full flag list when the argv references an
# @argsfile. This shape is intentionally its own thing, not a compile_commands.json entry nor the
# [linker_commands] shape: it spans compile/link/merge/relink/gatorade/bolt uniformly, so consumers
# should key off `kind` rather than assume any single tool's schema.
#
# `argv` must not contain any `.as_output()` reference: write_json cannot re-serialize an output
# artifact, and doing so would also make this entry's writer a second producer of that artifact.
# Pass the plain output artifact instead.
#
# Field notes:
#  - `argv`: for link/merge/relink/bolt/gatorade kinds this is the real executed command. For
#    kind=compile it is the compile_commands.json-style invocation (compiler + @argsfile + source),
#    NOT the exact object-producing action (it omits `-o`, LTO/bitcode flavor flags, dep-file and
#    compiler-wrapper args); it mirrors what comp_db.bzl emits for a source. Use it to reproduce a
#    representative compile, not to byte-match the build action.
#  - `soname`: for per-library kinds, the FINAL shipped/merged soname the command contributes to.
#    Best-effort for kind=compile: the compile->merged-soname map is built from each merged lib's
#    PRIMARY constituents (matching shared_object_targets.txt), so an object that lands in a merged
#    .so only as a non-primary / transitively-linked-static constituent is reported under its own
#    `pre_merge_soname` rather than the merged soname. Split groups and late-gatorade code motion also
#    make it approximate — cross-reference the app's merge.map / shared_object_targets.txt when an
#    authoritative object->soname mapping is needed. None for whole-graph steps (e.g. compute_mergemap).
#  - `pre_merge_soname`: for kind=compile, the per-cxx_library (pre-merge) soname the object was
#    built for; None for other kinds.
#  - `output`: the produced artifact's short_path for link/merge/relink/bolt/gatorade kinds; for
#    kind=compile it is the compiled SOURCE path (the produced object is not available here — the
#    full per-source attribution lives in `identifier`).
#  - `argsfile`: the @argsfile the command references, if any. The sub-target MATERIALIZES it on disk
#    for the link-family (merge/link/relink/bolt) and compile kinds, so a consumer that builds
#    TARGET[native_build_commands] (or a per-kind filter) can read the flags. Gatorade argsfiles
#    (early's input-containers list, late codegen and tmp-link) are recorded by PATH only, not
#    materialized by the sub-target — build the app itself to obtain those files on disk.
def native_build_command_entry(kind, category, arch, soname, output, argv, argsfile = None, identifier = None, pre_merge_soname = None):
    if kind not in NATIVE_BUILD_COMMAND_KINDS:
        fail("native_build_command_entry: unknown kind {!r}; expected one of {}".format(kind, NATIVE_BUILD_COMMAND_KINDS))
    return {
        "arch": arch,
        "argsfile": argsfile,
        "argv": argv,
        "category": category,
        "identifier": identifier,
        "kind": kind,
        "output": output,
        "pre_merge_soname": pre_merge_soname,
        "soname": soname,
    }

# Record a link-family entry (link/merge/relink/bolt) for a LinkedObject, deduplicating the identical
# guard+append that otherwise gets copy-pasted at every capture site. No-op when recording is off
# (native_cmd_entries == None) or the object carries no reified linker command (prebuilt copies,
# pre-bolt/late-gatorade dummy objects, and DistLTO all have none). `linked_object` is accepted
# untyped to keep this a leaf module (no prelude/linking import); it must expose the LinkedObject
# fields `output`, `linker_command`, and `linker_argsfile`.
#
# Intentionally NOT built on linking's make_link_command_debug_output: that helper also requires an
# argsfile and drops the command otherwise, which would silently lose links that inline all flags —
# unacceptable for a capture-everything sub-target. Here argsfile is optional (recorded as None).
def record_link_command(native_cmd_entries, kind, arch, soname, linked_object, category = "cxx_link", pre_merge_soname = None):
    if native_cmd_entries == None or not linked_object.linker_command:
        return
    native_cmd_entries.append(
        native_build_command_entry(
            kind = kind,
            category = category,
            arch = arch,
            soname = soname,
            output = linked_object.output.short_path,
            argv = linked_object.linker_command,
            argsfile = linked_object.linker_argsfile,
            pre_merge_soname = pre_merge_soname,
        ),
    )
