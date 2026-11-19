# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:java_providers.bzl", "JavaLibraryInfo")
load(
    "@prelude//utils:build_target_pattern.bzl",
    "parse_build_target_pattern",
)
load("@prelude//utils:lazy.bzl", "lazy")

_JVM_ABI_GEN_PRESERVE_DECLARATION_ORDER = "plugin:org.jetbrains.kotlin.jvm.abi:preserveDeclarationOrder="

# kotlinc is strict about the target that you can pass, e.g.
# error: unknown JVM target version: 8.  Supported versions: 1.8, 9, 10, 11, 12
def get_kotlinc_compatible_target(target: str) -> str:
    return "1.8" if target == "8" else target

def kotlinc_arg_text(arg) -> str:
    """The text kotlincd gives kotlinc for one `extra_kotlinc_arguments`
    element: KotlinCStepsBuilder passes each element unchanged. `str()` of an
    `attrs.arg()` value is that text in one pair of double quotes, with a
    macro written as `$(location ...)`."""
    if isinstance(arg, str):
        return arg
    return str(arg)[1:-1]

def jvm_abi_gen_preserves_declaration_order(extra_kotlinc_arguments: list) -> bool:
    """
    Whether these `extra_kotlinc_arguments` set jvm-abi-gen's `preserveDeclarationOrder` option,
    which keeps the members of the class ABI in the order kotlinc generates instead of sorting them.
    """
    preserves = False
    for arg in extra_kotlinc_arguments:
        # A `-P` value is a comma-separated list of plugin options, the last setting of an option
        # winning.
        for option in kotlinc_arg_text(arg).split(","):
            if option.startswith(_JVM_ABI_GEN_PRESERVE_DECLARATION_ORDER):
                preserves = option[len(_JVM_ABI_GEN_PRESERVE_DECLARATION_ORDER) :] == "true"
    return preserves

def get_friend_paths(ctx: AnalysisContext) -> list[Dependency]:
    """
    Resolves the `friend_paths` attribute to the deps whose ABI jars kotlinc gets as `-Xfriend-paths`.
    """
    friend_paths = ctx.attrs.friend_paths
    explicit_friends = [dep for dep in friend_paths if not isinstance(dep, str)]
    patterns = [parse_build_target_pattern(f) for f in friend_paths if isinstance(f, str)]
    if not patterns:
        return explicit_friends

    friends = {dep.label.raw_target(): dep for dep in explicit_friends}
    for dep in ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.provided_deps + ctx.attrs.exported_provided_deps:
        if JavaLibraryInfo not in dep:
            continue
        target = dep.label.raw_target()
        if target not in friends and lazy.is_any(lambda pattern: pattern.matches(dep.label), patterns):
            friends[target] = dep

    return list(friends.values())
