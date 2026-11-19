# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def config_backed_java_toolchain(name, is_bootstrap_toolchain = False, javac = None, java = None, java_for_tests = None, visibility = None, global_code_config = {}):
    _unused = (name, is_bootstrap_toolchain, javac, java, java_for_tests, visibility, global_code_config)

def config_backed_kotlin_toolchain(name, kotlinc = None, is_bootstrap_toolchain = False, extra_kotlin_home_libraries = [], **kwargs):
    _unused = (name, kotlinc, is_bootstrap_toolchain, extra_kotlin_home_libraries, kwargs)
