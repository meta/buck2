# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _artifacts(value: list[Artifact]):
    return value

# The `.olean` and IR files of a module and of all the modules it imports.
LeanArtifactsTSet = transitive_set(args_projections = {"artifacts": _artifacts})

# A compiled Lean module.
LeanModule = record(
    name = field(str),
    # The label of the `lean_library` that compiles the module.
    library = field(str),
    # The `LEAN_PATH` entry that contains the module's `.olean` file.
    root = field(cmd_args),
    # A `LeanArtifactsTSet`: the module's `.olean` and IR files and those of
    # all the modules it imports, transitively.
    artifacts = field(typing.Any),
    # The object of the C that `lean` generated for the module.
    object = field(Artifact),
    # The same C compiled with the `c_export_flags` of the toolchain, which
    # export every symbol of the module. `None` when the toolchain has none.
    export_object = field(Artifact | None),
)

# The value of the `LeanLibraryInfo.modules` dynamic value.
LeanModulesValue = provider(
    fields = {
        # All modules of the library and of its dependencies, by module name.
        "modules": provider_field(dict[str, LeanModule]),
        # The `LEAN_PATH` entries of the library and of its dependencies, by
        # library label, in dependency order. Each library is in it once.
        "roots": provider_field(dict[str, cmd_args]),
    }
)

LeanLibraryInfo = provider(
    fields = {
        # A `LeanModulesValue`. The modules are known only after `lean --deps-json`
        # ran, so this is a dynamic value.
        "modules": provider_field(DynamicValue),
    }
)

LeanToolchainInfo = provider(
    fields = {
        # Flags for every compilation of the C that `lean` generates.
        "c_compiler_flags": provider_field(list[typing.Any], default = []),
        # Flags that make the generated C export every symbol, for example
        # `-DLEAN_EXPORTING`. When set, `lean_library` compiles the C of each
        # module a second time with them, for shared libraries of Lean code
        # on Windows, where an executable cannot export the whole core
        # library. Empty when `c_compiler_flags` exports the symbols already.
        "c_export_flags": provider_field(list[typing.Any], default = []),
        # Preprocessor information for `lean/lean.h` and the headers it includes.
        "c_preprocessor_infos": provider_field(list[typing.Any], default = []),
        # The `lean` compiler.
        "lean": provider_field(RunInfo),
        # Flags for every `lean` invocation.
        "lean_flags": provider_field(list[typing.Any], default = []),
        # Flags for every link of a `lean_binary` or `lean_test`.
        "linker_flags": provider_field(list[typing.Any], default = []),
        # Native libraries that the generated C needs at link time, for example
        # the Lean runtime.
        "runtime": provider_field(list[Dependency], default = []),
        # `lean_library` targets that every library depends on, for example the
        # core library with `Init`. Empty when the toolchain builds the core
        # library itself.
        "stdlib": provider_field(list[Dependency], default = []),
    }
)
