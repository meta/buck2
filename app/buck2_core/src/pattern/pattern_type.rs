/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::configuration::bound_label::BoundConfigurationLabel;
use crate::configuration::builtin::BuiltinPlatform;
use crate::configuration::data::ConfigurationData;
use crate::configuration::hash::ConfigurationHash;
use crate::package::PackageLabel;
use crate::pattern::pattern::Modifiers;
use crate::pattern::pattern::ProvidersLabelWithModifiers;
use crate::provider::label::ProvidersLabel;
use crate::provider::label::ProvidersName;
use crate::target::label::label::TargetLabel;
use crate::target::name::TargetNameRef;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum PatternTypeError {
    #[error("Expecting target pattern, without providers")]
    ExpectingTargetNameWithoutProviders,
    #[error("Expecting target pattern, without configuration")]
    ExpectingTargetPatternWithoutConfiguration,
    #[error("Expecting provider pattern, without configuration")]
    ExpectingProviderPatternWithoutConfiguration,
}

/// The pattern type to be parsed from the command line target patterns.
///
/// This is either 'TargetLabel', 'ConfiguredTargetLabel', or
/// 'ConfiguredProvidersLabel'
pub trait PatternType:
    Send
    + Sync
    + Sized
    + Clone
    + Default
    + Display
    + Debug
    + PartialEq
    + Eq
    + Ord
    + Allocative
    + 'static
{
    const NAME: &'static str;

    /// Construct this from a configured providers pattern.
    /// Return error if configured providers pattern extra contains parts
    /// that are not allowed for this pattern type.
    fn from_configured_providers(
        providers: ConfiguredProvidersPatternExtra,
    ) -> buck2_error::Result<Self>;

    /// This pattern matches the configuration.
    ///
    /// Ignore providers.
    fn matches_cfg(&self, cfg: &ConfigurationData) -> bool;

    fn into_providers(self) -> ProvidersName;
}

/// Pattern that matches an explicit target without any inner providers label.
/// This is useful for 'query's where we do not expect any provider specifiers.
///
/// Ex. `//some/package:target`
#[derive(
    derive_more::Display,
    Clone,
    Default,
    Debug,
    Eq,
    PartialEq,
    Hash,
    StrongHash,
    Ord,
    PartialOrd,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
#[display("")]
pub struct TargetPatternExtra;

impl PatternType for TargetPatternExtra {
    const NAME: &'static str = "target";

    fn from_configured_providers(
        providers: ConfiguredProvidersPatternExtra,
    ) -> buck2_error::Result<Self> {
        let ConfiguredProvidersPatternExtra {
            providers,
            cfg,
            exec_cfg,
        } = providers;
        if providers != ProvidersName::Default {
            return Err(PatternTypeError::ExpectingTargetNameWithoutProviders.into());
        }
        if !cfg.is_any() || !exec_cfg.is_any() {
            return Err(PatternTypeError::ExpectingTargetPatternWithoutConfiguration.into());
        }
        Ok(TargetPatternExtra)
    }

    fn matches_cfg(&self, _cfg: &ConfigurationData) -> bool {
        true
    }

    fn into_providers(self) -> ProvidersName {
        ProvidersName::Default
    }
}

/// Pattern that matches an inner providers label that refers to a specific
/// set of providers from a rule.
/// This is useful for builds, and provider or action queries where provider
/// specifiers makes sense
///
/// Ex. `//some/package:target[java-group]`
#[derive(
    derive_more::Display,
    Clone,
    Default,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative
)]
pub struct ProvidersPatternExtra {
    pub providers: ProvidersName,
}

impl ProvidersPatternExtra {
    pub fn into_providers_label(
        self,
        package: PackageLabel,
        target_name: &TargetNameRef,
    ) -> ProvidersLabel {
        ProvidersLabel::new(TargetLabel::new(package, target_name), self.providers)
    }

    pub fn into_providers_label_with_modifiers(
        self,
        package: PackageLabel,
        target_name: &TargetNameRef,
        modifiers: Modifiers,
    ) -> ProvidersLabelWithModifiers {
        ProvidersLabelWithModifiers {
            providers_label: self.into_providers_label(package, target_name),
            modifiers,
        }
    }
}

impl PatternType for ProvidersPatternExtra {
    const NAME: &'static str = "providers";

    fn from_configured_providers(
        providers: ConfiguredProvidersPatternExtra,
    ) -> buck2_error::Result<Self> {
        let ConfiguredProvidersPatternExtra {
            providers,
            cfg,
            exec_cfg,
        } = providers;
        if !cfg.is_any() || !exec_cfg.is_any() {
            return Err(PatternTypeError::ExpectingProviderPatternWithoutConfiguration.into());
        }
        Ok(ProvidersPatternExtra { providers })
    }

