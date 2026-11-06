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

use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::pattern::package::PackagePattern;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_hash::BuckMutInsertionSet;
use buck2_interpreter::paths::package::PackageFilePath;
use buck2_node::visibility::StarlarkTargetNameGlob;
use buck2_node::visibility::VisibilityPattern;
use buck2_node::visibility::VisibilitySpecification;
use buck2_node::visibility::VisibilityWithinViewBuilder;
use buck2_node::visibility::WithinViewSpecification;
use buck2_node::visibility::within_scope_from_parsed;
use buck2_util::arc_str::ThinArcSlice;
use either::Either;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;

use crate::interpreter::build_context::BuildContext;
use crate::super_package::eval_ctx::PackageFileEvalCtx;
use crate::super_package::eval_ctx::PackageFileVisibilityFields;
use crate::super_package::eval_ctx::VisibilitySource;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum PackageFileError {
    #[error("`package()` function can be used at most once per `PACKAGE` file")]
    AtMostOnce,
    #[error("`{0}()` function can be used at most once per `PACKAGE` file")]
    EnforceIntersectionAtMostOnce(&'static str),
    #[error("`{0}()` can only be called from a `PACKAGE` file")]
    EnforceIntersectionMustBeDirect(&'static str),
    #[error(
        "`visibility_exempt_targets` requires an explicit `visibility=` in the same `package()` call"
    )]
    ExemptTargetsRequireVisibility,
    #[error(
        "invalid `visibility_exempt_targets` entry `{0}`: target patterns (`cell//pkg:name`) are not accepted; exemptions select whole packages (exact-package `cell//pkg:` or recursive `cell//pkg/...`)"
    )]
    InvalidExemptTarget(String),
    #[error("`visibility_exempt_targets` entry `{0}` is not a valid pattern: {1}")]
    InvalidExemptTargetSyntax(String, String),
    #[error(
        "`PUBLIC` is not a valid `visibility_exempt_targets` entry (it is a target name, not a package pattern)"
    )]
    PublicNotAllowedInExemptTargets,
    #[error(
        "`visibility_exempt_targets` is vacuous with a `visibility` list containing `\"PUBLIC\"`: any such list collapses to `Public`, which contributes nothing to the intersection, so the exemptions have no effect"
    )]
    ExemptTargetsWithPublicVisibility,
    #[error(
        "`visibility_exempt_targets` entry `{0}` covers this PACKAGE's entire subtree, making the visibility layer vacuous: every target that could inherit it is exempt"
    )]
    ExemptTargetCoversSubtree(String),
    #[error(
        "`visibility_exempt_targets` entry `{0}` is outside this PACKAGE's subtree: exemptions only apply to targets defined under this PACKAGE's directory"
    )]
    ExemptTargetOutsideSubtree(String),
}

fn add_visibility_pattern<'v>(
    builder: &mut VisibilityWithinViewBuilder,
    value: Either<&'v str, &'v StarlarkTargetNameGlob>,
    cell_name: CellName,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<()> {
    match value {
        Either::Left(s) => {
            if s == VisibilityPattern::PUBLIC {
                builder.add_public();
            } else {
                builder.add(VisibilityPattern::Parsed(ParsedPattern::parse_precise(
                    s,
                    cell_name,
                    cell_resolver,
                    cell_alias_resolver,
                )?));
            }
        }
        Either::Right(target_name_glob) => {
            let record = target_name_glob.coerce(|s| {
                ParsedPattern::parse_precise(s, cell_name, cell_resolver, cell_alias_resolver)
            })?;
            builder.add(VisibilityPattern::TargetNameGlob(record));
        }
    }
    Ok(())
}

fn parse_visibility<'v>(
    patterns: &[Either<&'v str, &'v StarlarkTargetNameGlob>],
    cell_name: CellName,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<VisibilitySpecification> {
    let mut builder = VisibilityWithinViewBuilder::with_capacity(patterns.len());
    for &pattern in patterns {
        add_visibility_pattern(
            &mut builder,
            pattern,
            cell_name,
            cell_resolver,
            cell_alias_resolver,
        )?;
    }
    Ok(builder.build_visibility())
}

