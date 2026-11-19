/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;

use buck2_core::package::source_path::SourcePathRef;
use dupe::Dupe;
use either::Either;
use serde_json::to_value;

use crate::attrs::attr_type::any_matches::AnyMatches;
use crate::attrs::coerced_path::CoercedPath;
use crate::attrs::configured_attr::ConfiguredAttr;
use crate::attrs::fmt_context::AttrFmtContext;
use crate::attrs::json::ToJsonWithContext;

impl ToJsonWithContext for ConfiguredAttr {
    fn to_json(&self, ctx: &AttrFmtContext) -> buck2_error::Result<serde_json::Value> {
        match self {
            ConfiguredAttr::Bool(v) => Ok(to_value(v)?),
            ConfiguredAttr::Int(v) => Ok(to_value(v)?),
            ConfiguredAttr::String(v) | ConfiguredAttr::EnumVariant(v) => Ok(to_value(v)?),
            ConfiguredAttr::List(list) => list.to_json(ctx),
            ConfiguredAttr::Tuple(list) => list.to_json(ctx),
            ConfiguredAttr::Dict(dict) => dict.to_json(ctx),
            ConfiguredAttr::None => Ok(serde_json::Value::Null),
            ConfiguredAttr::OneOf(box l, _) => l.to_json(ctx),
            ConfiguredAttr::Visibility(v) => Ok(v.to_json()),
            ConfiguredAttr::WithinView(v) => Ok(v.to_json()),
            ConfiguredAttr::ExplicitConfiguredDep(e) => e.to_json(),
            ConfiguredAttr::TransitionDep(e) => e.to_json(),
            ConfiguredAttr::SplitTransitionDep(e) => e.to_json(),
            ConfiguredAttr::ConfigurationDep(e) => Ok(to_value(e.to_string())?),
            ConfiguredAttr::PluginDep(e, _) => Ok(to_value(e.to_string())?),
            ConfiguredAttr::Dep(e) => Ok(to_value(e.to_string())?),
            ConfiguredAttr::SourceLabel(e) => Ok(to_value(e.to_string())?),
            ConfiguredAttr::Label(e) => Ok(to_value(e.to_string())?),
            ConfiguredAttr::Arg(e) => Ok(to_value(e.to_string())?),
            ConfiguredAttr::Query(e) => Ok(to_value(&e.query.query)?),
            ConfiguredAttr::SourceFile(e) => Ok(to_value(source_file_display(ctx, e).to_string())?),
            ConfiguredAttr::Metadata(m) => Ok(m.to_value()),
            ConfiguredAttr::TargetModifiers(m) => Ok(m.to_json()),
        }
    }
}

impl AnyMatches for ConfiguredAttr {
    fn any_matches(
        &self,
        filter: &dyn Fn(&str) -> buck2_error::Result<bool>,
    ) -> buck2_error::Result<bool> {
        match self {
            ConfiguredAttr::String(v) | ConfiguredAttr::EnumVariant(v) => filter(v),
            ConfiguredAttr::List(vals) => vals.any_matches(filter),
            ConfiguredAttr::Tuple(vals) => vals.any_matches(filter),
            ConfiguredAttr::Dict(d) => d.any_matches(filter),
            ConfiguredAttr::None => Ok(false),
            ConfiguredAttr::Bool(b) => b.any_matches(filter),
            ConfiguredAttr::Int(i) => filter(&i.to_string()),
            ConfiguredAttr::OneOf(l, _) => l.any_matches(filter),
            ConfiguredAttr::Visibility(v) => v.any_matches(filter),
            ConfiguredAttr::WithinView(v) => v.any_matches(filter),
            ConfiguredAttr::ExplicitConfiguredDep(e) => e.any_matches(filter),
            ConfiguredAttr::TransitionDep(e) => e.any_matches(filter),
            ConfiguredAttr::SplitTransitionDep(e) => e.any_matches(filter),
            ConfiguredAttr::ConfigurationDep(e) => filter(&e.to_string()),
            ConfiguredAttr::PluginDep(e, _) => filter(&e.to_string()),
            ConfiguredAttr::Dep(e) => filter(&e.to_string()),
            ConfiguredAttr::SourceLabel(e) => filter(&e.to_string()),
            ConfiguredAttr::Label(e) => filter(&e.to_string()),
            ConfiguredAttr::Arg(e) => filter(&e.to_string()),
            ConfiguredAttr::Query(e) => filter(&e.query.query),
            ConfiguredAttr::SourceFile(e) => filter(&e.path().to_string()),
            ConfiguredAttr::Metadata(e) => e.any_matches(filter),
            ConfiguredAttr::TargetModifiers(e) => e.any_matches(filter),
        }
    }
}

pub(crate) fn source_file_display<'a>(
    ctx: &'a AttrFmtContext,
    source_file: &'a CoercedPath,
) -> impl Display + 'a {
    match &ctx.package {
        Some(pkg) => Either::Left(SourcePathRef::new(pkg.dupe(), source_file.path())),
        None => {
            // This code is unreachable, but better this than panic.
            Either::Right(format!("<no package>/{}", source_file.path()))
        }
    }
}