    fn matches_cfg(&self, _cfg: &ConfigurationData) -> bool {
        true
    }

    fn into_providers(self) -> ProvidersName {
        self.providers
    }
}

#[derive(
    Default,
    Clone,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    derive_more::Display
)]
pub enum ConfigurationPredicate {
    /// Matches any configuration.
    #[default]
    #[display("<any>")]
    Any,
    /// Matches builtin platform.
    Builtin(BuiltinPlatform),
    /// Matches user defined configuration.
    #[display(
        "{}{}",
        _0,
        _1.as_ref().map_or(String::new(), |h| format!("#{h}"))
    )]
    Bound(
        BoundConfigurationLabel,
        /// None means match any configuration with given label.
        Option<ConfigurationHash>,
    ),
}

impl ConfigurationPredicate {
    fn is_any(&self) -> bool {
        matches!(self, ConfigurationPredicate::Any)
    }

    fn matches_cfg(&self, cfg: &ConfigurationData) -> bool {
        match self {
            ConfigurationPredicate::Any => true,
            ConfigurationPredicate::Builtin(builtin) => {
                cfg == &ConfigurationData::builtin(*builtin)
            }
            ConfigurationPredicate::Bound(label, hash) => match cfg.bound() {
                None => false,
                Some(cfg_label) => {
                    label == cfg_label
                        && (hash.is_none() || hash.as_ref() == Some(cfg.output_hash()))
                }
            },
        }
    }
}

impl ConfigurationPredicate {
    fn display_suffix(&self) -> impl Display + '_ {
        struct Impl<'a>(&'a ConfigurationPredicate);

        impl Display for Impl<'_> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                match self.0 {
                    ConfigurationPredicate::Any => Ok(()),
                    ConfigurationPredicate::Builtin(builtin) => {
                        write!(f, " ({builtin})")
                    }
                    ConfigurationPredicate::Bound(label, hash) => {
                        write!(f, " ({label}")?;
                        if let Some(hash) = hash {
                            write!(f, "#{hash}")?;
                        }
                        write!(f, ")")?;
                        Ok(())
                    }
                }
            }
        }

        Impl(self)
    }
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Allocative)]
pub struct ConfiguredTargetPatternExtra {
    /// Configuration part of pattern `foo//bar:baz (cfg#ab01)`.
    pub cfg: ConfigurationPredicate,
    /// Execution configuration part of pattern `foo//bar:baz (cfg) (exec_cfg)`.
    pub exec_cfg: ConfigurationPredicate,
}

impl Display for ConfiguredTargetPatternExtra {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cfg.display_suffix())?;
        if !self.exec_cfg.is_any() {
            write!(f, "{}", self.exec_cfg.display_suffix())?;
        }
        Ok(())
    }
}

impl PatternType for ConfiguredTargetPatternExtra {
    const NAME: &'static str = "configured target";

    fn from_configured_providers(
        providers: ConfiguredProvidersPatternExtra,
    ) -> buck2_error::Result<Self> {
        let ConfiguredProvidersPatternExtra {
            providers,
            cfg,
            exec_cfg,
        } = providers;
        if providers != ProvidersName::Default {
            return Err(PatternTypeError::ExpectingTargetNameWithoutProviders.into());
        }
        Ok(ConfiguredTargetPatternExtra { cfg, exec_cfg })
    }

    fn matches_cfg(&self, cfg: &ConfigurationData) -> bool {
        self.cfg.matches_cfg(cfg)
    }

    fn into_providers(self) -> ProvidersName {
        ProvidersName::Default
    }
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Allocative)]
pub struct ConfiguredProvidersPatternExtra {
    pub providers: ProvidersName,
    /// Configuration part of pattern `foo//bar:baz[Provider] (cfg#ab01)`.
    pub cfg: ConfigurationPredicate,
    /// Execution configuration part of pattern `foo//bar:baz (cfg) (exec_cfg)`.
    pub exec_cfg: ConfigurationPredicate,
}

impl Display for ConfiguredProvidersPatternExtra {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.providers, self.cfg.display_suffix())?;
        if !self.exec_cfg.is_any() {
            write!(f, "{}", self.exec_cfg.display_suffix())?;
        }
        Ok(())
    }
}

impl PatternType for ConfiguredProvidersPatternExtra {
    const NAME: &'static str = "configured providers";

