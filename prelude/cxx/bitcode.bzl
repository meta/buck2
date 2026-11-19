# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_common")
load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerInfo")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:utils.bzl", "value_or")
load(":cxx_context.bzl", "get_cxx_toolchain_info")

BitcodeBundle = record(
    artifact = field(Artifact),
)

BitcodeTSet = transitive_set()

BitcodeBundleInfo = provider(
    fields = {
        "bitcode": provider_field(typing.Any, default = None),
        "bitcode_bundle": provider_field(typing.Any, default = None),
    }
)

def _bundle_locally(ctx: AnalysisContext, linker_info: LinkerInfo) -> bool:
    archive_locally = linker_info.archive_objects_locally
    if hasattr(ctx.attrs, "_archive_objects_locally_override"):
        return value_or(ctx.attrs._archive_objects_locally_override, archive_locally)
    return archive_locally

def _bundle(ctx: AnalysisContext, name: str, args: cmd_args, prefer_local: bool) -> Artifact:
    llvm_link = get_cxx_toolchain_info(ctx).llvm_link
    if llvm_link == None:
        fail("Bitcode generation not supported when no LLVM linker, the `cxx_toolchain` has no `llvm_link`.")

    bundle_output = ctx.actions.declare_output(name, has_content_based_path = False)

    command = at_argfile(
        actions = ctx.actions,
        name = name + ".cxx_bitcode_argsfile",
        args = args,
        allow_args = True,
    )
    llvm_cmd = cmd_args(
        llvm_link,
        command,
        "-v",
        "-o",
        bundle_output.as_output(),
    )

    ctx.actions.run(llvm_cmd, category = "bitcode_bundle", identifier = name, prefer_local = prefer_local)
    return bundle_output

# Creates a static library given a list of object files.
def make_bitcode_bundle(ctx: AnalysisContext, name: str, objects: list[Artifact], ignore_native: bool = False, override: bool = False) -> [BitcodeBundle, None]:
    if len(objects) == 0:
        fail("no objects to archive")

    llvm_link = get_cxx_toolchain_info(ctx).llvm_link
    if llvm_link == None:
        return None

    linker_info = get_cxx_toolchain_info(ctx).linker_info

    args = cmd_args(quote = "shell")
    if ignore_native:
        args.add("--ignore-non-bitcode")

    if override and len(objects) > 1:
        args.add(objects[0])
        overrides = cmd_args(objects[1:], format = "--override={}")
        args.add(overrides)
    else:
        args.add(objects)

    bundle = _bundle(ctx, name, args, _bundle_locally(ctx, linker_info))

    return BitcodeBundle(artifact = bundle)

def llvm_link_bitcode_impl(ctx: AnalysisContext) -> list[Provider]:
    llvm_link = get_cxx_toolchain_info(ctx).llvm_link
    if llvm_link == None:
        fail("llvm-link is not provided by toolchain.")

    result = make_bitcode_bundle(ctx, ctx.attrs.name, ctx.attrs.srcs)
    if result != None:
        return [DefaultInfo(default_output = result.artifact), BitcodeBundleInfo(bitcode_bundle = result)]
    else:
        return [DefaultInfo()]

llvm_link_bitcode = prelude_rule(
    name = "llvm_link_bitcode",
    impl = llvm_link_bitcode_impl,
    docs = """
        An `llvm_link_bitcode()` rule combines a set of LLVM bitcode
        inputs into a single bitcode object using `llvm-link`.

        LLVM bitcode is the intermediate representation produced by Clang
        when compiling with `-emit-llvm`; merging multiple bitcode files
        into one allows downstream rules to perform whole-program
        optimization, link-time optimization (LTO), or other
        bitcode-level analyses on the combined module.

        Inputs can be other `llvm_link_bitcode()` outputs, raw `.bc` /
        `.o` files containing bitcode, or `cxx_library()` rules producing
        bitcode artifacts.
    """,
    examples = """
        ```
        # A rule that builds and runs C/C++ test using gtest.
        llvm_link_bitcode(
          name = 'echo_test',
          srcs = [
            'echo_test.o',  // Where this is a LLVM bitcode object.
            'echo_other.o', // And this is another LLVM bitcode object.
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        cxx_common.srcs_arg()
        | buck.deps_query_arg()
        | {
            "deps": attrs.list(attrs.dep(), default = []),
        }
        | {
            "_cxx_toolchain": toolchains_common.cxx(),
        }
    ),
)
