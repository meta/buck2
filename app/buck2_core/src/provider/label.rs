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
use std::fmt::Display;
use std::fmt::Formatter;
use std::iter;

use allocative::Allocative;
use buck2_util::arc_str::ArcSlice;
use buck2_util::size_assert;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use serde::Serializer;
use strong_hash::StrongHash;
use triomphe::Arc;

use crate::ascii_char_set::AsciiCharSet;
use crate::cells::CellAliasResolver;
use crate::cells::CellResolver;
use crate::cells::name::CellName;
use crate::configuration::data::ConfigurationData;
use crate::configuration::pair::Configuration;
use crate::configuration::pair::ConfigurationNoExec;
use crate::pattern::pattern::ParsedPattern;
use crate::pattern::pattern_type::ProvidersPatternExtra;
use crate::target::configured_target_label::ConfiguredTargetLabel;
use crate::target::label::label::TargetLabel;

#[derive(
    Display,
    Clone,
    Debug,
    Hash,
    StrongHash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    Pagable,
    Serialize,
    Deserialize
)]
pub struct ProviderName(#[pagable(flatten_serde)] String);

#[derive(buck2_error::Error, Debug)]
#[error(
    "Invalid provider name `{}`. Inner providers names can only contain non-empty alpha numeric characters, and symbols `,`, `=`, `-`, `/`, `+` and `_`. No other characters are allowed.",
    _0
)]
#[buck2(tag = Input)]
struct InvalidProviderName(String);

impl ProviderName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn new_unchecked(name: String) -> ProviderName {
        ProviderName(name)
    }

    pub fn new(name: String) -> buck2_error::Result<ProviderName> {
        Self::verify(&name)?;
        Ok(ProviderName(name))
    }

    fn verify(name: &str) -> buck2_error::Result<()> {
        const VALID_CHARS: &str =
            r"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_\\/.=,+-";
        const SET: AsciiCharSet = AsciiCharSet::new(VALID_CHARS);

        if name.is_empty() || name.as_bytes().iter().any(|&b| !SET.contains(b)) {
            return Err(InvalidProviderName(name.to_owned()).into());
        }

        Ok(())
    }
}

#[derive(
    Clone,
    Dupe,
    Debug,
    Hash,
    StrongHash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
pub enum NonDefaultProvidersName {
    Named(ArcSlice<ProviderName>),
}

///
/// A 'ProvidersName' is an optional String label that refers to the specific
/// set of inner providers of a rule.
/// It should be non-empty alphanumeric characters, '/', '.', ',', '-','=',
/// and'_' character. All other special characters including spaces are
/// prohibited.
#[derive(
    Clone,
    Debug,
    Hash,
    StrongHash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
#[derive(Default)]
pub enum ProvidersName {
    #[default]
    Default,
    NonDefault(Arc<NonDefaultProvidersName>),
}

size_assert::words_of_type!(ProvidersName, 1);

impl Dupe for ProvidersName {}

impl Display for ProvidersName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ProvidersName::Default => {
                write!(f, "")
            }
            ProvidersName::NonDefault(flavor) => match flavor.as_ref() {
                NonDefaultProvidersName::Named(names) => {
                    for name in &**names {
                        write!(f, "[{name}]")?;
                    }
                    Ok(())
                }
            },
        }
    }
}

impl ProvidersName {
    pub fn push(&self, name: ProviderName) -> Self {
        let items = match self {
            ProvidersName::Default => vec![name],
            ProvidersName::NonDefault(x) => match &**x {
                NonDefaultProvidersName::Named(xs) => {
                    xs.iter().cloned().chain(iter::once(name)).collect()
                }
            },
        };
        ProvidersName::NonDefault(Arc::new(NonDefaultProvidersName::Named(
            ArcSlice::from_iter(items),
        )))
    }
}

