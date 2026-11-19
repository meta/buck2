/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::package::PackageLabel;
use buck2_core::package::source_path::SourcePathRef;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_hash::BuckFixedSortedSet;
use buck2_node::attrs::arena::CoercedAttrRef;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::attr_type::configuration_dep::ConfigurationDepKind;
use buck2_node::attrs::traversal::CoercedAttrTraversal;
use buck2_node::visibility::VisibilityPattern;
use buck2_node::visibility::VisibilityPatternList;
use buck2_node::visibility::WithinViewSpecification;
use dupe::Dupe;

fn indented_within_view(spec: &WithinViewSpecification) -> String {
    match &spec.0 {
        VisibilityPatternList::Public => format!("  {}\n", VisibilityPattern::PUBLIC),
        VisibilityPatternList::List(items) => {
            let mut s = String::new();
            for item in items {
                s.push_str(&format!("  {item}\n"));
            }
            s
        }
        VisibilityPatternList::Intersection(_) => {
            unreachable!("WithinViewSpecification cannot contain Intersection")
        }
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum CheckWithinViewError {
    #[error(
        "Target's `within_view` attribute does not allow dependency `{}`. Allowed dependencies:\n{}",
        _0,
        indented_within_view(_1)
    )]
    #[buck2(tag = Visibility)]
    DepNotWithinView(TargetLabel, WithinViewSpecification),
    #[error(
        "Target's effective `within_view` does not allow dependency `{0}` (within_view = {1}). Capped to {2} by `enforce_within_view_intersection()` in an ancestor PACKAGE"
    )]
    #[buck2(tag = Visibility)]
    DepNotWithinViewCap(TargetLabel, WithinViewSpecification, VisibilityPatternList),
}

/// Check that dependencies in attribute do not violate `within_view`, i.e. that
/// every dep matches both the target's own `within_view` and the cap propagated
/// from `enforce_within_view_intersection()` in ancestor `PACKAGE` files.
pub(crate) fn check_within_view(
    attr: CoercedAttrRef<'_>,
    pkg: PackageLabel,
    attr_type: &AttrType,
    within_view: &WithinViewSpecification,
    within_view_cap: &VisibilityPatternList,
    default_deps: Option<&BuckFixedSortedSet<TargetLabel>>,
) -> buck2_error::Result<()> {
    if within_view == &WithinViewSpecification::PUBLIC
        && within_view_cap == &VisibilityPatternList::Public
    {
        // Shortcut.
        return Ok(());
    }

    struct WithinViewCheckTraversal<'x> {
        pkg: PackageLabel,
        within_view: &'x WithinViewSpecification,
        within_view_cap: &'x VisibilityPatternList,
        default_deps: &'x BuckFixedSortedSet<TargetLabel>,
    }

    impl<'x> WithinViewCheckTraversal<'x> {
        fn check_dep_within_view(&self, dep: TargetLabel) -> buck2_error::Result<()> {
            if self.pkg == dep.pkg()
                || self.default_deps.contains(&dep)
                || (self.within_view.0.matches_target(&dep)?
                    && self.within_view_cap.matches_target(&dep)?)
            {
                return Ok(());
            }
            // Inside an opted-in subtree the cap is named even when the target's
            // own list is what refuses the dep: widening that list alone would
            // still be refused by the cap, so a cap-blind message misleads.
            if matches!(self.within_view_cap, VisibilityPatternList::Public) {
                Err(
                    CheckWithinViewError::DepNotWithinView(dep.dupe(), self.within_view.dupe())
                        .into(),
                )
            } else {
                Err(CheckWithinViewError::DepNotWithinViewCap(
                    dep.dupe(),
                    self.within_view.dupe(),
                    self.within_view_cap.dupe(),
                )
                .into())
            }
        }
    }

    impl<'a, 'x> CoercedAttrTraversal<'a> for WithinViewCheckTraversal<'x> {
        fn dep(&mut self, dep: &ProvidersLabel) -> buck2_error::Result<()> {
            self.check_dep_within_view(dep.target().dupe())
        }

        fn configuration_dep(
            &mut self,
            dep: &ProvidersLabel,
            t: ConfigurationDepKind,
        ) -> buck2_error::Result<()> {
            match t {
                // Skip some configuration deps
                ConfigurationDepKind::CompatibilityAttribute => (),
                ConfigurationDepKind::SelectKey => (),
                ConfigurationDepKind::DefaultTargetPlatform => (),
                ConfigurationDepKind::ConfiguredDepPlatform | ConfigurationDepKind::Transition => {
                    self.check_dep_within_view(dep.target().dupe())?
                }
            }
            Ok(())
        }

        fn input(&mut self, _input: SourcePathRef) -> buck2_error::Result<()> {
            Ok(())
        }
    }

    attr.traverse(
        attr_type,
        Some(pkg),
        &mut WithinViewCheckTraversal {
            pkg,
            within_view,
            within_view_cap,
            default_deps: default_deps.unwrap_or(&BuckFixedSortedSet::new()),
        },
    )
}
