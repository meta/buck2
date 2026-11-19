# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

AospJarInfo = provider(
    fields = {
        # Optional ART profile source (a text HSPL class/method listing) for this jar.
        # When the jar is placed on an APEX systemserverclasspath, the APEX rule compiles
        # this against the jar with profman to produce a profile-guided .prof for
        # dexpreopt/odrefresh. None if the jar has no profile.
        "profile": provider_field(typing.Any, default = None),
    },
)
