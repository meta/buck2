/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Configuration of coerced values: select resolution, the fused configured
//! traversal, and concat validation.

use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::config_setting::ConfigSettingData;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use itertools::Itertools;
use ref_cast::RefCast;
use smallvec::SmallVec;

use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::arena::schema::View;
use crate::attrs::attr_type::AttrType;
use crate::attrs::coerced::kinds::ValueVisitor;
use crate::attrs::coerced::kinds::dispatch_value;
use crate::attrs::coerced::select::AttrRefSelectorEntries;
use crate::attrs::coerced::semantics::Semantics;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::configuration::resolved::ConfigurationSettingKey;

impl<'a> CoercedAttrRef<'a> {
    /// Builds the `ConfiguredAttr` straight off the arena: selects resolve on
    /// the view, and only the configured form is materialized.
    ///
    /// TODO(jtbraun): tag errors with `ErrorTag::ConfigureAttr`.
    pub fn configure(
        &self,
        t: &AttrType,
        ctx: &dyn AttrConfigurationContext,
        attr_name: Option<&str>,
    ) -> ResultMaybeCompatible<ConfiguredAttr> {
        dispatch_value(
            *self,
            Configure {
                v: *self,
                t,
                ctx,
                name: attr_name,
            },
        )
    }

    /// The error for a select where no condition matched and no default
    /// was set.
    pub(crate) fn select_no_match_error(
        ctx: &dyn AttrConfigurationContext,
        entries: AttrRefSelectorEntries<'a>,
    ) -> buck2_error::Error {
        buck2_error!(
            buck2_error::ErrorTag::Input,
            "None of {} conditions matched configuration `{}` and no default was set:\n{}",
            entries.len(),
            ctx.cfg().cfg(),
            entries.iter().map(|(k, _)| format!("  {k}")).join("\n"),
        )
    }

