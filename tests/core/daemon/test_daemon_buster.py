# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import json

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class DaemonBusterTest(BuckTestCase):
    @buck_test()
    async def test_daemon_buster(self, buck: Buck) -> None:
        async def pid() -> int:
            return json.loads((await buck.status()).stdout)["process_info"]["pid"]

        settings = buck.cwd / ".bucksettings.toml"

        await buck.build(":")
        pid0 = await pid()

        await buck.build(":")
        pid1 = await pid()
        assert pid1 == pid0

        settings.write_text('[daemon]\nbuster = "1"\n')
        result = await buck.build(":")
        assert (
            'Buck settings changed (daemon.buster: null -> "1"); restarting daemon'
            in result.stderr
        )
        pid2 = await pid()
        assert pid2 != pid1

        await buck.build(":")
        pid3 = await pid()
        assert pid3 == pid2

        settings.write_text('[daemon]\nbuster = "2"\n')
        await buck.build(":")
        pid4 = await pid()
        assert pid4 != pid3

        settings.unlink()
        await buck.build(":")
        pid5 = await pid()
        assert pid5 != pid4
