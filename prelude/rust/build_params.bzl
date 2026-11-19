# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Rules for mapping requirements to options

load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkStrategy",
)
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//utils:expect.bzl", "expect")

# --crate-type=
# Excludes `lib` because we want to explicitly choose the library flavour
CrateType = enum(
    # Binary
    "bin",
    # Rust linkage
    "rlib",
    "dylib",
    "proc-macro",
    # Native linkage
    "cdylib",
    "staticlib",
)

# Crate type requires linking
def crate_type_linked(crate_type: CrateType) -> bool:
    return crate_type.value in ("bin", "dylib", "proc-macro", "cdylib")

# -Crelocation-model= from --print relocation-models
RelocModel = enum(
    # Common
    "static",
    "pic",
    "pie",
    # Various obscure types
    "dynamic-no-pic",
    "ropi",
    "rwpi",
    "ropi-rwpi",
    "default",
)

# --emit=
Emit = enum(
    "asm",
    "llvm-bc",
    "llvm-ir",
    "llvm-ir-noopt",
    "obj",
    # A real link, performed by rustc: non-AUL binaries and dylibs/cdylibs,
    # proc macros (which rustc itself loads), and staticlib bundling (not a
    # true link, but like one in that it pulls together all dependencies).
    "link",
    # The crate's static-library-shaped artifact. For `rlib` crates this is
    # the rlib itself. For `bin` crates (AUL only) rustc compiles exactly as
    # for `link`, but the extraction wrapper captures the objects it would
    # have handed the linker.
    "rlib",
    "dep-info",
    "mir",
    "expand",  # pseudo emit alias for -Zunpretty=expanded
    "clippy",
    "miri",
    # Rustc actually has two different forms of metadata:
    #  - The full flavor, which is what's outputted when passing
    #    `--emit link,metadata` and can be used as a part of pipelined builds
    #  - The fast flavor, which is emitted from `--emit metadata`, is faster to
    #    build, but cannot be used in pipelined builds.
    "metadata-full",
    "metadata-fast",
)

ProfileMode = enum(
    "llvm-time-trace",
    "self-profile",
    "remarks",
)

# The different quantities of Rust metadata that can be requested from
# dependencies. Each one corresponds to an `Emit` variant, but not all `Emit`
# variants output metadata
MetadataKind = enum(
    "fast",
    "full",
    "link",
    "miri",
)

# Emitting this artifact generates code
def dep_metadata_of_emit(emit: Emit) -> MetadataKind:
    return {
        Emit("asm"): MetadataKind("full"),
        Emit("llvm-bc"): MetadataKind("full"),
        Emit("llvm-ir"): MetadataKind("full"),
        Emit("llvm-ir-noopt"): MetadataKind("full"),
        Emit("obj"): MetadataKind("full"),
        Emit("link"): MetadataKind("link"),
        # rlibs are consumed as archives by cxx-driven links, never loaded by
        # a downstream rustc link, so their deps never need real object code.
        Emit("rlib"): MetadataKind("full"),
        Emit("mir"): MetadataKind("full"),
        Emit("metadata-fast"): MetadataKind("fast"),
        Emit("clippy"): MetadataKind("fast"),
        Emit("dep-info"): MetadataKind("fast"),
        Emit("expand"): MetadataKind("fast"),
        Emit("metadata-full"): MetadataKind("full"),
        Emit("miri"): MetadataKind("miri"),
    }[emit]

# Represents a way of invoking rustc to produce an artifact. These values are computed from
# information such as the rule type, linkstyle, crate type, etc.
BuildParams = record(
    crate_type = field(CrateType),
    reloc_model = field(RelocModel),
    dep_link_strategy = field(LinkStrategy),
    # A prefix and suffix to use for the name of the produced artifact. Note that although we store
    # these in this type, they are in principle computable from the remaining fields and the OS.
    # Keeping them here just turns out to be a little more convenient.
    prefix = field(str),
    suffix = field(str),
)

