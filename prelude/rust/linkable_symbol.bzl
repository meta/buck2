# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Example:

    buck_genrule(
        name = "my-generated-data"
        bash = "something slow",
    )

    rust_linkable_symbol(
        name = "my-generated-data-symbol",
        content_str = ":my-generated-data",  # or `content_bytes` for non-utf8
    )

    rust_binary(
        name = "whoa",
        srcs = ...,
        deps = [
            ...
            ":my-generated-data-symbol",
        ],
    )

The generated Rust library contains a get() function that returns your symbol's
data as &'static str or &'static [u8], depending on whether you used str or
bytes in the rust_linkable_symbol target.

    fn main() {
        let my_generated_data = my_generated_data_symbol::get();
        println!("{:?}", my_generated_data);
    }

The major advantage of rust_linkable_symbol over directly using include_bytes
with a mapped_srcs in your Rust target is that your slow genrule does not have
to get built when you're doing typecheck-only builds of the Rust code. That
applies to all of the following situations:

  - `arc rust-check` a.k.a. `buck2 build :whoa[check]`

  - documentation builds: `buck2 build :whoa[doc]`

  - all building performed by IDE
"""

load("@prelude//:prelude.bzl", prelude = "native")
load("@prelude//rust:link_info.bzl", "RustLinkInfo")
load("@prelude//rust:sources.bzl", "RustSources")

def _remove_rust_providers_impl(ctx: AnalysisContext) -> list[Provider]:
    out = []
    for p in ctx.attrs.base.providers:
        # Remove RustLinkInfo so the dep is treated as a C++ dep rather than
        # a Rust dep, avoiding the need to build @symbol during type checking.
        # Remove RustSources so the @symbol's transitive sources (which include
        # the potentially slow genrule content) are not pulled into downstream
        # compile actions as hidden inputs.
        if not isinstance(p, RustLinkInfo) and not isinstance(p, RustSources):
            out.append(p)
    return out

_remove_rust_providers = rule(
    impl = _remove_rust_providers_impl,
    attrs = {
        "base": attrs.dep(),
        "labels": attrs.list(attrs.string()),
    },
)

def rust_linkable_symbol(name, content_str = None, content_bytes = None, align_bytes = None, visibility = None, rust_library_macro = None):
    if (content_str == None) == (content_bytes == None):
        fail("rust_linkable_symbol requires exactly one of `content_str =` or `content_bytes =` to be passed")

    if align_bytes != None:
        if content_bytes == None:
            fail("rust_linkable_symbol's align_bytes is only supported when using content_bytes")
        if align_bytes not in [2, 4, 8]:
            fail("unsupported rust_linkable_symbol alignment")

    kind, content = ("str", content_str) if content_str else ("bytes", content_bytes)

    rust_library_macro = rust_library_macro or prelude.rust_library

    # Generate a globally unique symbol name that also works as a regular C style symbol
    # to be compatible with all flavors of linkers.
    linkable_symbol = "rust_linkable_symbol_{}__{}".format(package_name().replace("/", "_"), name)

    # Rustc shouldn't be the easiest way to accomplish this but here we are.
    #
    # Background reading:
    # https://tratt.net/laurie/blog/2022/whats_the_most_portable_way_to_include_binary_blobs_in_an_executable.html
    #
    # Maybe use `#embed` eventually (several years from now?).
    # https://www.open-std.org/jtc1/sc22/wg14/www/docs/n3017.htm
    rust_library_macro(
        name = "{}@symbol".format(name),
        crate = name,
        doctests = False,
        env = {
            "LINKABLE_SYMBOL": linkable_symbol,
        },
        labels = [
            "generated",
            "rustc_do_not_check",
        ],
        mapped_srcs = {
            "prelude//rust/tools:linkable_symbol.rs": "lib.rs",
            content: "content",
        },
        rustc_flags = [
            "--cfg=rust_linkable_symbol_content_{}".format(kind),
            '--cfg=rust_linkable_symbol_align_bytes="{}"'.format(align_bytes or 1),
            "@$(location prelude//rust/tools:linkable_symbol_supports_no_std)",
        ],
        visibility = [],
    )

    # Alias the Rust library with a rule that removes the `RustLinkInfo` and `RustSources`.
    # This causes the dependent library to be treated more like a C++ dep than a
    # Rust dep, and thereby not be needed during type checking.
    _remove_rust_providers(
        name = "{}@link".format(name),
        base = ":{}@symbol".format(name),
        labels = ["generated"],
    )

    rust_library_macro(
        name = name,
        deps = [
            ":{}@link".format(name),
        ],
        doctests = False,
        env = {
            "LINKABLE_SYMBOL": linkable_symbol,
        },
        labels = [
            "generated",
        ],
        mapped_srcs = {
            "prelude//rust/tools:linkable_symbol.rs": "lib.rs",
        },
        rustc_flags = [
            "--cfg=rust_linkable_symbol_getter_{}".format(kind),
            '--cfg=rust_linkable_symbol_align_bytes="{}"'.format(align_bytes or 1),
            # Setting `no_std` here is unconditionally fine - a panic handler will
            # be provided by whatever uses this library.
            "--cfg=set_nostd",
        ],
        visibility = visibility,
    )
