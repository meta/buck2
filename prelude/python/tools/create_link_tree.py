# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import argparse
import json
import os
import shutil


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument(
        "--link-manifest", dest="link_manifests", action="append", default=[]
    )
    parser.add_argument(
        "--copy-manifest", dest="copy_manifests", action="append", default=[]
    )
    args = parser.parse_args()

    os.makedirs(args.output)

    pkgs: set[str] = set()
    pkgs_with_init: set[str] = set()

    def _add_pkg(pkg: str) -> None:
        pkgs.add(pkg)
        parent = os.path.dirname(pkg)
        if parent:
            _add_pkg(parent)

    for symlink, manifest in [(True, m) for m in args.link_manifests] + [
        (False, m) for m in args.copy_manifests
    ]:
        with open(manifest, "r") as f:
            for dst, src, _ in json.load(f):
                # Record pkgs and the ones w/ `__init__.py` files already.
                if dst.endswith((".py", ".so")):
                    pkg = os.path.dirname(dst)
                    _add_pkg(pkg)
                    if os.path.basename(dst) == "__init__.py":
                        pkgs_with_init.add(pkg)

                dst = os.path.join(args.output, dst)
                os.makedirs(os.path.dirname(dst), exist_ok=True)

                # Create symlink.
                if symlink:
                    src = os.path.relpath(src, start=os.path.dirname(dst))
                    try:
                        os.symlink(src, dst)
                    except FileExistsError:
                        # Only fail is the symlink we're going to create would be
                        # different.
                        current_src = os.readlink(dst)
                        if current_src != src:
                            raise RuntimeError(
                                f"conflict for source {dst}: {src} and {current_src}"
                            )
                else:
                    assert not os.path.exists(dst)
                    shutil.copy2(src, dst, follow_symlinks=False)

    # Create any missing ones.
    for pkg in pkgs - pkgs_with_init:
        with open(os.path.join(args.output, pkg, "__init__.py"), "wb") as _:
            pass


if __name__ == "__main__":
    main()
