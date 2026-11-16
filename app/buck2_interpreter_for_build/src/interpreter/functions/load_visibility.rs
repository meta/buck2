/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_node::visibility::LoadVisibility;
use buck2_node::visibility::VisibilityPattern;
use buck2_node::visibility::VisibilityWithinViewBuilder;
use dice::DiceComputations;
use either::Either;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::list_or_tuple::UnpackListOrTuple;
use starlark::values::none::NoneType;

use crate::interpreter::build_context::BuildContext;
use crate::interpreter::extra_value::InterpreterExtraValue;

/// Buckconfig key (`buck2.check_load_visibility`) gating enforcement of load
/// visibility declared via the `visibility()` builtin in `.bzl` files. Defaults
/// to `true`: only files that call `visibility()` are restricted, so existing
/// code is unaffected. Set it to `false` to disable enforcement.
const CHECK_LOAD_VISIBILITY: BuckconfigKeyRef = BuckconfigKeyRef {
    section: "buck2",
    property: "check_load_visibility",
};

pub(crate) trait GetCheckLoadVisibility {
    /// Whether to enforce load visibility declared via `visibility()`.
    fn get_check_load_visibility(&mut self) -> buck2_error::Result<bool>;
}

impl<T> GetCheckLoadVisibility for T
where
    T: LegacyBuckConfigView,
{
    fn get_check_load_visibility(&mut self) -> buck2_error::Result<bool> {
        Ok(self.parse::<bool>(CHECK_LOAD_VISIBILITY)?.unwrap_or(true))
    }
}

#[async_trait]
pub(crate) trait HasCheckLoadVisibility {
    /// Reads `buck2.check_load_visibility` from the root buckconfig.
    async fn get_check_load_visibility(&mut self) -> buck2_error::Result<bool>;
}

#[async_trait]
impl HasCheckLoadVisibility for DiceComputations<'_> {
    async fn get_check_load_visibility(&mut self) -> buck2_error::Result<bool> {
        self.get_legacy_root_config_on_dice()
            .await?
            .view(self)
            .get_check_load_visibility()
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub(crate) enum LoadVisibilityError {
    #[error(
        "`visibility()` must be called at the top level of the `.bzl` file it applies to, not from inside a function"
    )]
    NotTopLevel,
    #[error("`visibility()` can be called at most once per `.bzl` file")]
    AtMostOnce,
    #[error(
        "`{0}` is a target label, but `visibility()` takes package patterns (`//pkg:` or `//pkg/...`)"
    )]
    TargetLabel(String),
    #[error(
        "`{0}` is not a package pattern; `visibility()` takes package patterns (`//pkg:` or `//pkg/...`)"
    )]
    NotPackagePattern(String),
    #[error("`\"{0}\"` is not valid in `visibility()`; use {1}")]
    WrongCase(String, &'static str),
    #[error("Negative patterns are not supported in `visibility()`: `{0}`")]
    NegativePattern(String),
    #[error("`\"private\"` must be the only entry when passed to `visibility()`")]
    PrivateNotAlone,
    #[error(
        "`{loader}` cannot load `{loaded}`: it is not visible to this package. `{loaded}` declares `visibility({visibility})`; to allow this load, its `visibility()` must include `\"{pattern}\"`."
    )]
    NotVisible {
        loader: CellPath,
        loaded: CellPath,
        visibility: String,
        /// The package pattern of the loader, as it would be written in the
        /// loaded file's `visibility()`.
        pattern: String,
    },
    #[error(
        "`{loader}` cannot load `{loaded}`: `{loaded}` declares `visibility({visibility})`, but `{loader}` is not in any package (there is no buildfile in its directory or any parent directory). Add a buildfile so it belongs to a package that `{loaded}`'s `visibility()` can name."
    )]
    NotVisibleNoPackage {
        loader: CellPath,
        loaded: CellPath,
        visibility: String,
    },
}

/// The `"private"` special token, meaning "loadable only from the same package".
/// Equivalent to passing an empty list.
const PRIVATE: &str = "private";
/// The lowercase Bazel-style spelling of `PUBLIC`. Both are accepted.
const PUBLIC_LOWER: &str = "public";