fn parse_within_view<'v>(
    patterns: &[Either<&'v str, &'v StarlarkTargetNameGlob>],
    cell_name: CellName,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<WithinViewSpecification> {
    let mut builder = VisibilityWithinViewBuilder::with_capacity(patterns.len());
    for &pattern in patterns {
        add_visibility_pattern(
            &mut builder,
            pattern,
            cell_name,
            cell_resolver,
            cell_alias_resolver,
        )?;
    }
    Ok(builder.build_within_view())
}

/// Shared body of the `enforce_*_intersection()` functions: the call must come
/// directly from a `PACKAGE` file (not from a `bzl` it loads) and may happen at
/// most once per file; on success the file's opt-in flag is set.
fn enforce_intersection(
    eval: &mut Evaluator,
    function_name: &'static str,
    opted_in: fn(&PackageFileEvalCtx) -> &RefCell<bool>,
) -> starlark::Result<NoneType> {
    let build_context = BuildContext::from_context(eval)?;
    let package_file_eval_ctx = build_context
        .additional
        .require_package_file(function_name)?;

    let direct_package_call = eval.call_stack_top_location().is_some_and(|loc| {
        let filename = std::path::Path::new(loc.filename());
        PackageFilePath::package_file_names().any(|pkg| filename.ends_with(pkg))
    });
    if !direct_package_call {
        return Err(
            buck2_error::Error::from(PackageFileError::EnforceIntersectionMustBeDirect(
                function_name,
            ))
            .into(),
        );
    }

    let mut enforces = opted_in(package_file_eval_ctx).borrow_mut();
    if *enforces {
        return Err(
            buck2_error::Error::from(PackageFileError::EnforceIntersectionAtMostOnce(
                function_name,
            ))
            .into(),
        );
    }
    *enforces = true;
    Ok(NoneType)
}

/// Cell-local, unlike `PACKAGE` inheritance: an entry in a cell nested under
/// `declaring_dir` is rejected.
fn exemption_within_subtree(pattern: &PackagePattern, declaring_dir: &CellPath) -> bool {
    let entry_dir = match pattern {
        PackagePattern::Package(label) => label.as_cell_path(),
        PackagePattern::Recursive(path) => *path,
    };
    entry_dir.starts_with(*declaring_dir)
}

/// Whether a recursive exemption covers the declaring `PACKAGE`'s own directory,
/// which would exempt every package that inherits this layer.
fn exemption_covers_declaring_dir(pattern: &PackagePattern, declaring_dir: &CellPath) -> bool {
    match pattern {
        PackagePattern::Package(_) => false,
        PackagePattern::Recursive(path) => declaring_dir.starts_with(*path),
    }
}

