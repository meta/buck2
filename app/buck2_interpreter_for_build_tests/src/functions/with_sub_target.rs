/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::configuration::data::ConfigurationData;
use buck2_core::target::label::label::TargetLabel;
use buck2_interpreter::types::target_label::StarlarkConfiguredTargetLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use dupe::Dupe;
use indoc::indoc;

#[test]
fn test_with_sub_target() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(|globals| {
        let target = TargetLabel::testing_parse("cell//pkg:target");
        globals.set("unconf", StarlarkTargetLabel::from(target.dupe()));
        globals.set(
            "conf",
            StarlarkConfiguredTargetLabel::from(target.configure(ConfigurationData::unbound())),
        );
    });
    tester.run_starlark_test(indoc!(
        r#"
            def test():
                unconf_providers = unconf.with_sub_target(["ab", "cd"])
                assert_eq(isinstance(unconf_providers, ProvidersLabel), True)
                assert_eq(unconf_providers.raw_target(), unconf)
                assert_eq(str(unconf_providers), "cell//pkg:target[ab][cd]")

                conf_providers = conf.with_sub_target(["ab", "cd"])
                assert_eq(isinstance(conf_providers, ConfiguredProvidersLabel), True)
                assert_eq(conf_providers.configured_target(), conf)
                assert_eq(str(conf_providers), "cell//pkg:target[ab][cd] (<unbound>)")
            "#
    ))?;
    Ok(())
}
