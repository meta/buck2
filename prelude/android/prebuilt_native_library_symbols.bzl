# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//android:android_providers.bzl",
    "PrebuiltNativeLibraryDir",  # @unused Used as a type
)
load("@prelude//android:cpu_filters.bzl", "CPU_FILTER_TO_ABI_DIRECTORY")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

# objdump's name for each platform's ELF format. A library of another machine
# (a DSP library some SDKs ship in jni/) is not loaded by the app's linker.
_OBJDUMP_FORMAT = {
    "arm64": "elf64-littleaarch64",
    "armv7": "elf32-littlearm",
    "x86": "elf32-i386",
    "x86_64": "elf64-x86-64",
}

_SCRIPT = """\
set -uo pipefail
records="$1"; objdump="$2"; nm="$3"; abi="$4"; format="$5"; shift 5
: > "$records"
for dir in "$@"; do
  for lib in "$dir/$abi"/*.so; do
    [ -f "$lib" ] || continue
    name="$(basename -- "$lib")"
    if ! headers="$("$objdump" -p "$lib" 2>/dev/null)"; then
      printf 'unreadable %s\\n' "$name" >> "$records"
      continue
    fi
    if ! printf '%s\\n' "$headers" | grep -q "file format $format\\$"; then
      printf 'foreign %s\\n' "$name" >> "$records"
      continue
    fi
    if ! symbols="$("$nm" -D -P -g "$lib" 2>/dev/null)"; then
      printf 'unreadable %s\\n' "$name" >> "$records"
      continue
    fi
    printf 'library %s\\n' "$name" >> "$records"
    printf '%s\\n' "$headers" | sed -n 's/^ *NEEDED *//p' | sed 's/^/needed /' >> "$records"
    printf '%s\\n' "$symbols" | awk 'NF >= 2 { n = $1; sub(/@.*/, "", n); if ($2 == "U") print "import " n; else if ($2 != "w" && $2 != "v") print "export " n }' >> "$records"
  done
done
"""

def prebuilt_native_library_symbols(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    platform: str,
    dirs: list[PrebuiltNativeLibraryDir],
) -> Artifact | None:
    """Reads the shared libraries in `dirs` (`prebuilt_native_library`, an
    AAR's `jni/`) for `platform`'s ABI. They are packaged as files, so no relink
    records what they import.

    The returned file has, per library in directory order, `library <file
    name>`, then `needed <soname>` in DT_NEEDED order, `import <symbol>` per
    strong undefined dynamic symbol and `export <symbol>` per defined one. A
    file objdump or nm cannot read is `unreadable <file name>`, and one built
    for another machine `foreign <file name>`. None when there is no
    directory, or no objdump, nm or bash to read them with.
    """
    objdump = cxx_toolchain.binary_utilities_info.objdump
    nm = cxx_toolchain.binary_utilities_info.nm
    exec_is_windows = hasattr(ctx.attrs, "_exec_os_type") and ctx.attrs._exec_os_type[OsLookup].os == Os("windows")
    if not dirs or objdump == None or nm == None or exec_is_windows or platform not in _OBJDUMP_FORMAT:
        return None
    records = ctx.actions.declare_output(paths.join("prebuilt_native_library_symbols", platform), has_content_based_path = False)
    ctx.actions.run(
        [
            "/usr/bin/env",
            "bash",
            "-c",
            _SCRIPT,
            "",
            records.as_output(),
            objdump,
            nm,
            CPU_FILTER_TO_ABI_DIRECTORY[platform],
            _OBJDUMP_FORMAT[platform],
        ]
        + [d.dir for d in dirs],
        category = "prebuilt_native_library_symbols",
        identifier = platform,
    )
    return records

def parse_prebuilt_native_library_symbols(text: str) -> tuple:
    """Returns (libraries, unreadable, foreign): each library a struct of
    `name`, `needed` (a list), `imports` and `exports` (dicts of symbol to
    True); the others lists of file names."""
    libraries = []
    unreadable = []
    foreign = []
    current = None
    for line in text.splitlines():
        kind, _, value = line.partition(" ")
        if kind == "library":
            current = struct(name = value, needed = [], imports = {}, exports = {})
            libraries.append(current)
        elif kind == "unreadable":
            unreadable.append(value)
        elif kind == "foreign":
            foreign.append(value)
        elif kind == "needed":
            current.needed.append(value)
        elif kind == "import":
            current.imports[value] = True
        elif kind == "export":
            current.exports[value] = True
    return (libraries, unreadable, foreign)

def charge_prebuilt_dir_imports(
    ctx: AnalysisContext,
    name: str,
    records: Artifact,
    provided_symbols: dict[str, Artifact],
    chargeable: list[str],
) -> Artifact:
    """Writes, one per line, the strong imports of the libraries `records`
    describes that bind in a library named in `chargeable`.

    Each import binds in the first of its library's DT_NEEDED libraries that
    exports it: another library of the directories, or an app library, whose
    exports before any relink `provided_symbols` holds by soname. System
    libraries' exports are not read, so the walk passes over them. Only the
    app libraries some library names in DT_NEEDED are read.
    """
    output = ctx.actions.declare_output(name, has_content_based_path = False)
    chargeable_names = {soname: True for soname in chargeable}

    def read_records(ctx, artifacts, outputs):
        libraries = parse_prebuilt_native_library_symbols(artifacts[records].read_string())[0]
        providers = {dep: True for lib in libraries for dep in lib.needed if dep in provided_symbols}
        if not providers:
            ctx.actions.write(outputs[output], "")
            return

        def charge(ctx, artifacts, outputs):
            provided = {dep: {s: True for s in artifacts[provided_symbols[dep]].read_string().splitlines()} for dep in providers}
            dir_exports = {lib.name: lib.exports for lib in libraries}
            charged = {}
            for lib in libraries:
                for symbol in lib.imports:
                    for dep in lib.needed:
                        if symbol in dir_exports.get(dep, {}):
                            break
                        if symbol in provided.get(dep, {}):
                            if dep in chargeable_names:
                                charged[symbol] = True
                            break
            ctx.actions.write(outputs[output], sorted(charged.keys()))

        ctx.actions.dynamic_output(
            dynamic = [provided_symbols[dep] for dep in providers],
            inputs = [],
            outputs = [outputs[output].as_output()],
            f = charge,
        )

    ctx.actions.dynamic_output(dynamic = [records], inputs = [], outputs = [output.as_output()], f = read_records)
    return output
