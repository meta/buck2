/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod testing {
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::pattern::pattern::ParsedPattern;
    use buck2_core::pattern::pattern_type::ProvidersPatternExtra;
    use buck2_core::pattern::pattern_type::TargetPatternExtra;
    use buck2_core::target::label::label::TargetLabel;
    use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
    use buck2_interpreter::types::target_label::StarlarkTargetLabel;
    use starlark::environment::GlobalsBuilder;
    use starlark::eval::Evaluator;
    use starlark::starlark_module;
    use starlark::values::ValueOf;

    use crate::interpreter::build_context::BuildContext;

    #[derive(Debug, buck2_error::Error)]
    #[buck2(tag = Input)]
    enum LabelCreatorError {
        #[error("Expected provider, found something else: `{0}`")]
        ExpectedProvider(String),
        #[error("Expected target, found something else: `{0}`")]
        ExpectedTarget(String),
    }

    #[starlark_module]
    pub fn label_creator(builder: &mut GlobalsBuilder) {
        fn label<'v>(
            s: &str,
            eval: &mut Evaluator<'v, '_, '_>,
        ) -> starlark::Result<ValueOf<'v, StarlarkConfiguredProvidersLabel>> {
            let c = BuildContext::from_context(eval)?;
            let target = match ParsedPattern::<ProvidersPatternExtra>::parse_precise(
                s,
                c.cell_info().name().name(),
                c.cell_info().cell_resolver(),
                c.cell_info().cell_alias_resolver(),
            )? {
                ParsedPattern::Target(package, target_name, providers) => {
                    providers.into_providers_label(package, target_name.as_ref())
                }
                _ => {
                    return Err(
                        buck2_error::Error::from(LabelCreatorError::ExpectedProvider(s.to_owned()))
                            .into(),
                    );
                }
            };
            Ok(ValueOf::alloc(
                eval.heap(),
                StarlarkConfiguredProvidersLabel::new(
                    target.configure(ConfigurationData::testing_new()),
                ),
            ))
        }

        fn target_label<'v>(
            s: &str,
            eval: &mut Evaluator<'v, '_, '_>,
        ) -> starlark::Result<ValueOf<'v, StarlarkTargetLabel>> {
            let c = BuildContext::from_context(eval)?;
            let target = match ParsedPattern::<TargetPatternExtra>::parse_precise(
                s,
                c.cell_info().name().name(),
                c.cell_info().cell_resolver(),
                c.cell_info().cell_alias_resolver(),
            )? {
                ParsedPattern::Target(package, target_name, TargetPatternExtra) => {
                    TargetLabel::new(package, target_name.as_ref())
                }
                _ => {
                    return Err(buck2_error::Error::from(LabelCreatorError::ExpectedTarget(
                        s.to_owned(),
                    ))
                    .into());
                }
            };
            Ok(ValueOf::alloc(
                eval.heap(),
                StarlarkTargetLabel::new(target),
            ))
        }
    }
}