/// A unconfigured 'ProvidersLabel'. This contains a 'TargetLabel' referring to
/// the node on the static graph that produces this provider, and a 'name' is
/// the 'ProvidersName' referring to the specific set of inner providers of a
/// rule.
#[derive(
    Clone, Dupe, Debug, Display, Hash, StrongHash, Eq, PartialEq, Ord, PartialOrd, Allocative,
    Pagable
)]
#[display("{}{}", target, name)]
pub struct ProvidersLabel {
    target: TargetLabel,
    name: ProvidersName,
}

size_assert::words_of_type!(ProvidersLabel, 2);

impl ProvidersLabel {
    pub fn new(target: TargetLabel, name: ProvidersName) -> Self {
        ProvidersLabel { target, name }
    }

    pub fn default_for(target: TargetLabel) -> Self {
        Self::new(target, ProvidersName::Default)
    }

    pub fn into_parts(self) -> (TargetLabel, ProvidersName) {
        (self.target, self.name)
    }

    pub fn target(&self) -> &TargetLabel {
        &self.target
    }

    pub fn name(&self) -> &ProvidersName {
        &self.name
    }

    pub fn parse(
        label: &str,
        cell_name: CellName,
        cell_resolver: &CellResolver,
        cell_alias_resolver: &CellAliasResolver,
    ) -> buck2_error::Result<ProvidersLabel> {
        let providers_label = ParsedPattern::<ProvidersPatternExtra>::parse_precise(
            label,
            cell_name,
            cell_resolver,
            cell_alias_resolver,
        )?
        .as_providers_label(label)?;
        Ok(providers_label)
    }

    /// Creates a 'ConfiguredProvidersLabel' from ['Self'] based on the provided
    /// configuration.
    pub fn configure(&self, cfg: ConfigurationData) -> ConfiguredProvidersLabel {
        ConfiguredProvidersLabel {
            target: self.target.configure(cfg),
            name: self.name.dupe(),
        }
    }

    /// Like `configure`, but forces the execution configuration too.
    pub fn configure_with_exec(
        &self,
        cfg: ConfigurationData,
        exec_cfg: ConfigurationData,
    ) -> ConfiguredProvidersLabel {
        ConfiguredProvidersLabel {
            target: self.target.configure_with_exec(cfg, exec_cfg),
            name: self.name.dupe(),
        }
    }

    #[inline]
    pub fn configure_pair(&self, cfg_pair: Configuration) -> ConfiguredProvidersLabel {
        ConfiguredProvidersLabel {
            target: self.target.configure_pair(cfg_pair),
            name: self.name.dupe(),
        }
    }

    #[inline]
    pub fn configure_pair_no_exec(&self, cfg: ConfigurationNoExec) -> ConfiguredProvidersLabel {
        self.configure_pair(cfg.cfg_pair().dupe())
    }
}

impl Serialize for ProvidersLabel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

///
/// A configured 'ProvidersLabel'.
#[derive(
    Clone, Dupe, Debug, Hash, StrongHash, Eq, PartialEq, Ord, PartialOrd, Allocative, Pagable
)]
pub struct ConfiguredProvidersLabel {
    target: ConfiguredTargetLabel,
    name: ProvidersName,
}

impl Display for ConfiguredProvidersLabel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{} ({})",
            self.target.unconfigured(),
            self.name,
            self.target.cfg()
        )?;
        if let Some(exec_cfg) = self.target.exec_cfg() {
            write!(f, " ({exec_cfg})")?;
        }
        Ok(())
    }
}

impl ConfiguredProvidersLabel {
    pub fn new(target: ConfiguredTargetLabel, name: ProvidersName) -> Self {
        Self { target, name }
    }

    pub fn default_for(target: ConfiguredTargetLabel) -> Self {
        Self {
            target,
            name: ProvidersName::Default,
        }
    }

    pub fn target(&self) -> &ConfiguredTargetLabel {
        &self.target
    }

    #[inline]
    pub fn cfg(&self) -> &ConfigurationData {
        self.target.cfg()
    }

    pub fn unconfigured(&self) -> ProvidersLabel {
        ProvidersLabel::new(self.target.unconfigured().dupe(), self.name.dupe())
    }

