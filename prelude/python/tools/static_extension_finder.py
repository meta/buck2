# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


# Add a try except to force eager importing
try:
    # pyre-fixme[21]: Could not find module `_static_extension_utils`.
    from _static_extension_utils import _check_module, StaticExtensionLoader
except BaseException:
    raise


class StaticExtensionFinder:
    # pyre-fixme
    ModuleSpec = None

    @classmethod
    # pyre-fixme[3]: Return type must be annotated.
    # pyre-fixme[2]: Parameter must be annotated.
    def find_spec(cls, fullname, path, target=None):
        """
        Use fullname to look up the PyInit function in the main binary. Returns None if not present.
        This allows importing CExtensions that have been statically linked in.
        """

        if not fullname:
            return None
        if not _check_module(fullname):
            return None
        spec = cls.ModuleSpec(
            fullname, StaticExtensionLoader, origin="static-extension", is_package=False
        )
        return spec


def _initialize() -> None:
    # This imports are here to avoid tricking circular dependencies. see S389486
    import sys
    from importlib.machinery import ModuleSpec

    StaticExtensionFinder.ModuleSpec = ModuleSpec

    sys.meta_path.insert(0, StaticExtensionFinder)
