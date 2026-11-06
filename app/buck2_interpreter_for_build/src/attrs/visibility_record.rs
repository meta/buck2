/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_node::visibility::StarlarkTargetNameGlob;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;

#[starlark_module]
pub(crate) fn register_target_name_glob(builder: &mut GlobalsBuilder) {
    /// Creates a target-name glob pattern used inside `visibility` /
    /// `within_view` attribute lists.
    ///
    /// `name_globs` is a non-empty list of glob patterns (`*` is the only
    /// wildcard) that match against target names. A target matches the
    /// `target_name_glob` if **any** glob in the list matches its name. A glob
    /// consisting solely of `*` is rejected — use `"PUBLIC"` for unrestricted
    /// visibility instead.
    ///
    /// The optional `within` parameter is a list of package or recursive
    /// package patterns (e.g. `["//pkg:", "//other/..."]`) that scope the
    /// match: a target matches only if its package falls inside **any** of the
    /// `within` scopes. If omitted or empty, the globs match targets in any
    /// package.
    ///
    /// Each `within` pattern is parsed when the enclosing target's `visibility`
    /// / `within_view` attribute is coerced, using the cell aliases of the file
    /// that target is defined in. `"PUBLIC"` is not a valid `within` scope (it is
    /// a target name, not a package); omit `within` to match any package.
    ///
    /// Example usage:
    /// ```python
    /// visibility = [
    ///     target_name_glob(["*-deprecated"]),
    ///     target_name_glob(["*-test", "*-bench"], within = ["//pkg/..."]),
    /// ]
    /// ```
    #[starlark(as_type = StarlarkTargetNameGlob)]
    fn target_name_glob<'v>(
        #[starlark(require = pos)] name_globs: ValueOf<'v, ListOrTupleType<&'v str>>,
        #[starlark(require = named, default = ValueOf::empty_list())] within: ValueOf<
            'v,
            ListOrTupleType<&'v str>,
        >,
    ) -> starlark::Result<StarlarkTargetNameGlob> {
        let name_globs = name_globs
            .try_items()
            .map(|s| s.map(str::to_owned))
            .collect::<starlark::Result<_>>()?;
        let within = within
            .try_items()
            .map(|s| s.map(str::to_owned))
            .collect::<starlark::Result<_>>()?;
        Ok(StarlarkTargetNameGlob::try_new(name_globs, within)?)
    }
}
