# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _proto_python_library_impl(ctx: AnalysisContext) -> list[Provider]:
    prefix = ctx.label.package
    depth = len(prefix.split("/"))
    libname = ctx.attrs.src.basename.removesuffix(".proto") + "_pb2"
    python_out = ctx.actions.declare_output(prefix, "{}.py".format(libname), has_content_based_path = False)
    pyi_out = ctx.actions.declare_output(prefix, "{}.pyi".format(libname), has_content_based_path = False)
    grpc_python_out = ctx.actions.declare_output(prefix, "{}_grpc.py".format(libname), has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            ctx.attrs._protoc[RunInfo],
            cmd_args(ctx.attrs.src, format = "-I{}={}", parent = 1),
            cmd_args(python_out.as_output(), format = "--python_out={}", parent = depth + 1),
            cmd_args(pyi_out.as_output(), format = "--pyi_out={}", parent = depth + 1),
            cmd_args(grpc_python_out.as_output(), format = "--grpc_python_out={}", parent = depth + 1),
            ctx.attrs.src,
        ),
        category = "protoc",
    )

    # protoc does not let us control the import prefix and path prefix separately.
    # So, we need to copy the generated files into the correct location after the fact.
    python_out_copied = ctx.actions.declare_output("{}.py".format(libname), has_content_based_path = False)
    pyi_out_copied = ctx.actions.declare_output("{}.pyi".format(libname), has_content_based_path = False)
    grpc_python_out_copied = ctx.actions.declare_output("{}_grpc.py".format(libname), has_content_based_path = False)
    ctx.actions.copy_file(python_out_copied, python_out)
    ctx.actions.copy_file(pyi_out_copied, pyi_out)
    ctx.actions.copy_file(grpc_python_out_copied, grpc_python_out)
    return [DefaultInfo(default_outputs = [python_out_copied, pyi_out_copied, grpc_python_out_copied])]

_proto_python_library = rule(
    impl = _proto_python_library_impl,
    attrs = {
        "src": attrs.source(),
        "_protoc": attrs.exec_dep(default = "//proto:protoc", providers = [RunInfo]),
    },
)

def proto_python_library(*, name, src, **kwargs):
    _proto_python_library(
        name = "{}-gen".format(name),
        src = src,
    )
    native.python_library(name = name, srcs = [":{}-gen".format(name)], deps = ["//python:grpcio"], **kwargs)
