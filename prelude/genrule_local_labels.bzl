# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:selects.bzl", "selects")

"""
Handle labels used to opt-out genrules from running remotely.
"""

# Some rules have to be run locally for various reasons listed next to the label.
_GENRULE_LOCAL_LABELS = set([
    # Used for buck2 tests that want to run locally
    "buck2_test_local_exec",
    # Split dwarf merge rules currently don't properly list their inputs.
    "dwp",
    # Bolt and hottext post-processing rules operate on a large statically
    # linked binary which contains non-deterministic build info, meaning its
    # a) currently too large for RE to handle and b) caching it would only
    # waste cache space.
    "postprocess_bolt",
    "postprocess_hottext",
    # The iOS build needs to run a genrule locally to gather non-deterministic
    # build info from `hg`.
    "non_deterministic_build_info",
    # Some call "buck run" & "buck root" recursively.
    "uses_buck_run",
    # Some antlir and telephoto genrules use clowder for downloading from everstore
    "uses_clowder",
    # Some antlir genrules use cpio for unpacking rpms
    "uses_cpio",
    # Creates secondary Eden repos outside of `buck-out/`
    "uses_eden_mounts",
    # The Antlir core compiler uses sudo
    "uses_sudo",
    # Some rules utilize hg for some reason as part of code generation
    "uses_hg",
    # Dotslash is not yet supported on RE.
    "uses_dotslash",
    # Some rules apply a patch which is not on RE.
    "uses_patch",
    # Directly uses the smcc binary which is not on RE.
    "uses_smcc",
    # Uses shasum which is not on RE.
    "uses_shasum",
    # Uses xz which is not on RE.
    "uses_xz",
    # Uses tw tool which is not on RE.
    "uses_tw",
    # Uses thrift tool which is not on RE.
    "uses_thrift",
    # Uses protoc tool which is not on RE.
    "uses_protoc",
    # Yarn installs use a large in-repo yarn repo that's ~6.1GB at the time of
    # writing, and so v1 uses workarounds (D17359502) to avoid the overhead this
    # would causes.  So, run these rules locally to maintain compatibility and
    # until we have a better yarn solution.
    "yarn_install",
    # Non-deterministic builds that depend on data from configerator.
    "reads_configerator",
    # Third party java artifacts are stored in manifold and therefore can't be accessed from RE worker.
    "third_party_java",
    # The antlir package-at-build-time rules current rely on tools like hg/git,
    # which don't work on RE.
    "antlir_macros",
    # UPM codegen does lots of network I/O (e.g. scuba, JK, configerator), which
    # makes it fail on RE.
    "upm_binary_gen",
    # Uses the `libX11-devel` package which is not available on RE.
    "uses_x11",
    # Unity license client needs to be set up on RE workers for this to work, and maybe further debugging.
    "uses_unity",
    # mksquashfs isn't available in RE, so run these locally
    # (https://fb.workplace.com/groups/buck2users/permalink/3023630007893360/)
    "uses_mksquashfs",
    # PXL rules can't yet run on RE.
    "pxl",
    # Accesses dewey
    "uses_dewey",
    # Accesses justknobs configuration
    "justknobs",
    # Side effecting writes directly into buck-out on the local
    # filesystem
    "writes_to_buck_out",
    # Side effecting writes directly to local filesystem outside of buck-out
    # Do not add or use in new rules, just for tagging existing rules for
    # better categorization.
    "writes_outside_buck_out",
    # Calculates and writes absolute paths in the local filesystem
    "uses_local_filesystem_abspaths",
    # Use local GPUs with latest Nvidia libs which are not available in RE yet
    "uses_lower_locally",
    # Uses fbpkg outside of the repo
    "uses_fbpkg",
    # Makes recursive calls to buck
    "uses_buck",
    # Uses files in the repo that it doesn't declare as dependencies
    "uses_undeclared_inputs",
    # Connects to service router which won't work on RE
    "uses_service_router",
    # Downloads direct from manifold
    "uses_manifold",
    # When run on RE produces "Cache is out of space" (excessive disk/memory)
    "re_cache_out_of_space",
    # HHVM Post-link rules need to be local since the binary is huge.
    "hhvm_postlink",
    # Uses network access (unspecified what as of yet)
    "network_access",
    # Uses clang format which is not in RE
    "uses_clang_format",
    # Perform makes compilation in situ.
    "uses_make",
    # Like uses_make but for windows
    "uses_msbuild",
    # Like it says in the label
    "uses_mkscratch",
    # Locally built toolchains which do not exist on RE
    "toolchain_testing",
    # sphinx_wiki always needs to run locally
    "sphinx_wiki",
    # Uses R (which is feature gated) pending RE support
    "uses_rlang",
    # Uses watchman which is not in RE
    "uses_watchman",
    # Uses yumdownloader which is not in RE
    "yumdownloader",
    # Uses locally installed mvn.
    "uses_maven",
    # Some Qt genrules don't support RE yet
    "qt_moc",
    "qt_qmlcachegen",
    "qt_qrc_compile",
    "qt_qrc_gen",
    "qt_qsb_gen",
    "qt_rcc",
    "qt_uic",
    # use local jar
    "uses_jar",
    # uses local java
    "uses_java",
    # use locally installed svnyum
    "uses_svnyum",
    # uses ruby
    "uses_ruby",
    # Produces an output symlink, and since Meta RE doesn't support returning
    # output symlinks (yet?), and sometimes the output symlink also cannot be
    # dereferenced anyway (e.g. because it violates security policies or simply
    # points to something that does not exist).
    "dangling_output_symlink",
    # speech translation uses genrule to get models from manifold, high priority project
    "speech_translation_high_priority",
    # Uses Apple's codesign command which might not be in RE
    "uses_codesign",
    # Uses jf which is not on RE
    "uses_jf",
    # On Messenger Desktop few targets are massive and take much longer on RE than
    # locally to build on Windows. This is a mitigation until we can break down these
    # targets
    "zeratul_windows_capacity_hog",
    # The compilation databases produced by Buck have paths relative to the root of
    # fbsource. This isn't compatible with RE.
    "uses_compilation_database",
    # Uses checkpolicy which is not on RE
    "uses_checkpolicy",
    # Need to build sgw containers on devserver and not on RE
    # We pull base image from internet
    "sgw_build_containers",
    # Needs to pull docker image from a docker
    # registry hosted on our internal network
    "uses_docker_registry",
])

def genrule_labels_require_local(labels):
    def check_labels(labels_list):
        if labels_list == None:
            return False

        for label in labels_list:
            if selects.is_select(label):
                return selects.apply(label, lambda val: val in _GENRULE_LOCAL_LABELS if val else False)

            elif label in _GENRULE_LOCAL_LABELS:
                return True
        return False

    return selects.apply(labels, check_labels)

def resolved_genrule_labels_require_local(labels: list[str]) -> bool:
    for label in labels:
        if label in _GENRULE_LOCAL_LABELS:
            return True
    return False
