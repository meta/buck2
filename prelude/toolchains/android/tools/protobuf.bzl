# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:native.bzl", "native")
load("@prelude//toolchains/android/tools:build_rules.bzl", "buck_java_library")

def protobuf_src_gen(name, srcs, proto_path = [], deps = [], exported_deps = []):
    common_args = [
        "--java_opt=annotate_code",
        "--proto_path=$SRCDIR",
        "--java_out=$OUT --grpc-java_out=$OUT $SRCS".format(name, name),
    ] + ["--proto_path={}".format(path) for path in proto_path]

    command = [
        "mkdir $OUT",
        "&& $(exe prelude//toolchains/android/tools/protobuf:protoc)",
        "--plugin=protoc-gen-grpc-java=$(location_exec prelude//toolchains/android/tools/protobuf:gen-grpc)",
    ] + common_args

    command_exe = [
        "mkdir $OUT",
        "&& copy $(location_exec prelude//toolchains/android/tools/protobuf:gen-grpc-windows) $TMP\\gen-grpc",
        "&& $(exe prelude//toolchains/android/tools/protobuf:protoc)",
        "--plugin=protoc-gen-grpc-java=$TMP\\gen-grpc",
    ] + common_args

    genrule_name = name + "-src-gen"
    native.genrule(
        name = genrule_name,
        srcs = srcs,
        out = name,
        cmd = " ".join(command),
        cmd_exe = " ".join(command_exe),
        exec_compatible_with = ["config//cpu:x86_64"],
    )

    zip_rule_name = name + ".src.zip"
    native.zip_file(
        name = zip_rule_name,
        srcs = [":" + genrule_name],
        entries_to_exclude = [".*\\.pb\\.meta"],
        out = name + ".src.zip",
    )

    buck_java_library(
        name = name,
        srcs = [":" + zip_rule_name],
        visibility = ["PUBLIC"],
        deps = deps,
        exported_deps = exported_deps,
    )