    /// Resolve a select against the configuration: most specific matching
    /// entry, or `Ok(None)` for "use the default / no match".
    pub(crate) fn select_view(
        ctx: &dyn AttrConfigurationContext,
        entries: AttrRefSelectorEntries<'a>,
        default: Option<CoercedAttrRef<'a>>,
    ) -> buck2_error::Result<Option<CoercedAttrRef<'a>>> {
        let matched_cfg_keys = ctx.matched_cfg_keys();
        let resolved: SmallVec<[(&ProvidersLabel, &ConfigSettingData, CoercedAttrRef<'a>); 17]> =
            entries
                .iter()
                .filter_map(|(k, v)| {
                    matched_cfg_keys
                        .setting_matches(ConfigurationSettingKey::ref_cast(k))
                        .map(|d| (k, d, v))
                })
                .collect();

        let mut iter = resolved.iter();
        let Some(first) = iter.next() else {
            return Ok(default);
        };
        let mut matching = first;
        let mut slow = None;
        for entry in iter {
            let (_, conf, _) = entry;
            let (_, prev_conf, _) = matching;
            if conf.refines(prev_conf) {
                matching = entry;
            } else if prev_conf.refines(conf) {
                // keep `matching`
            } else {
                slow = Some(Self::select_view_slow(&resolved)?);
                break;
            }
        }
        Ok(Some(slow.unwrap_or(matching.2)))
    }

    pub(super) fn select_view_slow(
        resolved: &[(&ProvidersLabel, &ConfigSettingData, CoercedAttrRef<'a>)],
    ) -> buck2_error::Result<CoercedAttrRef<'a>> {
        let mut entries: Vec<(&ProvidersLabel, &ConfigSettingData, CoercedAttrRef<'a>)> =
            Vec::new();
        for (k, d, v) in resolved {
            if entries.iter().any(|(_, prev_d, _)| prev_d.refines(d)) {
                continue;
            }
            entries.retain(|(_, prev_d, _)| !d.refines(prev_d));
            entries.push((k, d, *v));
        }
        match entries.as_slice() {
            [] => Err(internal_error!(
                "no entries after slow select the most specific"
            )),
            [(.., x)] => Ok(*x),
            multiple_entries => {
                let (first_key, _, first_value) = &multiple_entries[0];
                let different_value_entry = multiple_entries
                    .iter()
                    .skip(1)
                    .find(|(_, _, v)| !v.content_eq(first_value));
                if let Some((different_key, _, _)) = different_value_entry {
                    Err(buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Both select keys `{first_key}` and `{different_key}` match the configuration, but neither is more specific and they have different values"
                    ))
                } else {
                    Ok(*first_value)
                }
            }
        }
    }

    /// Fused [`Self::configure`] + `ConfiguredAttr::traverse`: visit the deps
    /// the configured form of this value would contain, without
    /// materializing `ConfiguredAttr` trees. Containers, selects, and concat
    /// recurse natively (scalars cost nothing); dep-like leaves configure
    /// just the leaf value and traverse it, reusing the per-dep-kind
    /// dispatch of the owned path.
    ///
    /// Concat items are validated as `ConfiguredAttr::concat` would (one
    /// concat-able kind, equal oneof variants, no duplicate dict keys):
    /// configure-time errors must surface here, before the node exists,
    /// because later attr reads assume configuration succeeds.
    pub fn traverse_configured(
        &self,
        t: &AttrType,
        pkg: PackageLabel,
        ctx: &dyn AttrConfigurationContext,
        traversal: &mut dyn ConfiguredAttrTraversal,
        attr_name: Option<&str>,
    ) -> ResultMaybeCompatible<()> {
        dispatch_value(
            *self,
            TraverseConfigured {
                v: *self,
                t,
                pkg,
                ctx,
                tr: traversal,
                name: attr_name,
            },
        )
    }

    /// The leaves `ConfiguredAttr::concat` would see for this value: selects
    /// resolved, nested concats inlined.
    pub(crate) fn concat_leaves(
        &self,
        ctx: &dyn AttrConfigurationContext,
        out: &mut SmallVec<[CoercedAttrRef<'a>; 4]>,
    ) -> ResultMaybeCompatible<()> {
        dispatch_value(*self, ConcatLeaves { v: *self, ctx, out })
    }

    /// The class of values this one concatenates with, if any.
    pub(crate) fn concat_class(&self) -> Option<&'static str> {
        dispatch_value(*self, ConcatClass)
    }
}

pub(crate) struct Configure<'x, 'a> {
    pub(crate) v: CoercedAttrRef<'a>,
    pub(crate) t: &'x AttrType,
    pub(crate) ctx: &'x dyn AttrConfigurationContext,
    pub(crate) name: Option<&'x str>,
}

impl<'a> ValueVisitor<'a> for Configure<'_, 'a> {
    type Out = ResultMaybeCompatible<ConfiguredAttr>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::configure(view, self)
    }
}

pub(crate) struct TraverseConfigured<'x, 'a> {
    pub(crate) v: CoercedAttrRef<'a>,
    pub(crate) t: &'x AttrType,
    pub(crate) pkg: PackageLabel,
    pub(crate) ctx: &'x dyn AttrConfigurationContext,
    pub(crate) tr: &'x mut dyn ConfiguredAttrTraversal,
    pub(crate) name: Option<&'x str>,
}

pub(crate) struct ConcatLeaves<'x, 'a> {
    pub(crate) v: CoercedAttrRef<'a>,
    pub(crate) ctx: &'x dyn AttrConfigurationContext,
    pub(crate) out: &'x mut SmallVec<[CoercedAttrRef<'a>; 4]>,
}

impl<'a> ValueVisitor<'a> for ConcatLeaves<'_, 'a> {
    type Out = ResultMaybeCompatible<()>;
    fn visit<K: Semantics + 'static>(self, view: View<'a, K>) -> Self::Out {
        K::concat_leaves(view, self)
    }
}

pub(crate) struct ConcatClass;

impl<'a> ValueVisitor<'a> for ConcatClass {
    type Out = Option<&'static str>;
    fn visit<K: Semantics + 'static>(self, _view: View<'a, K>) -> Self::Out {
        K::CONCAT
    }
}
