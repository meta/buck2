/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::ValueOfUnchecked;
use starlark::values::list::AllocList;
use starlark::values::list::UnpackList;
use starlark::values::list_or_tuple::UnpackListOrTuple;

use crate::interpreter::build_context::BuildContext;
use crate::interpreter::globspec::GlobSpec;
use crate::interpreter::module_internals::ModuleInternals;

#[starlark_module]
pub(crate) fn register_path(builder: &mut GlobalsBuilder) {
    /// The `glob()` function specifies a set of files using patterns.
    /// Only available from `BUCK` files.
    ///
    /// A typical `glob` call looks like:
    ///
    /// ```python
    /// glob(["foo/**/*.h"])
    /// ```
    ///
    /// This call will match all header files in the `foo` directory, recursively.
    ///
    /// You can also pass a named `exclude` parameter to remove files matching a pattern:
    ///
    /// ```python
    /// glob(["foo/**/*.h"], exclude = ["**/config.h"])
    /// ```
    ///
    /// This call will remove all `config.h` files from the initial match.
    ///
    /// The `glob()` call is evaluated against the list of files owned by this `BUCK` file.
    /// A file is owned by whichever `BUCK` file is closest above it - so given `foo/BUCK` and
    /// `foo/bar/BUCK` the file `foo/file.txt` would be owned by `foo/BUCK` (and available from
    /// its `glob` results) but the file `foo/bar/file.txt` would be owned by `foo/bar/BUCk`
    /// and _not_ appear in the glob result of `foo/BUCK`, even if you write `glob(["bar/file.txt"])`.
    /// As a consequence of this rule, `glob(["../foo.txt"])` will always return an empty list of files.
    ///
    /// Currently `glob` is evaluated case-insensitively on all file systems, but we expect
    /// that to change to case sensitive in the near future.
    fn glob<'v>(
        include: UnpackListOrTuple<String>,
        #[starlark(require = named, default=UnpackListOrTuple::default())]
        exclude: UnpackListOrTuple<String>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueOfUnchecked<'v, UnpackList<String>>> {
        let extra = ModuleInternals::from_context(eval, "glob")?;
        let spec = GlobSpec::new(&include.items, &exclude.items)?;
        let res = extra.resolve_glob(&spec).map(|path| path.as_str());
        Ok(eval.heap().alloc_typed_unchecked(AllocList(res)).cast())
    }

    /// `package_name()` can only be called in buildfiles (e.g. BUCK files) or PACKAGE files, and returns the name of the package.
    /// E.g. inside `foo//bar/baz/BUCK` the output will be `bar/baz`.
    /// E.g. inside `foo//bar/PACKAGE` the output will be `bar`.
    fn package_name(eval: &mut Evaluator) -> starlark::Result<String> {
        // An (IMO) unfortunate choice in the skylark api is that this just gives the cell-relative
        //  path of the package (which isn't a unique "name" for the package)
        Ok(BuildContext::from_context(eval)?
            .base_path()?
            .path()
            .to_string())
    }

    /// `get_base_path()` can only be called in buildfiles (e.g. BUCK files) or PACKAGE files, and returns the name of the package.
    /// E.g. inside `foo//bar/baz/BUCK` the output will be `bar/baz`.
    /// E.g. inside `foo//bar/PACKAGE` the output will be `bar`.
    ///
    /// This function is identical to `package_name`.
    fn get_base_path(eval: &mut Evaluator) -> starlark::Result<String> {
        Ok(BuildContext::from_context(eval)?
            .base_path()?
            .path()
            .to_string())
    }

    /// Like `get_cell_name()` but prepends a leading `@` for compatibility with Buck1.
    /// You should call `get_cell_name()` instead, and if you really want the `@`,
    /// prepend it yourself.
    fn repository_name(eval: &mut Evaluator) -> starlark::Result<String> {
        // In Buck v1 the repository name has a leading `@` on it, so match that with v2.
        // In practice, most users do `repository_name()[1:]` to drop it.
        Ok(format!(
            "@{}",
            BuildContext::from_context(eval)?.cell_info().name()
        ))
    }

    /// `get_cell_name()` can be called from either a `BUCK` file or a `.bzl` file,
    /// and returns the name of the cell where the `BUCK` file that started the call
    /// lives.
    ///
    /// For example, inside `foo//bar/baz/BUCK` the output will be `foo`.
    /// If that `BUCK` file does a `load("@hello//:world.bzl", "something")` then
    /// the result in that `.bzl` file will also be `foo`.
    ///
    /// ### Cell segmentation
    ///
    /// Historically, Buck has re-evaluated each .bzl file once per cell it is imported
    /// into. That meant that `get_cell_name` would be based on where the chain of imports
    /// started. Re-evaluating came with undesirable behaviour for cross-cell type checking
    /// so this behaviour can now be disabled, but be aware that this affects get_cell_name().
    ///
    /// In the prelude, and when you disable this behaviour for all cells with the
    /// buckconfig `buck2.disable_cell_segmentation`, the returned cell name depends on
    /// the function call stack.
    ///
    /// ```python
    /// # root .buckconfig
    /// [buck2]
    ///     disable_cell_segmentation = true
    ///
    /// # hello//:world.bzl
    /// HELLO_CELL_NAME = get_cell_name()
    /// def dynamic_cell_name() -> str:
    ///     return get_cell_name()
    ///
    /// # root//BUCK
    /// load("@hello//:world.bzl", "HELLO_CELL_NAME", "dynamic_cell_name")
    /// print(HELLO_CELL_NAME) # hello
    /// print(dynamic_cell_name()) # root
    /// ```
    fn get_cell_name(eval: &mut Evaluator) -> starlark::Result<String> {
        Ok(BuildContext::from_context(eval)?
            .cell_info()
            .name()
            .to_string())
    }
}
