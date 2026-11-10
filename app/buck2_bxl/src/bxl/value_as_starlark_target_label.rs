/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::target_aliases::BuckConfigTargetAliasResolver;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::label::label::TargetLabel;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use dupe::Dupe;
use starlark::values::UnpackValue;
use starlark::values::ValueOf;
use starlark::values::none::NoneType;

#[derive(UnpackValue)]
pub(crate) enum ValueAsStarlarkTargetLabel<'v> {
    None(NoneType),
    Str(&'v str),
    TargetLabel(ValueOf<'v, StarlarkTargetLabel>),
}

impl<'v> ValueAsStarlarkTargetLabel<'v> {
    pub(crate) const NONE: Self = Self::None(NoneType);

    pub(crate) fn parse_target_platforms(
        self,
        target_alias_resolver: &BuckConfigTargetAliasResolver,
        cell_resolver: &CellResolver,
        cell_alias_resolver: &CellAliasResolver,
        cell_name: CellName,
        default_target_platform: Option<TargetLabel>,
    ) -> buck2_error::Result<Option<TargetLabel>> {
        match self {
            ValueAsStarlarkTargetLabel::None(_) => Ok(default_target_platform),
            ValueAsStarlarkTargetLabel::Str(s) => {
                Ok(Some(
                    ParsedPattern::<TargetPatternExtra>::parse_relaxed(
                        target_alias_resolver,
                        // TODO(nga): Parse relaxed relative to cell root is incorrect.
                        CellPath::new(cell_name, CellRelativePath::empty()),
                        s,
                        cell_resolver,
                        cell_alias_resolver,
                    )?
                    .as_target_label(s)?,
                ))
            }
            ValueAsStarlarkTargetLabel::TargetLabel(target) => Ok(Some(target.label().dupe())),
        }
    }

    pub(crate) fn is_none(&self) -> bool {
        matches!(self, Self::None(_))
    }
}
