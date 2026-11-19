# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android/user:android_emulators.bzl", _android_emulators_spec = "registration_spec")
load("@prelude//cxx/user:cxx_toolchain_override.bzl", _cxx_toolchain_override_spec = "cxx_toolchain_override_registration_spec")
load("@prelude//cxx/user:link_group_map.bzl", _link_group_map_spec = "registration_spec")
load("@prelude//http_archive:extract_archive.bzl", _extract_archive_spec = "registration_spec")
load("@prelude//ide_integrations/xcode:xcode_files.bzl", _xcode_files_spec = "registration_spec")
load(":cxx_headers_bundle.bzl", _cxx_headers_bundle_spec = "registration_spec")
load(":write_file.bzl", _write_file_spec = "registration_spec")

_all_specs = [
    _extract_archive_spec,
    _android_emulators_spec,
    _link_group_map_spec,
    _cxx_headers_bundle_spec,
    _cxx_toolchain_override_spec,
    _write_file_spec,
    _xcode_files_spec,
]

rules = {
    s.name: rule(impl = s.impl, attrs = s.attrs, doc = s.doc, is_toolchain_rule = s.is_toolchain_rule, **{k: v for k, v in {"cfg": s.cfg}.items() if v != None})
    for s in _all_specs
}

# The rules are accessed by doing module.name, so we have to put them on the correct module.
load_symbols(rules)
