/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt::Write;

use buck2_hash::BuckMutInsertionMap;
use itertools::Itertools;

use crate::query::syntax::simple::functions::helpers::QueryArgType;

pub struct MarkdownOptions {
    /// Termimad doesn't support links, see <https://github.com/Canop/termimad/issues/48>.
    /// Flag to disable generation of those for terminal / rendered outputs.
    pub links_enabled: bool,
}

// Instances created by #[query_module]
pub struct ArgDescription {
    pub name: String,
    pub repr_format: String,
    pub arg_type: QueryArgType,
}

impl ArgDescription {
    pub fn render_markdown(&self, options: &MarkdownOptions) -> String {
        format!(
            "{}: {}",
            self.name,
            self.repr_format
                .replace("{}", &self.arg_type.rendered_reference(options))
        )
    }
}

// Instances created by #[query_module]
pub struct FunctionDescription {
    pub name: &'static str,
    pub args: Vec<ArgDescription>,

    pub short_help: Option<String>,
    pub details: Option<String>,
}

impl FunctionDescription {
    fn rendered_reference(&self, options: &MarkdownOptions) -> Cow<'static, str> {
        if options.links_enabled {
            Cow::Owned(format!("[{}](#{})", self.name, self.name))
        } else {
            Cow::Borrowed(self.name)
        }
    }

    pub fn render_markdown_for_index(&self, options: &MarkdownOptions) -> String {
        format!(
            " - {}{}",
            self.rendered_reference(options),
            match &self.short_help {
                None => "".to_owned(),
                Some(v) => format!(": {v}"),
            }
        )
    }

    pub fn render_markdown_for_details(&self, options: &MarkdownOptions) -> String {
        let anchor = if options.links_enabled {
            &format!("{{#{}}}", self.name)
        } else {
            ""
        };
        let mut rendered = format!(
            "---\n#### {}({}){}\n\n",
            self.name,
            self.args
                .iter()
                .map(|v| v.render_markdown(options))
                .join(", "),
            anchor,
        );
        if let Some(v) = &self.short_help {
            writeln!(rendered, "{v}\n").unwrap();
        }

        if let Some(v) = &self.details {
            writeln!(rendered, "{v}\n").unwrap();
        }
        rendered
    }
}

// Instances created by #[query_module]
pub struct ModuleDescription {
    pub functions: BuckMutInsertionMap<&'static str, FunctionDescription>,

    pub short_help: Option<String>,
    pub details: Option<String>,
}

pub struct QueryEnvironmentDescription {
    pub name: String,
    pub mods: Vec<ModuleDescription>,
}

impl QueryEnvironmentDescription {
    pub fn render_markdown(&self, options: &MarkdownOptions) -> String {
        let merged_sorted_functions = self
            .mods
            .iter()
            .fold(BuckMutInsertionMap::default(), |acc, module| {
                acc.into_iter().chain(module.functions.iter()).collect()
            })
            .into_iter()
            .sorted_by(|(lhs, _), (rhs, _)| Ord::cmp(lhs, rhs));

        let function_index = merged_sorted_functions
            .clone()
            .map(|(_, f)| f.render_markdown_for_index(options))
            .join("\n");

        let functions = merged_sorted_functions
            .map(|(_, f)| f.render_markdown_for_details(options))
            .join("\n");

        let value_types = enum_iterator::all::<QueryArgType>()
            .map(|v| render_arg_type_markdown(v, options))
            .join("\n\n");

        format!(
            indoc::indoc! {r#"
            # {}

            ## Functions

            {}

            {}

            ## Value Types

            {}
            "#},
            &self.name, function_index, functions, value_types
        )
    }
}

fn render_arg_type_markdown(v: QueryArgType, options: &MarkdownOptions) -> String {
    let anchor = if options.links_enabled {
        &format!("{{#{}}}", v.internal_link_id())
    } else {
        ""
    };
    let mut rendered = format!("### *{}*{}\n\n", v.repr(), anchor);
    if let Some(short_description) = v.short_description(options) {
        rendered.push_str(short_description.as_ref());
    }
    if let Some(description) = v.description() {
        rendered.push_str(&format!("\n\n{description}"));
    }
    rendered
}
