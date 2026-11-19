# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "ArtifactOutputs")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type

# Resources for transitive deps, shared by C++ and Rust.
ResourceInfo = provider(
    fields = {
        # A map containing all resources from transitive dependencies.  The keys
        # are rule labels and the values are maps of resource names (the name used
        # to lookup the resource at runtime) and the actual resource artifact.
        "resources": provider_field(dict[Label, dict[str, ArtifactOutputs]]),
    }
)

_EMPTY_RESOURCE_INFO = ResourceInfo(resources = {})

def make_resource_info(resources: dict[Label, dict[str, ArtifactOutputs]]) -> ResourceInfo:
    return _EMPTY_RESOURCE_INFO if not resources else ResourceInfo(resources = resources)

def create_relocatable_resources_info(
    ctx: AnalysisContext, name: str, resources: dict[str, ArtifactOutputs], has_content_based_path: bool = False
) -> [ArgLike, Artifact]:
    """
    Generate a resource DB (a JSON map of resource to its relative path) for the given
    binary in a way that is usable in relocatable contexts (where binaries must be fully
    self-contained). Paths in the DB are relative to the dir containing the binary, and
    point to a local dir that contains symlinks to the resources.

    Resources are packaged in a structure like:
      binary_name                                 (main executable)
      binary_name.resources.json                  (manifest pointing to the resources)
      __binary_name__resources/                   (dir containing resource files)
        resource_name_1/resource_file_1 -> ../../foo
        resource_name_2/resource_file_2 -> ../../bar
    """

    resources_dir_mapping = {}
    packaged_resource_name_to_json = {}

    # The name of the single resources directory for this binary
    resources_dir_name = "__{}__resources".format(name)
    for resource_name, resource_output in resources.items():
        # Sanitize resource name to create a valid subdir
        sanitized_resource_name = resource_name.replace("/", "__")

        # Create path within resources directory: <resource_name>/<filename>
        resource_filename = resource_output.default_output.basename
        resource_subdir_path = "{}/{}".format(sanitized_resource_name, resource_filename)

        resources_dir_mapping[resource_subdir_path] = resource_output.default_output

        # Update JSON to point to the packaged location, where path is relative to the
        # binary: (i.e. ./__binary_name__resources__/<resource_name>/<filename>)
        packaged_resource_name_to_json[resource_name] = "{}/{}".format(
            resources_dir_name,
            resource_subdir_path,
        )

    resources_dir = ctx.actions.symlinked_dir(
        resources_dir_name,
        resources_dir_mapping,
        has_content_based_path = has_content_based_path,
    )

    packaged_resources_json = ctx.actions.write_json(
        name + ".resources_pkg.json",
        packaged_resource_name_to_json,
        has_content_based_path = has_content_based_path,
    )

    return (packaged_resources_json, resources_dir)

def gather_resources(label: Label, resources: dict[str, ArtifactOutputs] = {}, deps: list[Dependency] = []) -> dict[Label, dict[str, ArtifactOutputs]]:
    """
    Return the resources for this rule and its transitive deps.
    """

    all_resources = {}

    # Resources for self.
    if resources:
        all_resources[label] = resources

    # Merge in resources for deps.
    for dep in deps:
        if ResourceInfo in dep:
            all_resources.update(dep[ResourceInfo].resources)

    return all_resources

def create_resource_db(ctx: AnalysisContext, name: str, binary: Artifact, resources: dict[str, ArtifactOutputs]) -> Artifact:
    """
    Generate a resource DB for resources for the given binary, relativized to
    the binary's working directory.
    """

    db = {name: cmd_args(resource.default_output, delimiter = "", relative_to = (binary, 1)) for (name, resource) in resources.items()}
    return ctx.actions.write_json(name, db, has_content_based_path = False)
