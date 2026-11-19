# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":nodejs_providers.bzl", "NodejsTreeInfo", "check_relative_path")

def compose_node_modules(
    actions: AnalysisActions,
    name: str,
    packages: dict[str, Artifact],
    owners: dict[str, Label],
    trees: list[NodejsTreeInfo],
    root_files: dict[str, Artifact] = {},
    root_file_desc: str = "root file '{path}'",
) -> Artifact:
    """A directory holding one real `node_modules` of first-party `packages` and third-party `trees`.

    `packages` and `owners` are keyed by install directory. Each tree's
    packages, minus its `exclude`, are projected out of it as its manifest
    lists them; one listed with an `error` fails. A scope directory is projected whole unless another source
    provides packages in it, one of its packages is excluded, one is a
    symlink, or it holds a dot entry. Dot entries such as `.bin` are never composed. `root_files` are
    placed beside `node_modules` at their keys, which cannot be under
    `node_modules` when anything is composed; `root_file_desc` names the
    offending file in that error, formatted with `path` and `artifact`.
    """
    srcs = {"node_modules/" + n: packages[n] for n in packages}
    for root_name, root_file in root_files.items():
        segments = check_relative_path(root_name, "invalid `root_files` path", allow_colon = True)
        if (packages or trees) and segments[0].lower() == "node_modules":
            fail("{} collides with the composed `node_modules` tree".format(root_file_desc.format(path = root_name, artifact = root_file)))
        srcs[root_name] = root_file
    if not trees:
        return _copy_node_modules(actions, name, name, srcs)
    out = actions.declare_output(name, dir = True)
    actions.dynamic_output_new(
        _compose(
            first_party = {n: str(owners[n].raw_target()) for n in packages},
            manifests = [tree.manifest for tree in trees],
            name = name,
            out = out.as_output(),
            srcs = srcs,
            trees = [(tree.node_modules, str(tree.owner.raw_target()), tree.exclude) for tree in trees],
        ),
    )
    return out

def _copy_node_modules(actions: AnalysisActions, out: str | OutputArtifact, name: str, srcs: dict[str, Artifact]) -> Artifact:
    return actions.copied_dir(out, srcs or {"node_modules": actions.copied_dir(name + "_empty", {})})

def _package_root(dir_name: str) -> str:
    segments = dir_name.split("/")
    return "/".join(segments[:2]) if segments[0].startswith("@") else segments[0]

def _scope(package_name: str) -> str | None:
    return package_name.split("/")[0] if package_name.startswith("@") else None

def _tree_packages(manifest: ArtifactValue, owner: str, exclude: list[str]) -> (list[dict[str, typing.Any]], dict[str, bool]):
    """The packages of one tree that are composed, and the scopes they cannot be projected whole from."""
    packages = []
    excluded = {}
    contents = manifest.read_json()
    split_scopes = {_scope(n): True for n in contents["ignored"] if _scope(n)}
    for package in contents["packages"]:
        package_name = package["name"]
        scope = _scope(package_name)
        excluded_as = [n for n in (package_name, scope) if n and n in exclude]
        if excluded_as:
            for n in excluded_as:
                excluded[n] = True
            if scope:
                split_scopes[scope] = True
            continue
        if "error" in package:
            fail("package '{}' in `{}` cannot be composed: {}".format(package_name, owner, package["error"]))
        if "link" in package:
            if "path" not in package:
                fail(
                    "package '{}' in `{}` is reached through a symlink to '{}', which {}; it cannot be composed (drop it with `exclude`)".format(
                        package_name,
                        owner,
                        package["link"],
                        "resolves to the tree itself or outside it" if package.get("outside") else "does not resolve to a file or directory",
                    )
                )
            if scope:
                split_scopes[scope] = True
        packages.append(package)
    for package_name in exclude:
        if package_name not in excluded:
            fail("`exclude` on `{}` names package '{}', which the tree does not provide".format(owner, package_name))
    return packages, split_scopes

def _compose_impl(
    actions: AnalysisActions,
    first_party: dict[str, str],
    manifests: list[ArtifactValue],
    name: str,
    out: OutputArtifact,
    srcs: dict[str, Artifact],
    trees: list[(Artifact, str, list[str])],
) -> list[Provider]:
    claimed = dict(first_party)
    first_party_roots = {_package_root(n): owner for n, owner in first_party.items()}
    shared_scopes = {_scope(n): True for n in first_party if _scope(n)}

    tree_packages = []
    scope_owners = {}
    for (_tree, owner, exclude), manifest in zip(trees, manifests):
        packages, split_scopes = _tree_packages(manifest, owner, exclude)
        shared_scopes.update(split_scopes)
        for package in packages:
            scope = _scope(package["name"])
            if scope:
                scope_owners.setdefault(scope, {})[owner] = True
        tree_packages.append(packages)
    for scope, owners in scope_owners.items():
        if len(owners) > 1:
            shared_scopes[scope] = True

    srcs = dict(srcs)
    claimed_by_tree = {}
    for i, ((tree, owner, _exclude), packages) in enumerate(zip(trees, tree_packages)):
        for package in packages:
            package_name = package["name"]
            scope = _scope(package_name)
            key = scope if scope and scope not in shared_scopes else package_name
            if claimed_by_tree.get(key) == i:
                continue
            existing = claimed.get(key) or first_party_roots.get(key) or (scope and first_party_roots.get(scope))
            if existing != None:
                fail("package '{}' is provided by both `{}` and `{}`; drop one with `exclude` on the `nodejs_node_modules`".format(key, existing, owner))
            claimed[key] = owner
            claimed_by_tree[key] = i
            srcs["node_modules/" + key] = tree.project(package.get("path", key))
    _copy_node_modules(actions, out, name, srcs)
    return []

_compose = dynamic_actions(
    impl = _compose_impl,
    attrs = {
        "first_party": dynattrs.value(dict[str, str]),
        "manifests": dynattrs.list(dynattrs.artifact_value()),
        "name": dynattrs.value(str),
        "out": dynattrs.output(),
        "srcs": dynattrs.value(dict[str, Artifact]),
        "trees": dynattrs.value(list[(Artifact, str, list[str])]),
    },
)