fn parse_load_visibility(
    value: Either<&str, UnpackListOrTuple<&str>>,
    cell_name: CellName,
    cell_resolver: &CellResolver,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<LoadVisibility> {
    let patterns: Vec<&str> = match value {
        Either::Left(s) => vec![s],
        Either::Right(list) => list.items,
    };

    // `"private"` is a same-package-only token that is not a package pattern, so
    // it is handled here rather than by the pattern parser. It is only
    // meaningful on its own (`visibility("private")` / `visibility(["private"])`).
    if patterns.iter().any(|s| *s == PRIVATE) {
        if patterns.len() != 1 {
            return Err(LoadVisibilityError::PrivateNotAlone.into());
        }
        return Ok(LoadVisibility::private());
    }

    let mut builder = VisibilityWithinViewBuilder::with_capacity(patterns.len());
    for s in patterns {
        if s == PUBLIC_LOWER || s == VisibilityPattern::PUBLIC {
            builder.add_public();
        } else if s.eq_ignore_ascii_case(PUBLIC_LOWER) {
            return Err(LoadVisibilityError::WrongCase(
                s.to_owned(),
                "`\"PUBLIC\"` or `\"public\"`",
            )
            .into());
        } else if s.eq_ignore_ascii_case(PRIVATE) {
            return Err(LoadVisibilityError::WrongCase(s.to_owned(), "`\"private\"`").into());
        } else if s.starts_with('-') {
            return Err(LoadVisibilityError::NegativePattern(s.to_owned()).into());
        } else if !s.contains(':') && !s.ends_with("...") {
            return Err(LoadVisibilityError::NotPackagePattern(s.to_owned()).into());
        } else {
            match ParsedPattern::parse_precise(s, cell_name, cell_resolver, cell_alias_resolver)? {
                ParsedPattern::Target(..) => {
                    return Err(LoadVisibilityError::TargetLabel(s.to_owned()).into());
                }
                p => builder.add(VisibilityPattern::Parsed(p)),
            }
        }
    }
    Ok(LoadVisibility::new(builder.build_visibility().0))
}

#[starlark_module]
pub(crate) fn register_load_visibility(globals: &mut GlobalsBuilder) {
    /// Set the load visibility of the current `.bzl` file: the set of packages
    /// whose `BUCK`/`.bzl` files may `load()` it.
    ///
    /// The argument is a list of package specifications (`//pkg:` or
    /// `//pkg/...`, as in target `visibility`), but target labels, negative
    /// specifications and `target_name_glob(...)` are not accepted. The `[]`
    /// around a single entry may be omitted.
    ///
    /// * `visibility(["PUBLIC"])` / `visibility("public")` — loadable from
    ///   anywhere (this is also the default when `visibility()` is not called).
    /// * `visibility([])` / `visibility("private")` — loadable only from the
    ///   same package.
    /// * `visibility(["//foo/...", "//bar:"])` — loadable from the given
    ///   packages (and, always, the file's own package).
    ///
    /// As in Bazel, the package of a file is the nearest directory at or above
    /// it that contains a buildfile, so `//foo/sub/defs.bzl` is in package
    /// `//foo:` unless `foo/sub` has its own buildfile. A file outside of any
    /// package can only load public `.bzl` files.
    ///
    /// It must be called at most once, at the top level of the `.bzl` file it
    /// applies to (not inside a function). Enforcement of the resulting
    /// visibility can be disabled with `buck2.check_load_visibility = false`.
    ///
    /// ```python
    /// # //mylib/internal_defs.bzl
    /// visibility(["//mylib/...", "//tests/mylib/..."])
    ///
    /// def helper(): ...
    /// ```
    fn visibility<'v>(
        #[starlark(require = pos)] value: Either<&'v str, UnpackListOrTuple<&'v str>>,
        eval: &mut Evaluator,
    ) -> starlark::Result<NoneType> {
        let build_context = BuildContext::from_context(eval)?;
        // Only meaningful in `.bzl` files.
        build_context.additional.require_bzl("visibility")?;

        // Must be called from the top level of the `.bzl` file it applies to. At
        // top level the call stack holds the module's frame plus this call's
        // frame; anything deeper means we were reached through a function,
        // either one in this file or one in a loaded helper (which would
        // silently set the visibility of the *loading* module).
        if eval.call_stack_count() != 2 {
            return Err(buck2_error::Error::from(LoadVisibilityError::NotTopLevel).into());
        }

        let load_visibility = parse_load_visibility(
            value,
            build_context.cell_info().name().name(),
            build_context.cell_info().cell_resolver(),
            build_context.cell_info().cell_alias_resolver(),
        )?;

        InterpreterExtraValue::get(eval.module())?
            .load_visibility
            .set(load_visibility)
            .map_err(|_| buck2_error::Error::from(LoadVisibilityError::AtMostOnce))?;

        Ok(NoneType)
    }
}
