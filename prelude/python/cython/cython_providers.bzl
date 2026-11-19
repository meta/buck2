# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

CythonCompileOutput = record(
    # The generated .c or .cpp source file
    cpp_src = field(Artifact),
    # The generated _api.h header (may not exist for all modules)
    api_header = field(Artifact),
    # The generated .h public header (may not exist for all modules)
    public_header = field(Artifact),
)

CythonIncludesInfo = provider(
    fields = {
        # Symlinked directory of .pxd/.pxi files for Cython include resolution
        "include_tree": provider_field(Artifact),
        # {package_path/name: Artifact} for downstream merging of headers
        "raw_headers": provider_field(dict[str, Artifact]),
    }
)

CythonLibraryInfo = provider(
    fields = {
        # {path: Artifact} for _api.h/.h declaration headers for C++ interop
        "declaration_headers": provider_field(dict[str, Artifact]),
        # CythonIncludesInfo for downstream Cython deps
        "include_info": provider_field(CythonIncludesInfo),
    }
)
