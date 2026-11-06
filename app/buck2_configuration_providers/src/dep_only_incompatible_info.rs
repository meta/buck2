/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_core::error::validate_logview_category;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckFixedSortedMap;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list::ListType;

use crate::dep_only_incompatible_rollout::DepOnlyIncompatibleRollout;

/// A provider for defining custom soft error categories for dep-only incompatible targets.
/// This can be used to get finer-grained data on whether it is safe to enable
/// `error_on_dep_only_incompatible` for a given target.
///
/// To use this, first define rule returning this provider.
/// ```python   
/// def _impl(ctx: AnalysisContext) -> list[Provider]:
///    return [
///       DefaultInfo(),
///       DepOnlyIncompatibleInfo(custom_soft_errors = {
///          "dep_only_incompatible_foo": DepOnlyIncompatibleRollout(
///             target_patterns = ["root//foo/..."],
///             exclusions = ["root//foo/excluded/..."],
///          ),
///          "dep_only_incompatible_bar": DepOnlyIncompatibleRollout(
///             target_patterns = ["root//bar/..."],
///             exclusions = [],
///          ),
///       })
///    ]
/// ```
///
/// Then register a target of this rule in root .buckconfig under `buck2.dep_only_incompatible_info`
/// buckconfig key. Once registered, soft errors will be fired under category "dep_only_incompatible_foo"
/// when a target in `root//foo/...` is dep-only incompatible and likewise `dep_only_incompatible_bar` for
/// a target in `root//bar/...`.
#[internal_provider(dep_only_incompatible_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct DepOnlyIncompatibleInfo<'v> {
    pub custom_soft_errors:
        ValueOf<'v, DictType<StringValue<'v>, ValueTyped<'v, DepOnlyIncompatibleRollout<'v>>>>,
}

#[starlark_module]
fn dep_only_incompatible_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = DepOnlyIncompatibleInfo<'static>)]
    fn DepOnlyIncompatibleInfo<'v>(
        #[starlark(require = named)] custom_soft_errors: ValueOf<
            'v,
            DictType<StringValue<'v>, ValueTyped<'v, DepOnlyIncompatibleRollout<'v>>>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<DepOnlyIncompatibleInfo<'v>> {
        // The provider's own copy, so that mutating the caller's dict cannot add a category that
        // was not validated.
        let mut entries = Vec::with_capacity(custom_soft_errors.dict().len());
        for entry in custom_soft_errors.iter() {
            let (category, rollout) = entry?;
            validate_logview_category(category.unpack().as_str())?;
            entries.push((category, rollout));
        }
        Ok(DepOnlyIncompatibleInfo {
            custom_soft_errors: ValueOf::alloc_dict(eval.heap(), entries),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Allocative, pagable::Pagable)]
pub struct DepOnlyIncompatibleRolloutPatterns {
    target_patterns: Box<[ParsedPattern<TargetPatternExtra>]>,
    exclusions: Box<[ParsedPattern<TargetPatternExtra>]>,
}

impl DepOnlyIncompatibleRolloutPatterns {
    pub fn matches(&self, target: &TargetLabel) -> bool {
        for exclusion in &self.exclusions {
            if exclusion.matches(target) {
                return false;
            }
        }
        for target_pattern in &self.target_patterns {
            if target_pattern.matches(target) {
                return true;
            }
        }
        false
    }
}

pub type DepOnlyIncompatibleCustomSoftErrors =
    BuckFixedSortedMap<ArcStr, DepOnlyIncompatibleRolloutPatterns>;

impl<'v> DepOnlyIncompatibleInfo<'v> {
    pub fn custom_soft_errors(
        &self,
        root_cell: CellName,
        cell_resolver: &CellResolver,
        cell_alias_resolver: &CellAliasResolver,
    ) -> buck2_error::Result<DepOnlyIncompatibleCustomSoftErrors> {
        let patterns = |patterns: ValueOf<'v, ListType<StringValue<'v>>>| {
            patterns
                .iter()
                .map(|pattern| {
                    ParsedPattern::parse_precise(
                        pattern?.unpack().as_str(),
                        root_cell.dupe(),
                        cell_resolver,
                        cell_alias_resolver,
                    )
                })
                .collect::<buck2_error::Result<Box<[_]>>>()
        };
        self.custom_soft_errors
            .iter()
            .map(|entry| {
                let (category, rollout) = entry?;
                let rollout = rollout.unpack();
                Ok((
                    ArcStr::from(category.unpack().as_str()),
                    DepOnlyIncompatibleRolloutPatterns {
                        target_patterns: patterns(rollout.target_patterns)?,
                        exclusions: patterns(rollout.exclusions)?,
                    },
                ))
            })
            .collect()
    }
}
