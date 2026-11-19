/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;
use std::sync::Arc;

use buck2_core::pattern::package::PackagePattern;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::paths::package::PackageFilePath;
use buck2_node::cfg_constructor::CfgConstructorImpl;
use buck2_node::metadata::key::MetadataKeyRef;
use buck2_node::modifiers::CfgModifiers;
use buck2_node::modifiers::MODIFIER_METADATA_KEY;
use buck2_node::modifiers::ModifierLabelParser;
use buck2_node::super_package::SuperPackage;
use buck2_node::visibility::VisibilityLayerOrigin;
use buck2_node::visibility::VisibilityPatternList;
use buck2_node::visibility::VisibilitySpecification;
use buck2_node::visibility::WithinViewSpecification;
use buck2_settings::PackageVisibilityDefaultIntersection;
use buck2_util::arc_str::ThinArcSlice;
use dupe::Dupe;

use crate::interpreter::package_file_extra::MAKE_CFG_CONSTRUCTOR;
use crate::interpreter::package_file_extra::OwnedFrozenPackageFileExtra;
use crate::super_package::package_value::SuperPackageValuesImpl;

#[derive(Debug, Default)]
pub(crate) struct PackageFileVisibilityFields {
    pub(crate) visibility: VisibilitySpecification,
    pub(crate) within_view: WithinViewSpecification,
    pub(crate) inherit: bool,
    pub(crate) visibility_source: VisibilitySource,
}

/// Whether the `package()` call passed an explicit `visibility=` list.
///
/// A non-empty `visibility_exempt_targets` requires an explicit `visibility=`
/// (validated in `package()`), so "exemptions without visibility" is
/// unrepresentable: only the `Explicit` variant carries exemptions. Omitted or
/// `visibility=None` are both `Unset` and contribute nothing to the intersection
/// (unlike `visibility=[]`, which is a non-`None` empty list).
#[derive(Debug, Default)]
pub(crate) enum VisibilitySource {
    #[default]
    Unset,
    /// Carries this call's exemptions, matched against the package containing
    /// the target being defined; dormant unless this call's visibility
    /// contributes to the intersection.
    Explicit {
        exemptions: ThinArcSlice<PackagePattern>,
    },
}

#[derive(Debug)]
pub struct PackageFileEvalCtx {
    pub path: PackageFilePath,
    /// Parent file context.
    /// When evaluating root `PACKAGE` file, parent is still defined.
    pub(crate) parent: SuperPackage,
    pub(crate) visibility: RefCell<Option<PackageFileVisibilityFields>>,
    pub(crate) test_config_unification_rollout: RefCell<Option<bool>>,
    /// `true` iff this PACKAGE called `enforce_visibility_intersection()`.
    pub(crate) enforces_visibility_intersection: RefCell<bool>,
    /// `true` iff this PACKAGE called `enforce_within_view_intersection()`.
    pub(crate) enforces_within_view_intersection: RefCell<bool>,
    pub(crate) package_visibility_default_intersection: PackageVisibilityDefaultIntersection,
}

impl PackageFileEvalCtx {
    fn cfg_constructor(
        extra: Option<&OwnedFrozenPackageFileExtra>,
    ) -> buck2_error::Result<Option<Arc<dyn CfgConstructorImpl>>> {
        let Some(extra) = extra else {
            return Ok(None);
        };
        let Some(cfg_constructor) = extra.cfg_constructor() else {
            return Ok(None);
        };
        let make_cfg_constructor = MAKE_CFG_CONSTRUCTOR.get()?;
        Ok(Some(make_cfg_constructor(cfg_constructor)?))
    }

    pub(crate) fn build_super_package(
        self,
        extra: Option<OwnedFrozenPackageFileExtra>,
        modifier_labels: &ModifierLabelParser,
    ) -> buck2_error::Result<SuperPackage> {
        let cfg_constructor = Self::cfg_constructor(extra.as_ref())?;

        let package_values = match &extra {
            None => BuckMutInsertionMap::new(),
            Some(extra) => extra.package_values(),
        };

        let modifier_key = MetadataKeyRef::new(MODIFIER_METADATA_KEY)?;
        let parent_cfg_modifiers = self.parent.cfg_modifiers().copied();
        let cfg_modifiers = match package_values.get(modifier_key) {
            Some(value) => Some(value.with_value(|v| {
                CfgModifiers::from_package(v, modifier_labels, parent_cfg_modifiers)
            })),
            None => parent_cfg_modifiers,
        };
        let merged_package_values =
            SuperPackageValuesImpl::merge(self.parent.package_values(), package_values)?;

        let visibility_fields = self.visibility.into_inner();

        // Captured before `inherit=True` is applied. `None` when omitted —
        // omitted must NOT contribute an empty list to the intersection.
        let (explicit_visibility, explicit_exemptions): (
            Option<VisibilityPatternList>,
            ThinArcSlice<PackagePattern>,
        ) = match visibility_fields
            .as_ref()
            .map(|f| (&f.visibility, &f.visibility_source))
        {
            Some((visibility, VisibilitySource::Explicit { exemptions })) => {
                (Some(visibility.0.dupe()), exemptions.dupe())
            }
            _ => (None, ThinArcSlice::empty()),
        };

        // Captured before `inherit=True` is applied, like `explicit_visibility`.
        // An omitted `within_view=` parses to `Public`, the identity of the
        // intersection, so unlike `visibility` it needs no `was_set` flag.
        let explicit_within_view: Option<VisibilityPatternList> =
            visibility_fields.as_ref().map(|f| f.within_view.0.dupe());

        let (visibility, within_view) = match visibility_fields {
            Some(package_visibility) => {
                if package_visibility.inherit {
                    (
                        self.parent
                            .visibility()
                            .extend_with(&package_visibility.visibility)?,
                        self.parent
                            .within_view()
                            .extend_with(&package_visibility.within_view)?,
                    )
                } else {
                    (
                        package_visibility.visibility,
                        package_visibility.within_view,
                    )
                }
            }
            None => {
                // If the package file does not specify any visibility, default to the parent visibility.
                (
                    self.parent.visibility().to_owned(),
                    self.parent.within_view().to_owned(),
                )
            }
        };

        let test_config_unification_rollout =
            match self.test_config_unification_rollout.into_inner() {
                Some(test_config_unification_rollout) => test_config_unification_rollout,
                None => self.parent.test_config_unification_rollout(),
            };

        let enforces_by_default = matches!(
            self.package_visibility_default_intersection,
            PackageVisibilityDefaultIntersection::Audit
                | PackageVisibilityDefaultIntersection::Enforce
        );
        let origin = if self.enforces_visibility_intersection.into_inner() {
            Some(VisibilityLayerOrigin::Marker)
        } else if enforces_by_default {
            Some(VisibilityLayerOrigin::Default)
        } else {
            None
        };
        let visibility_intersection = match (origin, explicit_visibility) {
            (Some(origin), Some(raw)) => {
                self.parent
                    .visibility_intersection()
                    .with_layer(raw, explicit_exemptions, origin)
            }
            _ => self.parent.visibility_intersection().dupe(),
        };

        let within_view_cap = match (
            self.enforces_within_view_intersection.into_inner(),
            explicit_within_view,
        ) {
            (true, Some(raw)) => self.parent.within_view_cap().intersect_with(&raw),
            _ => self.parent.within_view_cap().dupe(),
        };

        SuperPackage::new(
            merged_package_values,
            visibility,
            within_view,
            visibility_intersection,
            within_view_cap,
            cfg_constructor,
            cfg_modifiers,
            test_config_unification_rollout,
        )
    }
}
