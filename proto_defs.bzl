# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode//buck2:buck_rust_binary.bzl", "buck_rust_binary")
load("@fbcode_macros//build_defs:native_rules.bzl", "buck_genrule")
load("@fbsource//third-party/protobuf:protobuf_rule_helper.bzl", "protoc_exe")
load("@fbsource//tools/build_defs:rust_library.bzl", "rust_library")

def rust_protobuf_library(
    name,
    srcs,
    build_script,
    protos = None,  # Pass a list of files. Thye'll be placed in the cwd. Prefer using proto_srcs.
    deps = None,
    test_deps = None,
    doctests = True,
    build_env = None,
    proto_srcs = None,
):  # Use a proto_srcs() target, path is exposed as BUCK_PROTO_SRCS.
    _rust_protobuf_library(
        name,
        srcs,
        build_script,
        "buck2_protoc_dev",
        "0.14",
        protos,
        [
            "fbsource//third-party/rust:tonic",
            "fbsource//third-party/rust:tonic-prost",
        ]
        + (deps or []),
        test_deps,
        doctests,
        build_env,
        proto_srcs,
        None,
    )

def _rust_protobuf_library(name, srcs, build_script, buck2_protoc_dev, prost_version, protos, deps, test_deps, doctests, build_env, proto_srcs, crate_name):
    build_name = name + "-build"
    proto_name = name + "-proto"

    buck_rust_binary(
        name = build_name,
        srcs = [build_script],
        # Don't emit a `[[bin]]` in the generated Cargo.toml. Cargo
        # auto-detects `build.rs` as a build script; `buck2_protoc_dev` is
        # wired in as a build-dependency on the rust_library below.
        autocargo = {"ignore_rule": True},
        crate_root = build_script,
        deps = [
            "fbcode//buck2/app/buck2_protoc_dev:" + buck2_protoc_dev,
        ],
    )

    build_env = build_env or {}
    build_env.update({
        "PROTOC": protoc_exe(),
        "PROTOC_INCLUDE": "$(location fbsource//third-party/protobuf:google.protobuf)",
    })
    if proto_srcs:
        build_env["BUCK_PROTO_SRCS"] = "$(location {})".format(proto_srcs)

    anchor = " ".join(['case "${0}" in /*) ;; *) [ -e "${0}" ] && export {0}="$PWD/${0}";; esac;'.format(k) for k in sorted(build_env)])
    anchor_exe = "".join(["if exist %ROOT%%{0}% set {0}=%ROOT%%{0}%\n".format(k) for k in sorted(build_env)])
    buck_genrule(
        name = proto_name,
        repo_relative_root = True,
        srcs = protos,
        cmd = 'BUILD="$PWD/$(exe :{})"; export OUT="$PWD/$OUT"; {} cd "$SRCDIR" && "$BUILD" --required-for-buck1="$OUT"'.format(build_name, anchor),
        cmd_exe = 'set ROOT=%CD%\\\nset BUILD=%ROOT%$(exe :{})\nset OUT=%ROOT%$OUT\n{}cd /d "$SRCDIR" && %BUILD% --required-for-buck1=%OUT%'.format(
            build_name, anchor_exe
        ),
        env = build_env,
        out = ".",
    )

    new_deps = [
        {
            "0.14": "fbsource//third-party/rust:prost",
        }[prost_version]
    ] + (deps or [])

    rust_library(
        name = name,
        crate = crate_name or name,
        srcs = srcs,
        autocargo = {
            "cargo_toml_config": {
                "extra_buck_dependencies": {
                    "build-dependencies": [
                        "fbcode//buck2/app/buck2_protoc_dev:" + buck2_protoc_dev,
                    ],
                },
            },
        },
        doctests = doctests,
        env = {
            # This is where prost looks for generated .rs files
            "OUT_DIR": "$(location :{})".format(proto_name),
        },
        named_deps = {
            # "prost" is https://github.com/tokio-rs/prost, which is used
            # to generate Rust code from protobuf definitions.
            "generated_prost_target": ":{}".format(proto_name),
        },
        labels = [
            "generated_protobuf_library_rust",
        ],
        deps = new_deps,
        test_deps = test_deps,
        rustc_flags = ["-Aunused-crate-dependencies"],
    )

ProtoSrcsInfo = provider(fields = ["srcs"])

def _proto_srcs_impl(ctx):
    srcs = {src.basename: src for src in ctx.attrs.srcs}
    for dep in ctx.attrs.deps:
        for src in dep[ProtoSrcsInfo].srcs:
            if src.basename in srcs:
                fail("Duplicate src:", src.basename)
            srcs[src.basename] = src
    out = ctx.actions.copied_dir(ctx.attrs.name, srcs, has_content_based_path = False)
    return [DefaultInfo(default_output = out), ProtoSrcsInfo(srcs = srcs.values())]

proto_srcs = rule(
    impl = _proto_srcs_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "srcs": attrs.list(attrs.source(), default = []),
    },
)