    pub fn name(&self) -> &ProvidersName {
        &self.name
    }
}

impl Serialize for ConfiguredProvidersLabel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// [`ProvidersLabel`] or [`ConfiguredProvidersLabel`].
pub trait ProvidersLabelMaybeConfigured: Display + Allocative {}

impl ProvidersLabelMaybeConfigured for ProvidersLabel {}
impl ProvidersLabelMaybeConfigured for ConfiguredProvidersLabel {}

pub mod testing {

    use super::*;
    use crate::package::PackageLabel;
    use crate::target::name::TargetNameRef;

    pub trait ProvidersLabelTestExt {
        fn testing_new(
            cell: &str,
            package: &str,
            target: &str,
            name: Option<&[&str]>,
        ) -> ProvidersLabel;

        fn testing_new_with_target_label(
            target: TargetLabel,
            name: Option<&[&str]>,
        ) -> ProvidersLabel;
    }

    impl ProvidersLabelTestExt for ProvidersLabel {
        fn testing_new(
            cell: &str,
            package: &str,
            target: &str,
            name: Option<&[&str]>,
        ) -> ProvidersLabel {
            let label = TargetLabel::new(
                PackageLabel::testing_new(cell, package),
                TargetNameRef::new(target).unwrap(),
            );
            Self::testing_new_with_target_label(label, name)
        }

        fn testing_new_with_target_label(
            target: TargetLabel,
            name: Option<&[&str]>,
        ) -> ProvidersLabel {
            ProvidersLabel::new(
                target,
                match name {
                    Some(n) => ProvidersName::NonDefault(Arc::new(NonDefaultProvidersName::Named(
                        ArcSlice::from_iter(
                            n.iter()
                                .map(|s| ProviderName::new((*s).to_owned()).unwrap())
                                .collect::<Vec<_>>(),
                        ),
                    ))),
                    _ => ProvidersName::Default,
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use dupe::Dupe;

    use crate::configuration::data::ConfigurationData;
    use crate::configuration::data::ConfigurationDataData;
    use crate::configuration::data::ExecPlatformMarking;
    use crate::provider::label::ConfiguredProvidersLabel;
    use crate::provider::label::ProviderName;
    use crate::provider::label::ProvidersLabel;
    use crate::provider::label::testing::ProvidersLabelTestExt;

    #[test]
    fn providers_name_validation() {
        ProviderName::new("foo".to_owned()).unwrap();
        ProviderName::new("foo_-,.=+/1".to_owned()).unwrap();
        assert!(ProviderName::new("foo bar".to_owned()).is_err());
        assert!(ProviderName::new("foo@bar".to_owned()).is_err());
    }

    fn plain_and_with_exec() -> (ConfiguredProvidersLabel, ConfiguredProvidersLabel) {
        let label = ProvidersLabel::testing_new("root", "pkg", "tc", Some(&["sub"]));
        let cfg = ConfigurationData::testing_new();
        let exec = ConfigurationData::from_platform(
            "<exec>".to_owned(),
            ConfigurationDataData::empty(),
            ExecPlatformMarking::Unmarked,
        )
        .unwrap();
        (
            label.configure(cfg.dupe()),
            label.configure_with_exec(cfg, exec),
        )
    }

    /// The configured providers label prints its execution configuration after the target
    /// configuration, like the configured target label does.
    #[test]
    fn configured_providers_label_display_keeps_exec_cfg() {
        let (plain, with_exec) = plain_and_with_exec();
        assert_ne!(plain, with_exec);

        let cfg = plain.cfg().to_string();
        assert_eq!(plain.to_string(), format!("root//pkg:tc[sub] ({cfg})"));
        assert!(
            with_exec
                .to_string()
                .starts_with(&format!("root//pkg:tc[sub] ({cfg}) (<exec>#")),
            "{with_exec}"
        );
        assert_ne!(plain.to_string(), with_exec.to_string());
        assert_ne!(
            serde_json::to_string(&plain).unwrap(),
            serde_json::to_string(&with_exec).unwrap()
        );
    }
}
