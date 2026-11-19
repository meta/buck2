/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::provider::label::ProvidersLabel;
use buck2_util::arc_str::ArcStr;
use display_container::fmt_container;

use crate::attrs::arena::CoercedAttrArenaBuilder;
use crate::attrs::arena::OwnedCoercedAttr;
use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::display::AttrDisplayWithContext;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::modifiers::CfgModifiers;

/// A synthesized `buck.*` attribute value: built per call from node data and
/// served as built, never stored in any arena. The vocabulary is fixed by
/// `TargetNodeRef::special_attr_or_none`, and every operation renders exactly
/// as the equivalent stored attribute value would.
#[derive(Debug, Clone)]
pub enum SpecialAttr {
    None,
    String(ArcStr),
    /// Unconfigured dep labels (`buck.deps`).
    LabelList(Vec<ProvidersLabel>),
    /// Configuration dep labels (`buck.configuration_deps`).
    ConfigurationDepList(Vec<ProvidersLabel>),
    TargetModifiers(CfgModifiers),
}

impl SpecialAttr {
    pub fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        match self {
            SpecialAttr::None => Ok(false),
            SpecialAttr::String(s) => filter(s),
            SpecialAttr::LabelList(labels) | SpecialAttr::ConfigurationDepList(labels) => {
                for l in labels {
                    if filter(&l.to_string())? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            SpecialAttr::TargetModifiers(m) => m.any_matches(filter),
        }
    }

    pub fn to_json(&self) -> buck2_error::Result<serde_json::Value> {
        match self {
            SpecialAttr::None => Ok(serde_json::Value::Null),
            SpecialAttr::String(s) => Ok(serde_json::Value::String(s.as_str().to_owned())),
            SpecialAttr::LabelList(labels) | SpecialAttr::ConfigurationDepList(labels) => {
                Ok(serde_json::Value::Array(
                    labels
                        .iter()
                        .map(|l| serde_json::Value::String(l.to_string()))
                        .collect(),
                ))
            }
            SpecialAttr::TargetModifiers(m) => Ok(m.to_json()),
        }
    }

    /// The value sealed into its own single-value storage, for holders that
    /// serve attributes uniformly as arena views.
    pub fn to_owned_coerced(&self) -> buck2_error::Result<OwnedCoercedAttr> {
        let mut builder = CoercedAttrArenaBuilder::default();
        let encoded = match self {
            SpecialAttr::None => builder.encode_none()?,
            SpecialAttr::String(s) => builder.encode_string(s)?,
            SpecialAttr::LabelList(labels) => {
                let items = labels
                    .iter()
                    .map(|l| builder.encode_label(l))
                    .collect::<buck2_error::Result<Vec<_>>>()?;
                builder.encode_list(&items)?
            }
            SpecialAttr::ConfigurationDepList(labels) => {
                let items = labels
                    .iter()
                    .map(|l| builder.encode_configuration_dep(l))
                    .collect::<buck2_error::Result<Vec<_>>>()?;
                builder.encode_list(&items)?
            }
            SpecialAttr::TargetModifiers(m) => builder.encode_target_modifiers(m)?,
        };
        OwnedCoercedAttr::seal_from_builder(builder, encoded)
    }
}

impl AttrDisplayWithContext for SpecialAttr {
    fn fmt(&self, ctx: &AttrFmtContext, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecialAttr::None => write!(f, "None"),
            SpecialAttr::String(s) => {
                if ctx.options.exclude_quotes {
                    f.write_str(s.as_str())
                } else {
                    write!(f, "\"{s}\"")
                }
            }
            SpecialAttr::LabelList(labels) | SpecialAttr::ConfigurationDepList(labels) => {
                fmt_container(f, "[", "]", labels.iter().map(|l| format!("\"{l}\"")))
            }
            SpecialAttr::TargetModifiers(m) => write!(f, "{m}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::target::label::label::TargetLabel;

    use super::*;
    use crate::attrs::display::AttrDisplayWithContextExt;
    use crate::modifiers::ModifierLabelParser;

    fn label(s: &str) -> ProvidersLabel {
        ProvidersLabel::default_for(TargetLabel::testing_parse(s))
    }

    fn samples() -> Vec<SpecialAttr> {
        vec![
            SpecialAttr::None,
            SpecialAttr::String(ArcStr::from("some_rule")),
            SpecialAttr::LabelList(vec![label("root//a:b"), label("cell1//c:d")]),
            SpecialAttr::ConfigurationDepList(vec![label("root//cfg:x")]),
            SpecialAttr::TargetModifiers(CfgModifiers::from_target(
                &serde_json::json!(["root//cfg:linux", {"_type": "ModifiersMatch", "DEFAULT": null, "root//cfg:windows": "root//cfg:msvc"}]),
                &ModifierLabelParser::new(&CellResolver::testing_with_name_and_path(
                    CellName::testing_new("root"),
                    CellRootPathBuf::testing_new(""),
                )),
            )),
        ]
    }

    #[test]
    fn test_rendering_matches_encoded_view() {
        for attr in samples() {
            let owned = attr.to_owned_coerced().unwrap();
            let view = owned.as_ref();
            assert_eq!(
                attr.as_display_no_ctx().to_string(),
                view.as_display_no_ctx().to_string(),
                "display should match the encoded rendering: {attr:?}"
            );
            assert_eq!(
                attr.to_json().unwrap(),
                view.to_json(&AttrFmtContext::NO_CONTEXT).unwrap(),
                "JSON should match the encoded rendering: {attr:?}"
            );
            for probe in ["a", "some", "cfg", "no-such"] {
                let filter = |s: &str| Ok(s.contains(probe));
                assert_eq!(
                    attr.any_matches(&filter).unwrap(),
                    view.any_matches(&filter).unwrap(),
                    "any_matches({probe:?}) should match the encoded result: {attr:?}"
                );
            }
        }
    }

    #[test]
    fn test_display_shapes() {
        assert_eq!("None", SpecialAttr::None.as_display_no_ctx().to_string());
        assert_eq!(
            "\"x\"",
            SpecialAttr::String(ArcStr::from("x"))
                .as_display_no_ctx()
                .to_string()
        );
        assert_eq!(
            "[\"root//a:b\", \"cell1//c:d\"]",
            SpecialAttr::LabelList(vec![label("root//a:b"), label("cell1//c:d")])
                .as_display_no_ctx()
                .to_string()
        );
    }
}