fn parse_exempt_target_pattern(
    value: &str,
    cell_name: CellName,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<PackagePattern> {
    if value == VisibilityPattern::PUBLIC {
        return Err(buck2_error::Error::from(
            PackageFileError::PublicNotAllowedInExemptTargets,
        ));
    }
    let parsed = ParsedPattern::<TargetPatternExtra>::parse_precise(
        value,
        cell_name,
        cell_resolver,
        cell_alias_resolver,
    )
    .map_err(|e| PackageFileError::InvalidExemptTargetSyntax(value.to_owned(), format!("{e:#}")))?;
    within_scope_from_parsed(parsed).map_err(|_| {
        buck2_error::Error::from(PackageFileError::InvalidExemptTarget(value.to_owned()))
    })
}

fn parse_exempt_targets(
    patterns: &[&str],
    cell_name: CellName,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<ThinArcSlice<PackagePattern>> {
    let mut seen = BuckMutInsertionSet::with_capacity(patterns.len());
    patterns
        .iter()
        .map(|pattern| {
            parse_exempt_target_pattern(pattern, cell_name, cell_resolver, cell_alias_resolver)
        })
        .filter_map(|parsed| match parsed {
            Ok(pattern) if seen.insert(pattern.clone()) => Some(Ok(pattern)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

/// Globals for `PACKAGE` files and `bzl` files included from `PACKAGE` files.
#[starlark_module]
pub(crate) fn register_package_function(globals: &mut GlobalsBuilder) {
    /// DO NOT USE THIS FUNCTION!
    ///
    /// It controls which test config to use in downstream systems. Mostly likely you don't want to specify it by yourself.
    fn test_config_unification_rollout(
        enabled: bool,
        eval: &mut Evaluator,
    ) -> starlark::Result<NoneType> {
        let build_context = BuildContext::from_context(eval)?;
        let package_file_eval_ctx = build_context.additional.require_package_file("package")?;
        *package_file_eval_ctx
            .test_config_unification_rollout
            .borrow_mut() = Some(enabled);
        Ok(NoneType)
    }

    fn package<'v>(
        #[starlark(require=named, default=false)] inherit: bool,
        #[starlark(require=named, default=NoneOr::None)] visibility: NoneOr<
            ValueOf<'v, ListOrTupleType<Either<&'v str, &'v StarlarkTargetNameGlob>>>,
        >,
        #[starlark(require=named, default=ValueOf::empty_list())] within_view: ValueOf<
            'v,
            ListOrTupleType<Either<&'v str, &'v StarlarkTargetNameGlob>>,
        >,
        #[starlark(require=named, default=ValueOf::empty_list())]
        visibility_exempt_targets: ValueOf<'v, ListOrTupleType<&'v str>>,
        eval: &mut Evaluator,
    ) -> starlark::Result<NoneType> {
        let build_context = BuildContext::from_context(eval)?;
        let package_file_eval_ctx = build_context.additional.require_package_file("package")?;
        if package_file_eval_ctx.visibility.borrow().is_some() {
            return Err(buck2_error::Error::from(PackageFileError::AtMostOnce).into());
        }
        let visibility_provided = visibility
            .into_option()
            .map(|v| v.try_to_vec())
            .transpose()?;
        let within_view = within_view.try_to_vec()?;
        let visibility_exempt_targets = visibility_exempt_targets.try_to_vec()?;
        // Validated regardless of `package_visibility.default_intersection`, so an
        // emergency enforce-to-off flip cannot turn valid PACKAGE files into errors.
        if visibility_provided.is_none() && !visibility_exempt_targets.is_empty() {
            return Err(
                buck2_error::Error::from(PackageFileError::ExemptTargetsRequireVisibility).into(),
            );
        }
        let visibility = parse_visibility(
            visibility_provided.as_deref().unwrap_or(&[]),
            build_context.cell_info().name().name(),
            build_context.cell_info().cell_resolver(),
            build_context.cell_info().cell_alias_resolver(),
        )?;
        let within_view = parse_within_view(
            &within_view,
            build_context.cell_info().name().name(),
            build_context.cell_info().cell_resolver(),
            build_context.cell_info().cell_alias_resolver(),
        )?;
        let exemptions = parse_exempt_targets(
            &visibility_exempt_targets,
            build_context.cell_info().name().name(),
            build_context.cell_info().cell_resolver(),
            build_context.cell_info().cell_alias_resolver(),
        )?;
        if !exemptions.is_empty() && visibility.0.is_intersection_identity() {
            return Err(buck2_error::Error::from(
                PackageFileError::ExemptTargetsWithPublicVisibility,
            )
            .into());
        }
        let declaring_dir: CellPath = package_file_eval_ctx.path.dir().to_owned();
        for pattern in exemptions.iter() {
            if exemption_covers_declaring_dir(pattern, &declaring_dir) {
                return Err(
                    buck2_error::Error::from(PackageFileError::ExemptTargetCoversSubtree(
                        pattern.to_string(),
                    ))
                    .into(),
                );
            }
            if !exemption_within_subtree(pattern, &declaring_dir) {
                return Err(buck2_error::Error::from(
                    PackageFileError::ExemptTargetOutsideSubtree(pattern.to_string()),
                )
                .into());
            }
        }
        let visibility_source = match visibility_provided {
            Some(_) => VisibilitySource::Explicit { exemptions },
            None => VisibilitySource::Unset,
        };

        *package_file_eval_ctx.visibility.borrow_mut() = Some(PackageFileVisibilityFields {
            visibility,
            within_view,
            inherit,
            visibility_source,
        });

        Ok(NoneType)
    }

    /// Opts this PACKAGE and its descendants into intersection-based
    /// visibility: every target's effective visibility is ANDed with a
    /// propagating visibility intersection built from each opted-in
    /// ancestor PACKAGE's explicit `package(visibility=...)` list.
    /// `"PUBLIC"` is the identity, so `visibility=["PUBLIC"]` targets are
    /// silently clipped rather than rejected. Calling this without a non-`None`
    /// `package(visibility=...)` (omitted or `visibility=None`) adds
    /// nothing to the intersection — the parent's intersection propagates
    /// unchanged.
    ///
    /// Can only be called from a `PACKAGE` file.
    fn enforce_visibility_intersection(eval: &mut Evaluator) -> starlark::Result<NoneType> {
        enforce_intersection(eval, "enforce_visibility_intersection", |ctx| {
            &ctx.enforces_visibility_intersection
        })
    }

    /// Opts this PACKAGE and its descendants into intersection-based
    /// `within_view`: every target's effective `within_view` is ANDed with a
    /// propagating cap built from each opted-in ancestor PACKAGE's
    /// `package(within_view=...)` list. The cap only tightens: a target
    /// declaring a broader `within_view`, or a descendant PACKAGE
    /// replacing the inherited default with `within_view=["PUBLIC"]`, still
    /// cannot depend on anything outside the cap. `"PUBLIC"` is the
    /// identity, so calling this without `package(within_view=...)` adds
    /// nothing to the cap — the parent's cap propagates unchanged.
    ///
    /// Can only be called from a `PACKAGE` file.
    fn enforce_within_view_intersection(eval: &mut Evaluator) -> starlark::Result<NoneType> {
        enforce_intersection(eval, "enforce_within_view_intersection", |ctx| {
            &ctx.enforces_within_view_intersection
        })
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::package::PackageLabel;

    use super::*;

    fn declaring_dir(package: &str) -> CellPath {
        PackageLabel::testing_parse(package)
            .as_cell_path()
            .to_owned()
    }

    #[test]
    fn exemption_within_subtree_same_cell() {
        let declaring = declaring_dir("root//etc");
        assert!(exemption_within_subtree(
            &PackagePattern::Package(PackageLabel::testing_parse("root//etc/legacy")),
            &declaring,
        ));
        assert!(exemption_within_subtree(
            &PackagePattern::Recursive(declaring_dir("root//etc/generated")),
            &declaring,
        ));
        assert!(!exemption_within_subtree(
            &PackagePattern::Package(PackageLabel::testing_parse("root//other")),
            &declaring,
        ));
    }

    #[test]
    fn exemption_within_subtree_rejects_other_cell() {
        let declaring = declaring_dir("root//");
        assert!(!exemption_within_subtree(
            &PackagePattern::Recursive(declaring_dir("other//generated")),
            &declaring,
        ));
        assert!(!exemption_within_subtree(
            &PackagePattern::Package(PackageLabel::testing_parse("other//generated")),
            &declaring,
        ));
    }

    #[test]
    fn exemption_covers_declaring_dir_recursive_same_cell() {
        let declaring = declaring_dir("root//etc");
        assert!(exemption_covers_declaring_dir(
            &PackagePattern::Recursive(declaring_dir("root//etc")),
            &declaring,
        ));
        assert!(exemption_covers_declaring_dir(
            &PackagePattern::Recursive(declaring_dir("root//")),
            &declaring,
        ));
        assert!(!exemption_covers_declaring_dir(
            &PackagePattern::Recursive(declaring_dir("root//etc/generated")),
            &declaring,
        ));
        assert!(!exemption_covers_declaring_dir(
            &PackagePattern::Package(PackageLabel::testing_parse("root//etc")),
            &declaring,
        ));
    }

    #[test]
    fn exemption_covers_declaring_dir_ignores_nested_cell() {
        let declaring = declaring_dir("root//");
        assert!(!exemption_covers_declaring_dir(
            &PackagePattern::Recursive(declaring_dir("other//")),
            &declaring,
        ));
    }
}