# Controls how we build our rust libraries, largely dependent on whether rustc
# or buck is driving the final linking and whether we are linking the artifact
# into other rust targets.
#
# Rust: In this mode, we build standard rlibs/dylibs. This is the approach that
# we use either when the artifacts will subsequently be consumed by rust, or
# when `advanced_unstable_linking` is enabled (or both). In the
# `advanced_unstable_linking` case, this approach mitigates the duplicate symbol
# downside of the "Native" approach. However, this option is not formally
# supported by rustc, and depends on an implementation detail of rlibs (they're
# effectively .a archives and can be linked with other native code using the CXX
# linker).
#
# See https://github.com/rust-lang/rust/issues/73632 for more details on
# stabilizing this approach.
#
# native-bundled: In this mode, we build rust libraries as staticlibs, where
# rustc will bundle all of this target's rust dependencies into a single library
# artifact. This approach is the most standardized OSS way to build rust
# libraries for linkage in non-rust code.

LinkageLang = enum(
    "rust",
    "native-bundled",
)

def _executable_prefix_suffix(linker_type: LinkerType, target_os_type: OsLookup) -> (str, str):
    return {
        LinkerType("darwin"): ("", ""),
        LinkerType("gnu"): ("", ".exe") if target_os_type.os == Os("windows") else ("", ""),
        LinkerType("wasm"): ("", ".wasm"),
        LinkerType("windows"): ("", ".exe"),
    }[linker_type]

def _library_prefix_suffix(linker_type: LinkerType, target_os_type: OsLookup) -> (str, str):
    return {
        LinkerType("darwin"): ("lib", ".dylib"),
        LinkerType("gnu"): ("", ".dll") if target_os_type.os == Os("windows") else ("lib", ".so"),
        LinkerType("wasm"): ("", ".wasm"),
        LinkerType("windows"): ("", ".dll"),
    }[linker_type]

# How a library crate is built for one `(proc_macro, LibOutputStyle, LinkageLang)` combination.
_LibraryFlags = record(
    crate_type = field(CrateType),
    platform_to_affix = field(typing.Callable),
    # FIXME(JakobDegen): We deal with Rust needing to know the link strategy
    # even for building archives by using a default link strategy specifically
    # for those cases. I've gone through the code and checked all the places
    # where the link strategy is used to determine that this won't do anything
    # too bad, but it would be nice to enforce that more strictly or not have
    # this at all.
    dep_link_strategy = field(LinkStrategy),
)

_RUST_PROC_MACRO = _LibraryFlags(
    crate_type = CrateType("proc-macro"),
    platform_to_affix = _library_prefix_suffix,
    # FIXME(JakobDegen): It's not really clear what we should do about
    # proc macros. The principled thing is probably to treat them sort
    # of like a normal library, except that they always have preferred
    # linkage shared? Preserve existing behavior for now
    dep_link_strategy = LinkStrategy("static_pic"),
)

_LIBRARY_FLAGS = {
    # Native linkable shared object
    (False, LibOutputStyle("shared_lib"), LinkageLang("native-bundled")): _LibraryFlags(
        crate_type = CrateType("cdylib"),
        platform_to_affix = _library_prefix_suffix,
        # cdylibs statically link all rust code and export a single C-style dylib
        # for consumption by other languages
        dep_link_strategy = LinkStrategy("static_pic"),
    ),
    # Rust dylib shared object
    (False, LibOutputStyle("shared_lib"), LinkageLang("rust")): _LibraryFlags(
        crate_type = CrateType("dylib"),
        platform_to_affix = _library_prefix_suffix,
        dep_link_strategy = LinkStrategy("shared"),
    ),
    # Rust proc-macro, built the same way whatever output style is asked for
    (True, LibOutputStyle("archive"), LinkageLang("rust")): _RUST_PROC_MACRO,
    (True, LibOutputStyle("pic_archive"), LinkageLang("rust")): _RUST_PROC_MACRO,
    (True, LibOutputStyle("shared_lib"), LinkageLang("rust")): _RUST_PROC_MACRO,
    # FIXME(JakobDegen): Add a comment explaining why `.a`s need reloc-strategy
    # dependent names while `.rlib`s don't.
    # Rust static_pic library
    (False, LibOutputStyle("pic_archive"), LinkageLang("rust")): _LibraryFlags(
        crate_type = CrateType("rlib"),
        platform_to_affix = lambda _l, _t: ("lib", ".rlib"),
        dep_link_strategy = LinkStrategy("static_pic"),
    ),
    # Rust static (non-pic) library
    (False, LibOutputStyle("archive"), LinkageLang("rust")): _LibraryFlags(
        crate_type = CrateType("rlib"),
        platform_to_affix = lambda _l, _t: ("lib", ".rlib"),
        dep_link_strategy = LinkStrategy("static"),
    ),
    # Native linkable static_pic
    (False, LibOutputStyle("pic_archive"), LinkageLang("native-bundled")): _LibraryFlags(
        crate_type = CrateType("staticlib"),
        platform_to_affix = lambda _l, _t: ("lib", "_pic.a"),
        dep_link_strategy = LinkStrategy("static_pic"),
    ),
    # Native linkable static non-pic
    (False, LibOutputStyle("archive"), LinkageLang("native-bundled")): _LibraryFlags(
        crate_type = CrateType("staticlib"),
        platform_to_affix = lambda _l, _t: ("lib", ".a"),
        dep_link_strategy = LinkStrategy("static"),
    ),
}

