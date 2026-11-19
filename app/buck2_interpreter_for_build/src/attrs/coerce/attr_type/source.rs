/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::source::SourceAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::typing::Ty;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum SourceLabelCoercionError {
    #[error(
        "Couldn't coerce `{0}` as a source.\n  Error when treated as a target: {1:#}\n  Error when treated as a path: {2:#}"
    )]
    CoercionFailed(String, buck2_error::Error, buck2_error::Error),
}

/// Try cleaning up irrelevant details users often type
fn cleanup_path(value: &str) -> &str {
    let value = value.strip_prefix("./").unwrap_or(value);
    let value = value.strip_suffix("/").unwrap_or(value);
    if value == "." { "" } else { value }
}

impl AttrTypeCoerce for SourceAttrType {
    fn coerce_item<'v>(
        &self,
        _configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let source_label = value.unpack_str_err()?;

        // A label needs a `:` (`//foo:bar`, `:bar`) unless the target name is being inferred from
        // the package name, in which case `//foo/bar` means `//foo/bar:bar`. `//` can't appear in a
        // valid path, so testing for it doesn't cost us any path coercions.
        let label_err = if source_label.contains(':') || source_label.contains("//") {
            match ctx.coerce_providers_label(source_label) {
                Ok(l) => return ctx.attr_arena().encode_source_label(&l),
                Err(e) => Some(e),
            }
        } else {
            // As an optimization, we skip trying to parse as a label in this case
            None
        };

        let path_err = match ctx.coerce_path(cleanup_path(source_label), self.allow_directory) {
            Ok(path) => return ctx.attr_arena().encode_source_file(&path),
            Err(path_err) => path_err,
        };

        if let Some(label_err) = label_err {
            Err(
                SourceLabelCoercionError::CoercionFailed(value.to_str(), label_err, path_err)
                    .into(),
            )
        } else {
            Err(path_err.context(format!("Coercing `{}` as a source", value.to_str())))
        }
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(Ty::string())
    }
}
