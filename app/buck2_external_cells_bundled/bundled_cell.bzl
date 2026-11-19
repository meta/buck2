# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:source_listing.bzl", "SourceListingInfo")

def _impl(ctx: AnalysisContext) -> list[Provider]:
    files = {}

    parts = ["pub(crate) const DATA: &[crate::BundledFile] = &["]
    for path, art in ctx.attrs.source_listing[SourceListingInfo].sources.items():
        exec_bit_rel_path = "includes/" + path + "/__exec_bit.txt"
        data_rel_path = "includes/" + path + "/__data"

        # In the future, this can be extended to additionally compress the data
        exec_bit = ctx.actions.declare_output("processed/" + path + "/__exec_bit.txt", has_content_based_path = False)
        ctx.actions.run(
            cmd_args(
                ctx.attrs._processor[RunInfo],
                art,
                exec_bit.as_output(),
            ),
            category = "process_source_file",
            identifier = "process " + path,
        )
        files[exec_bit_rel_path] = exec_bit
        files[data_rel_path] = art

        parts.append("crate::BundledFile {")
        parts.append('  path: "' + path + '",')
        parts.append('  contents: include_bytes!("' + data_rel_path + '"),')
        parts.append('  is_executable: include!("' + exec_bit_rel_path + '"),')
        parts.append("},")

    parts.append("];")

    contents = ctx.actions.write("contents.rs", cmd_args(parts, delimiter = "\n"), with_inputs = True, has_content_based_path = False)
    files["contents.rs"] = contents

    out = ctx.actions.symlinked_dir("out", files, has_content_based_path = False)

    return [DefaultInfo(default_output = out)]

# This rule outputs a default info with a section of a Rust source file that
# contains a `&[BundledFile]`. It gets supplied as a `mapped_src`, immediately
# adjacent to the `include_from_file`.
_bundled_cell = rule(
    impl = _impl,
    attrs = {
        "include_from_file": attrs.source(),
        "source_listing": attrs.dep(),
        "_processor": attrs.exec_dep(),
    },
)

def bundled_cell(**kwargs):
    _bundled_cell(_processor = ":processor", **kwargs)