def _get_reloc_model(link_strategy: LinkStrategy, target_os_type: OsLookup, binary: bool) -> RelocModel:
    if target_os_type.os == Os("windows"):
        return RelocModel("pic")
    if link_strategy == LinkStrategy("static"):
        return RelocModel("static")
    if binary:
        return RelocModel("pie")
    return RelocModel("pic")

# Build params for a `bin` crate: `rust_binary`, `rust_test`, and the rustdoc
# test harness of a `rust_library`. The link strategy is the target's own
# choice (its `link_style` attribute), unlike for libraries, where it follows
# from how the library is consumed.
def binary_build_params(
    proc_macro: bool,
    link_strategy: LinkStrategy,
    linker_type: LinkerType,
    target_os_type: OsLookup,
) -> BuildParams:
    prefix, suffix = _executable_prefix_suffix(linker_type, target_os_type)
    return BuildParams(
        # It's complicated: a proc macro here is the rustdoc test of a
        # procedural macro crate. We need deps built as if this were a binary,
        # while passing crate-type proc_macro to the rustdoc invocation.
        crate_type = CrateType("proc-macro") if proc_macro else CrateType("bin"),
        reloc_model = _get_reloc_model(link_strategy, target_os_type, binary = True),
        dep_link_strategy = link_strategy,
        prefix = prefix,
        suffix = suffix,
    )

# Compute crate type, relocation model and name mapping for a library, given whether it's a
# proc-macro, how it is to be consumed, and the language of the consumer.
#
# The linking information that's passed here is different from what one might expect in the C++
# rules. There's a good reason for that, so let's go over it. First, let's recap how C++ handles
# this, as of December 2023 (I say "recap" but I don't think this is actually documented anywhere):
#
#  1. C++ libraries can be built in three different ways: Archives, pic archives, and shared
#     libraries. Which one of these is used for a given link strategy is determined by the preferred
#     linkage using `linking/link_info.bzl:get_lib_output_style`.
#  2. When a C++ library is built as a shared library, the link strategy used for its dependencies
#     is determined by the link style attribute on the C++ library.
#  3. When a C++ library is built as an archive (either kind), there's no need to know a link
#     strategy for the dependencies. None of the per-link-strategy providers of the dependencies
#     need to be accessed.
#
# There are two relevant ways in which Rust differs:
#
#  1. There are more ways of building Rust libraries than are represented by `LibOutputStyle`. The
#     Rust analogue is the `BuildParams` type, which implicitly holds a `LibOutputStyle` as well as
#     a bunch of additional information - this is why `LibOutputStyle` is relatively rarely used
#     directly in the Rust rules.
#  2. Rust does not have the property in point three above, ie building a Rust library into an
#     archive does require knowing per-link-strategy properties of the dependencies. This is
#     fundamental in cases without native unbundled deps - with native unbundled deps it may be
#     fixable, but that's not super clear.
def library_build_params(
    proc_macro: bool,
    lib_output_style: LibOutputStyle,
    lang: LinkageLang,
    linker_type: LinkerType,
    target_os_type: OsLookup,
) -> BuildParams:
    key = (proc_macro, lib_output_style, lang)
    expect(
        key in _LIBRARY_FLAGS,
        "missing case for proc_macro={} lib_output_style={} lang={}",
        proc_macro,
        lib_output_style,
        lang,
    )
    flags = _LIBRARY_FLAGS[key]
    prefix, suffix = flags.platform_to_affix(linker_type, target_os_type)

    return BuildParams(
        crate_type = flags.crate_type,
        reloc_model = _get_reloc_model(flags.dep_link_strategy, target_os_type, binary = False),
        dep_link_strategy = flags.dep_link_strategy,
        prefix = prefix,
        suffix = suffix,
    )
