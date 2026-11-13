/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;

use crate::cfg_constructor::CfgConstructorImpl;
use crate::metadata::key::MetadataKeyRef;
use crate::metadata::super_package_values::SuperPackageValues;
use crate::modifiers::CfgModifiers;
use crate::visibility::VisibilityIntersection;
use crate::visibility::VisibilityPatternList;
use crate::visibility::VisibilitySpecification;
use crate::visibility::WithinViewSpecification;

/// Package value which, when set, makes a `select()` without a matching key or a default
/// mark its target incompatible instead of failing. The value is the reason shown.
pub const UNMATCHED_SELECT_INCOMPATIBLE_KEY: &str = "buck.unmatched_select_incompatible";

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum SuperPackageError {
    #[error("Package value `{0}` must be a string, got `{1}`")]
    NotAString(&'static str, serde_json::Value),
}

#[derive(Debug, Allocative, pagable::Pagable)]
pub(crate) struct SuperPackageData {
    package_values: Arc<dyn SuperPackageValues>,
    visibility: VisibilitySpecification,
    within_view: WithinViewSpecification,
    /// Intersection propagated from `enforce_visibility_intersection()`.
    /// Empty = no restriction.
    visibility_intersection: VisibilityIntersection,
    /// Cap propagated from `enforce_within_view_intersection()`. `Public` = no cap.
    within_view_cap: VisibilityPatternList,
    /// Set only for the repo root package.
    cfg_constructor: Option<Arc<dyn CfgConstructorImpl>>,
    // we have package cfg modifiers in package_values as a starlark value
    // we store it here separately as rust value because we are going to use it
    // in a special attribute and cfg constractor
    cfg_modifiers: Option<CfgModifiers>,
    /// Stored as a rust value for the same reason as `cfg_modifiers`.
    unmatched_select_incompatible: Option<ArcStr>,
    test_config_unification_rollout: bool,
}

impl SuperPackageData {
    pub fn new(
        package_values: Arc<dyn SuperPackageValues>,
        visibility: VisibilitySpecification,
        within_view: WithinViewSpecification,
        visibility_intersection: VisibilityIntersection,
        within_view_cap: VisibilityPatternList,
        cfg_constructor: Option<Arc<dyn CfgConstructorImpl>>,
        cfg_modifiers: Option<CfgModifiers>,
        test_config_unification_rollout: bool,
    ) -> buck2_error::Result<SuperPackageData> {
        let unmatched_select_key = MetadataKeyRef::new(UNMATCHED_SELECT_INCOMPATIBLE_KEY)
            .map_err(buck2_error::Error::from)?;
        let unmatched_select_incompatible =
            match package_values.get_package_value_json(unmatched_select_key)? {
                None => None,
                Some(serde_json::Value::String(reason)) => Some(ArcStr::from(reason)),
                Some(other) => {
                    return Err(SuperPackageError::NotAString(
                        UNMATCHED_SELECT_INCOMPATIBLE_KEY,
                        other,
                    )
                    .into());
                }
            };
        Ok(SuperPackageData {
            package_values,
            visibility,
            within_view,
            visibility_intersection,
            within_view_cap,
            cfg_constructor,
            cfg_modifiers,
            unmatched_select_incompatible,
            test_config_unification_rollout,
        })
    }
}

/// Contents of a `PACKAGE` file merged with contents of containing `PACKAGE` files.
/// This object exists even for non-existent `PACKAGE` files.
#[derive(Debug, Allocative, Clone, Dupe, pagable::Pagable)]
pub struct SuperPackage(Arc<SuperPackageData>);

impl SuperPackage {
    pub fn new(
        package_values: Arc<dyn SuperPackageValues>,
        visibility: VisibilitySpecification,
        within_view: WithinViewSpecification,
        visibility_intersection: VisibilityIntersection,
        within_view_cap: VisibilityPatternList,
        cfg_constructor: Option<Arc<dyn CfgConstructorImpl>>,
        cfg_modifiers: Option<CfgModifiers>,
        test_config_unification_rollout: bool,
    ) -> buck2_error::Result<SuperPackage> {
        Ok(SuperPackage(Arc::new(SuperPackageData::new(
            package_values,
            visibility,
            within_view,
            visibility_intersection,
            within_view_cap,
            cfg_constructor,
            cfg_modifiers,
            test_config_unification_rollout,
        )?)))
    }

    pub fn empty<T: SuperPackageValues + Default>() -> buck2_error::Result<SuperPackage> {
        SuperPackage::new(
            Arc::new(T::default()),
            VisibilitySpecification::default(),
            WithinViewSpecification::default(),
            VisibilityIntersection::unrestricted(),
            VisibilityPatternList::Public,
            None,
            None,
            false,
        )
    }

    pub fn package_values(&self) -> &Arc<dyn SuperPackageValues> {
        &self.0.package_values
    }

    pub fn visibility(&self) -> &VisibilitySpecification {
        &self.0.visibility
    }

    pub fn within_view(&self) -> &WithinViewSpecification {
        &self.0.within_view
    }

    pub fn visibility_intersection(&self) -> &VisibilityIntersection {
        &self.0.visibility_intersection
    }

    /// Cap propagated from `enforce_within_view_intersection()`. `Public` = no cap.
    pub fn within_view_cap(&self) -> &VisibilityPatternList {
        &self.0.within_view_cap
    }

    pub fn cfg_constructor(&self) -> Option<&Arc<dyn CfgConstructorImpl>> {
        self.0.cfg_constructor.as_ref()
    }

    pub fn cfg_modifiers(&self) -> Option<&CfgModifiers> {
        self.0.cfg_modifiers.as_ref()
    }

    /// Reason from `buck.unmatched_select_incompatible`, if set in this or a parent `PACKAGE`.
    pub fn unmatched_select_incompatible(&self) -> Option<&ArcStr> {
        self.0.unmatched_select_incompatible.as_ref()
    }
    pub fn test_config_unification_rollout(&self) -> bool {
        self.0.test_config_unification_rollout
    }
}

impl PartialEq for SuperPackage {
    fn eq(&self, other: &Self) -> bool {
        let SuperPackageData {
            package_values: this_values,
            visibility: this_visibility,
            within_view: this_within_view,
            visibility_intersection: this_visibility_intersection,
            within_view_cap: this_within_view_cap,
            cfg_constructor: this_cfg_constructor,
            cfg_modifiers: _, // cfg_modifiers are already contained in package_values
            unmatched_select_incompatible: _, // also contained in package_values
            test_config_unification_rollout: this_test_config_unification_rollout,
        } = &*self.0;
        let SuperPackageData {
            package_values: other_values,
            visibility: other_visibility,
            within_view: other_within_view,
            visibility_intersection: other_visibility_intersection,
            within_view_cap: other_within_view_cap,
            cfg_constructor: other_cfg_constructor,
            cfg_modifiers: _, // cfg_modifiers are already contained in package_values
            unmatched_select_incompatible: _, // also contained in package_values
            test_config_unification_rollout: other_test_config_unification_rollout,
        } = &*other.0;
        (this_visibility, this_within_view) == (other_visibility, other_within_view)
            && this_visibility_intersection == other_visibility_intersection
            && this_within_view_cap == other_within_view_cap
            && {
                // If either package values are not empty, we cannot compare them
                // because we cannot reliably compare arbitrary Starlark values.
                // So if either package values are not empty, we consider super package not equal.
                this_values.is_empty() && other_values.is_empty()
                &&
                // Same logic for cfg constructors.
                this_cfg_constructor.is_none() && other_cfg_constructor.is_none()
            }
            && this_test_config_unification_rollout == other_test_config_unification_rollout
    }
}
