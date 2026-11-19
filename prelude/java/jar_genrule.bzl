# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:genrule.bzl", "genrule_attributes", "process_genrule")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//utils:expect.bzl", "expect")

def jar_genrule_impl(ctx: AnalysisContext) -> list[Provider]:
    output_name = "{}.jar".format(ctx.label.name)
    providers = process_genrule(ctx, output_name, None)
    expect(
        len(providers) == 1,
        "expected exactly one provider of type DefaultInfo from {} ({})".format(ctx.label.name, providers),
    )

    default_info = providers[0]  # DefaultInfo type
    outputs = default_info.default_outputs
    expect(
        len(outputs) == 1,
        "expected exactly one output from {} ({})".format(ctx.label.name, outputs),
    )
    output_jar = outputs[0]

    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    java_cmd = cmd_args(java_toolchain.java[RunInfo])
    java_cmd.add("-jar", output_jar)

    providers.append(RunInfo(args = java_cmd))
    return providers

jar_genrule = prelude_rule(
    name = "jar_genrule",
    impl = jar_genrule_impl,
    docs = """
        A `jar_genrule()` is a variant of `genrule()` whose output is
        packaged into a JAR file. Use it to produce a JAR from arbitrary
        commands (`bash`/`cmd`/`cmd_exe`) that operate on `srcs`.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "bash": attrs.option(attrs.arg(), default = None),
            "cacheable": attrs.option(attrs.bool(), default = None),
            "cmd": attrs.option(attrs.arg(), default = None),
            "cmd_exe": attrs.option(attrs.arg(), default = None),
            "enable_sandbox": attrs.option(attrs.bool(), default = None),
            "environment_expansion_separator": attrs.option(attrs.string(), default = None),
            "weight": attrs.option(attrs.int(), default = None),
            "remote": attrs.option(attrs.bool(), default = None),
            "srcs": attrs.named_set(attrs.source(), sorted = False, default = []),
            "type": attrs.option(attrs.string(), default = None),
        }
        | genrule_attributes()
        | {
            "_java_toolchain": toolchains_common.java(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
