# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A persistent witness: retain one predecessor instead of copying a complete
# chain into every classpath entry. None represents depth zero.
JavaCompilerDepth = record(
    depth = int,
    label = Label,
    category = str,
    identifier = str | None,
    dependency = typing.Any,
)

def _compiler_depth_value(depth: JavaCompilerDepth | None) -> int:
    return depth.depth if depth else 0

def max_compiler_depth(depths: list[JavaCompilerDepth | None]) -> JavaCompilerDepth | None:
    deepest = None
    for depth in depths:
        if _compiler_depth_value(depth) > _compiler_depth_value(deepest):
            deepest = depth
    return deepest

def compiler_depth_for_action(
    label: Label,
    category: str,
    compiling_deps: TransitiveSet | None,
    identifier: str | None = None,
    source_only: bool = False,
    additional_depth: JavaCompilerDepth | None = None,
) -> JavaCompilerDepth | None:
    dependency = max_compiler_depth([
        compiling_deps.reduce("source_only_abi_compiler_depth" if source_only else "compiler_depth") if compiling_deps else None,
        additional_depth,
    ])

    # Class ABI extraction and other non-compiling actions add zero.
    if category not in ("javacd_jar", "kotlincd_jar", "source_abi_javacd_jar", "source_only_abi_javacd_jar", "source_only_abi_kotlincd_jar"):
        return dependency
    return JavaCompilerDepth(
        depth = _compiler_depth_value(dependency) + 1,
        label = label,
        category = category,
        identifier = identifier,
        dependency = dependency,
    )

def compiler_depth_report(depth: JavaCompilerDepth | None) -> dict:
    chain = []
    node = depth
    for _ in range(_compiler_depth_value(depth)):
        chain.append({"category": node.category, "identifier": node.identifier, "target": str(node.label)})
        node = node.dependency
    return {"chain": list(reversed(chain)), "depth": _compiler_depth_value(depth)}
