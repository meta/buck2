# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import argparse
import subprocess
import sys
import sysconfig
from pathlib import Path
from shutil import which


def find_libpython() -> Path:
    """The shared libpython of this interpreter.

    `LDLIBRARY` is relative to a different directory on each layout: `LIBDIR`
    for a plain install, the multiarch directory under it on Debian and Ubuntu,
    and `PYTHONFRAMEWORKPREFIX` for a macOS framework build, where it reads
    `Python.framework/Versions/X.Y/Python`.
    """
    config = sysconfig.get_config_var
    names = [str(config(name)) for name in ("LDLIBRARY", "INSTSONAME") if config(name)]
    dirs = [
        Path(str(config("LIBDIR") or "")),
        Path(str(config("LIBDIR") or "")) / str(config("MULTIARCH") or ""),
        Path(str(config("PYTHONFRAMEWORKPREFIX") or "")),
        Path(str(config("installed_platbase"))) / str(config("platlibdir")),
    ]
    candidates = [d / n for d in dirs if str(d) not in ("", ".") for n in names]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    tried = ", ".join(str(c) for c in candidates)
    raise RuntimeError(f"No shared libpython found; tried {tried}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--nm", type=Path, help="Path to nm tool")
    parser.add_argument("output", help="Output file path")
    args = parser.parse_args()

    if sys.platform == "win32":
        # Windows links against an import library, and the linker needs no
        # flags to keep libpython's symbols.
        open(args.output, "w").close()
        return

    nm: Path | None = args.nm
    if nm is None:
        maybe_nm = which("nm")
        if not maybe_nm:
            raise ValueError("nm tool was not specified and also not available on PATH")
        nm = Path(maybe_nm)
    if not nm.exists():
        raise ValueError(f"nm tool not found at specified location: {nm!r}")

    libpython = find_libpython()

    # Run nm on the library file
    nm_output = subprocess.check_output([nm, libpython], text=True)

    # Process the output and write linker args
    with open(args.output, "w") as f:
        for line in nm_output.splitlines():
            # Look for lines containing ' T ' which indicates text (code) symbols
            if " T " not in line:
                continue
            # Get the symbol name (third column)
            symbol = line.split()[2]
            # Write linker arguments
            if sys.platform == "linux":
                f.write(f"-Wl,--undefined={symbol}\n")
            else:
                f.write("-Wl,-U\n")
                f.write(f"-Wl,{symbol}\n")


if __name__ == "__main__":
    main()
