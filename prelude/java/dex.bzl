# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")

DexLibraryInfo = provider(
    # @unsorted-dict-items
    fields = {
        # the .dex.jar file. May be None if there were not any Java classes to dex. If None, the
        # remaining fields should be ignored.
        "dex": provider_field(typing.Any, default = None),  # ["artifact", None]
        # a unique string identifier for this DEX file
        "identifier": provider_field(typing.Any, default = None),  # [str, None]
        # the names of the .class files that went into the DEX file
        "class_names": provider_field(typing.Any, default = None),  # ["artifact", None]
        # resources that are referenced by the classes in this DEX file
        "referenced_resources": provider_field(typing.Any, default = None),  # ["artifact", None]
        # a value that estimates how much space the code represented by this object will take up in
        # a DEX file. The units for this estimate are not important, as long as they are consistent
        # with those used when determining how secondary DEX files should be packed.
        "weight_estimate": provider_field(typing.Any, default = None),  # ["artifact", None]
        # a file containing the exact method_ids_size, field_ids_size, and type_ids_size
        # from the DEX header. Format: "<method_ref_count> <field_ref_count> <type_ref_count>"
        # (first line, summed over the .dex.jar's dex files). These are the actual counts that
        # the 64K DEX limits apply to, unlike weight_estimate which is a byte-size proxy. When the
        # dex toolchain sets class_ref_counts, a library over a 64K limit without synthetic
        # classes also gets one "<method_ref_count> <field_ref_count> <type_ref_count> <class>"
        # line per class, counted with D8 dexing that class alone.
        "ref_count": provider_field(typing.Any, default = None),  # ["artifact", None]
        # a file mapping each synthetic class D8 created to the class it was synthesized from,
        # one "<synthetic> <context>" pair per line. A synthetic must be placed in the same dex
        # as its context class, and D8 reports the pairing because the synthetic's name is
        # mangled in a format it does not treat as stable.
        "synthetic_contexts": provider_field(typing.Any, default = None),  # ["artifact", None]
    },
)

def get_dex_produced_from_java_library(
    ctx: AnalysisContext,
    dex_toolchain: DexToolchainInfo,
    jar_to_dex: Artifact,
    needs_desugar: bool = False,
    desugar_deps: [TransitiveSetArgsProjection, None] = None,
    weight_factor: int = 1,
) -> DexLibraryInfo:
    d8_cmd = cmd_args(dex_toolchain.d8_command[RunInfo])

    library_path = jar_to_dex.short_path
    prefix = "dex/{}".format(library_path)
    output_dex_file = ctx.actions.declare_output(prefix + ".dex.jar", has_content_based_path = True)
    d8_cmd.add(["--output-dex-file", output_dex_file.as_output()])

    d8_cmd.add(["--file-to-dex", jar_to_dex])
    d8_cmd.add(["--android-jar", dex_toolchain.android_jar])

    d8_cmd.add(["--intermediate", "--no-optimize", "--force-jumbo"])
    if not needs_desugar:
        d8_cmd.add("--no-desugar")
    else:
        desugar_deps_file = ctx.actions.write(prefix + "_desugar_deps_file.txt", desugar_deps or [], has_content_based_path = True)
        d8_cmd.add(["--classpath-files", desugar_deps_file])
        d8_cmd.add(cmd_args(hidden = desugar_deps or []))

    referenced_resources_file = ctx.actions.declare_output(prefix + "_referenced_resources.txt", has_content_based_path = True)
    d8_cmd.add(["--referenced-resources-path", referenced_resources_file.as_output()])

    weight_estimate_file = ctx.actions.declare_output(prefix + "_weight_estimate.txt", has_content_based_path = True)
    d8_cmd.add(["--weight-estimate-path", weight_estimate_file.as_output()])

    d8_cmd.add(["--weight-factor", str(weight_factor)])

    class_names_file = ctx.actions.declare_output(prefix + "_class_names.txt", has_content_based_path = True)
    d8_cmd.add(["--class-names-path", class_names_file.as_output()])

    ref_count_file = ctx.actions.declare_output(prefix + "_ref_count.txt", has_content_based_path = True)
    d8_cmd.add(["--ref-count-path", ref_count_file.as_output()])
    if dex_toolchain.class_ref_counts:
        d8_cmd.add("--class-ref-counts")

    synthetic_contexts_file = ctx.actions.declare_output(prefix + "_synthetic_contexts.txt", has_content_based_path = True)
    d8_cmd.add(["--synthetic-contexts-path", synthetic_contexts_file.as_output()])

    min_sdk_version = ctx.attrs.min_sdk_version
    if min_sdk_version:
        d8_cmd.add(["--min-sdk-version", str(min_sdk_version)])

    identifier = "{}:{} {}".format(ctx.label.package, ctx.label.name, output_dex_file.short_path)
    ctx.actions.run(
        d8_cmd,
        category = "pre_dex",
        identifier = identifier,
        allow_cache_upload = True,
    )

    return DexLibraryInfo(
        dex = output_dex_file,
        identifier = identifier,
        class_names = class_names_file,
        referenced_resources = referenced_resources_file,
        weight_estimate = weight_estimate_file,
        ref_count = ref_count_file,
        synthetic_contexts = synthetic_contexts_file,
    )