    fn from_configured_providers(
        extra: ConfiguredProvidersPatternExtra,
    ) -> buck2_error::Result<Self> {
        Ok(extra)
    }

    fn matches_cfg(&self, cfg: &ConfigurationData) -> bool {
        self.cfg.matches_cfg(cfg)
    }

    fn into_providers(self) -> ProvidersName {
        self.providers
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::bound_label::BoundConfigurationLabel;
    use crate::configuration::builtin::BuiltinPlatform;
    use crate::configuration::data::ConfigurationData;
    use crate::configuration::data::ConfigurationDataData;
    use crate::configuration::data::ExecPlatformMarking;
    use crate::configuration::hash::ConfigurationHash;
    use crate::pattern::pattern_type::ConfigurationPredicate;
    use crate::pattern::pattern_type::ConfiguredProvidersPatternExtra;
    use crate::pattern::pattern_type::PatternType;
    use crate::provider::label::ProvidersName;

    #[test]
    fn test_configured_providers_matches_cfg() {
        // Possible configurations.
        let unbound = ConfigurationData::unbound();
        let unspecified = ConfigurationData::unspecified();
        let foo = ConfigurationData::from_platform(
            "<foo>".to_owned(),
            ConfigurationDataData::empty(),
            ExecPlatformMarking::Unmarked,
        )
        .unwrap();
        let bar = ConfigurationData::from_platform(
            "<bar>".to_owned(),
            ConfigurationDataData::empty(),
            ExecPlatformMarking::Unmarked,
        )
        .unwrap();

        // Possible matchers.
        let catch_all = ConfiguredProvidersPatternExtra {
            providers: ProvidersName::Default,
            cfg: ConfigurationPredicate::Any,
            exec_cfg: ConfigurationPredicate::Any,
        };
        let catch_unbound = ConfiguredProvidersPatternExtra {
            providers: ProvidersName::Default,
            cfg: ConfigurationPredicate::Builtin(BuiltinPlatform::Unbound),
            exec_cfg: ConfigurationPredicate::Any,
        };
        let catch_foo_any = ConfiguredProvidersPatternExtra {
            providers: ProvidersName::Default,
            cfg: ConfigurationPredicate::Bound(
                BoundConfigurationLabel::new("<foo>".to_owned()).unwrap(),
                None,
            ),
            exec_cfg: ConfigurationPredicate::Any,
        };
        let catch_foo_with_hash = ConfiguredProvidersPatternExtra {
            providers: ProvidersName::Default,
            cfg: ConfigurationPredicate::Bound(
                BoundConfigurationLabel::new("<foo>".to_owned()).unwrap(),
                Some(foo.output_hash().clone()),
            ),
            exec_cfg: ConfigurationPredicate::Any,
        };
        let catch_foo_wrong_hash = ConfiguredProvidersPatternExtra {
            providers: ProvidersName::Default,
            cfg: ConfigurationPredicate::Bound(
                BoundConfigurationLabel::new("<foo>".to_owned()).unwrap(),
                Some(ConfigurationHash::new(17)),
            ),
            exec_cfg: ConfigurationPredicate::Any,
        };

        // Now the tests.
        assert!(catch_all.matches_cfg(&unbound));
        assert!(catch_all.matches_cfg(&unspecified));
        assert!(catch_all.matches_cfg(&foo));
        assert!(catch_all.matches_cfg(&bar));

        assert!(catch_unbound.matches_cfg(&unbound));
        assert!(!catch_unbound.matches_cfg(&unspecified));
        assert!(!catch_unbound.matches_cfg(&foo));
        assert!(!catch_unbound.matches_cfg(&bar));

        assert!(!catch_foo_any.matches_cfg(&unbound));
        assert!(!catch_foo_any.matches_cfg(&unspecified));
        assert!(catch_foo_any.matches_cfg(&foo));
        assert!(!catch_foo_any.matches_cfg(&bar));

        assert!(!catch_foo_with_hash.matches_cfg(&unbound));
        assert!(!catch_foo_with_hash.matches_cfg(&unspecified));
        assert!(catch_foo_with_hash.matches_cfg(&foo));
        assert!(!catch_foo_with_hash.matches_cfg(&bar));

        assert!(!catch_foo_wrong_hash.matches_cfg(&unbound));
        assert!(!catch_foo_wrong_hash.matches_cfg(&unspecified));
        assert!(!catch_foo_wrong_hash.matches_cfg(&foo));
        assert!(!catch_foo_wrong_hash.matches_cfg(&bar));
    }
}
