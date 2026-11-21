# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test


@buck_test()
async def test_external_cell_set_cfg_constructor_silently_ignored(buck: Buck) -> None:
    """Test that set_cfg_constructor called from an external cell's PACKAGE is silently ignored.

    The external cell has its own set_cfg_constructor call in its PACKAGE file.
    This should be silently ignored, and the root cell's cfg_constructor
    should be used instead.
    """
    # Query a target in the root cell - should succeed
    result = await buck.cquery("root//:test")
    assert "root//:test" in result.stdout


@buck_test()
async def test_external_cell_target_succeeds_with_cfg_constructor(buck: Buck) -> None:
    """Test that targets in external cells work when the external cell has set_cfg_constructor.

    Even though the external cell has its own set_cfg_constructor in its PACKAGE,
    querying targets in that cell should succeed because the call is silently ignored.
    """
    # Query a target in the external cell - should succeed
    # The external cell's set_cfg_constructor should be silently ignored
    result = await buck.cquery("external_cell//:external_test")
    assert "external_cell//:external_test" in result.stdout
    # Make sure the external cell's constructor wasn't used
    assert "SHOULD_NOT_BE_USED" not in result.stdout
