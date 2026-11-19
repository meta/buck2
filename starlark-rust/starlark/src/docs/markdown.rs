/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::fmt::Write;
use std::iter;
use std::sync::LazyLock;

use itertools::Itertools;
use regex::Regex;

use crate::docs::DocFunction;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocModule;
use crate::docs::DocParam;
use crate::docs::DocProperty;
use crate::docs::DocString;
use crate::docs::DocType;
use crate::docs::multipage::RenderConfig;
use crate::typing::Ty;
use crate::typing::ty::TypeRenderConfig;

/// What to render from a [`DocString`].
enum DSOpts {
    /// Just the summary.
    Summary,
    /// Just the details (if present).
    Details,
    /// Just the examples section (if present)
    Examples,
    /// Both the summary, details, and examples separated in an appropriate fashion.
    Combined,
}

fn render_doc_string(opts: DSOpts, string: &Option<DocString>) -> Option<String> {
    string.as_ref().and_then(|d| match opts {
        DSOpts::Summary => Some(d.summary.clone()),
        DSOpts::Details => d.details.clone(),
        DSOpts::Examples => d.examples.clone(),
        DSOpts::Combined => Some(match (&d.details, &d.examples) {
            (Some(details), Some(examples)) => {
                format!("{}\n\n{}\n\nExamples:\n{}", d.summary, details, examples)
            }
            (Some(details), None) => format!("{}\n\n{}", d.summary, details),
            (None, Some(examples)) => format!("{}\n\nExamples:\n{}", d.summary, examples),
            (None, None) => d.summary.clone(),
        }),
    })
}

/// Function names can have underscores in them, which are markdown,
/// so escape them if we render them outside a codeblock.
fn escape_name(name: &str) -> String {
    name.replace('_', "\\_")
}

fn render_property(name: &str, property: &DocProperty, render_config: &RenderConfig) -> String {
    let prototype = render_code_block(
        &format!(
            "{name}: {}",
            property.typ.display_with(&render_config.type_config)
        ),
        &render_config.type_config,
    );
    let header = format!("## {}\n\n{prototype}", escape_name(name));
    let summary = render_doc_string(DSOpts::Summary, &property.docs);
    let details = render_doc_string(DSOpts::Details, &property.docs);

    let mut body = header;
    if let Some(summary) = summary {
        body.push_str("\n\n");
        body.push_str(&summary);
    }
    if let Some(details) = details {
        body.push_str("\n\n");
        body.push_str(&details);
    }

    body
}

/// If there are any parameter docs to render, render them as a list.
fn render_function_parameters<'a>(
    params: impl IntoIterator<Item = (String, &'a DocParam)>,
) -> Option<String> {
    let mut param_list: Option<String> = None;
    for (name, p) in params {
        let DocParam {
            docs,
            default_value,
            ..
        } = p;

        if docs.is_none() {
            continue;
        }

        let param_list = param_list.get_or_insert_with(String::new);

        let docs = render_doc_string(DSOpts::Combined, docs).unwrap_or_default();

        let mut lines_iter = docs.lines();
        if let Some(first_line) = lines_iter.next() {
            let default = match default_value {
                Some(v) => format!(" (defaults to: `{v}`)"),
                None => " (required)".to_owned(),
            };

            let _ = writeln!(param_list, "* `{name}`:{default}\n");
            let _ = writeln!(param_list, "  {first_line}\n");
            for line in lines_iter {
                let _ = writeln!(param_list, "  {line}");
            }
        } else {
            let _ = writeln!(param_list, "* `{name}`");
        }
    }

    param_list
}

fn render_function(
    name: &str,
    function: &DocFunction,
    include_header: bool,
    render_config: &RenderConfig,
) -> String {
    // Render the layouts differently based on the configs provided
    match render_config.layout_config {
        LayoutRenderConfig::SignatureAtBottom => {
            render_signature_at_bottom_layout(name, function, render_config)
        }
        LayoutRenderConfig::Default => {
            render_default_layout(name, function, include_header, render_config)
        }
    }
}

fn render_default_layout(
    name: &str,
    function: &DocFunction,
    include_header: bool,
    render_config: &RenderConfig,
) -> String {
    let prototype = render_code_block(
        &render_function_prototype(name, function, &render_config.type_config),
        &render_config.type_config,
    );
    let header = if include_header {
        format!("## {}\n\n{prototype}", escape_name(name))
    } else {
        prototype
    };

    let summary = render_doc_string(DSOpts::Summary, &function.docs);
    let details = render_doc_string(DSOpts::Details, &function.docs);
    let examples = render_doc_string(DSOpts::Examples, &function.docs);

    let parameter_docs =
        render_function_parameters(function.params.doc_params_with_starred_names());
    let return_docs = render_doc_string(DSOpts::Combined, &function.ret.docs);

    let mut body = header;
    if let Some(summary) = &summary {
        body.push_str("\n\n");
        body.push_str(summary);
    }
    if let Some(parameter_docs) = &parameter_docs {
        body.push_str("\n\n#### Parameters\n\n");
        body.push_str(parameter_docs);
    }
    if let Some(returns) = &return_docs {
        body.push_str("\n\n#### Returns\n\n");
        body.push_str(returns);
    }
    if let Some(details) = &details {
        if parameter_docs.is_some() || return_docs.is_some() {
            body.push_str("\n\n#### Details\n\n");
        } else {
            // No need to aggressively separate the defaults from the summary if there
            // was nothing in between them. Just let it flow.
            body.push_str("\n\n");
        }
        body.push_str(details);
    }
    if let Some(examples) = &examples {
        body.push_str("\n\n#### Examples\n\n");
        body.push_str(examples);
    }

    body
}

fn render_signature_at_bottom_layout(
    name: &str,
    function: &DocFunction,
    render_config: &RenderConfig,
) -> String {
    let prototype = render_code_block(
        &render_function_prototype(name, function, &render_config.type_config),
        &render_config.type_config,
    );

    let summary = render_doc_string(DSOpts::Summary, &function.docs);
    let details = render_doc_string(DSOpts::Details, &function.docs);
    let examples = render_doc_string(DSOpts::Examples, &function.docs);

    let parameter_docs =
        render_function_parameters(function.params.doc_params_with_starred_names());

    let mut body = "".to_owned();
    if let Some(summary) = &summary {
        body.push_str(summary);
    }
    if let Some(details) = &details {
        body.push_str("\n\n### Details\n\n");
        body.push_str(details);
    }
    body.push_str("\n\n");
    body.push_str(&format!("### Function Signature\n\n{prototype}"));
    if let Some(parameter_docs) = &parameter_docs {
        body.push_str("\n\n### Parameters\n\n");
        body.push_str(parameter_docs);
    }
    if let Some(examples) = &examples {
        body.push_str("\n\n### Examples\n\n");
        body.push_str(&render_strings_with_code_blocks(
            examples,
            &render_config.type_config,
        ));
    }

    body
}

pub(super) fn render_members<'a>(
    name: &str,
    docs: &Option<DocString>,
    prefix: &str,
    members: impl IntoIterator<Item = (&'a str, DocMember)>,
    after_summary: Option<String>,
    render_config: &RenderConfig,
) -> String {
    let summary = render_doc_string(DSOpts::Combined, docs)
        .map(|s| format!("\n\n{s}"))
        .unwrap_or_default();

    let member_details = members
        .into_iter()
        .sorted_by(|(l_m, _), (r_m, _)| l_m.cmp(r_m))
        .map(|(child, member)| {
            render_doc_member(&format!("{prefix}{child}"), &member, render_config)
        });
    let member_details: Vec<_> = after_summary.into_iter().chain(member_details).collect();
    let members_details = member_details.join("\n\n---\n\n");

    let header = if name == "" {
        "".to_owned()
    } else {
        format!("# {name}")
    };

    format!("{header}{summary}\n\n{members_details}")
}

pub(super) fn render_doc_type(
    name: &str,
    prefix: &str,
    t: &DocType,
    render_config: &RenderConfig,
) -> String {
    let constructor = t
        .constructor
        .as_ref()
        .map(|c| render_function(name, c, false, render_config));
    render_members(
        name,
        &t.docs,
        prefix,
        t.members.iter().map(|(n, m)| (&**n, m.clone())),
        constructor,
        render_config,
    )
}

/// Used by LSP.
/// It will not render the type signatures with link to types
pub fn render_doc_item_no_link(name: &str, item: &DocItem) -> String {
    render_doc_item(
        name,
        item,
        &RenderConfig {
            type_config: TypeRenderConfig::Default,
            layout_config: LayoutRenderConfig::Default,
        },
    )
}

pub fn render_doc_item(name: &str, item: &DocItem, render_config: &RenderConfig) -> String {
    match item {
        DocItem::Module(m) => render_members(
            name,
            &m.docs,
            "",
            m.members.iter().filter_map(|(n, m)| {
                m.try_as_member_with_collapsed_object()
                    .ok()
                    .map(|m| (&**n, m))
            }),
            None,
            render_config,
        ),
        DocItem::Type(o) => render_doc_type(
            &format!("`{name}` type"),
            &format!("{name}."),
            o,
            render_config,
        ),
        DocItem::Member(DocMember::Function(f)) => render_function(name, f, true, render_config),
        DocItem::Member(DocMember::Property(p)) => render_property(name, p, render_config),
    }
}

/// Used by LSP.
pub fn render_doc_member(name: &str, item: &DocMember, render_config: &RenderConfig) -> String {
    match item {
        DocMember::Function(f) => render_function(name, f, true, render_config),
        DocMember::Property(p) => render_property(name, p, render_config),
    }
}

/// Used by LSP.
pub fn render_doc_param(starred_name: String, item: &DocParam) -> String {
    render_function_parameters(iter::once((starred_name, item))).unwrap_or_default()
}

/// Any functions with more parameters than this will have
/// their prototype split over multiple lines. Otherwise, it is returned as
/// a single line.
const MAX_ARGS_BEFORE_MULTILINE: usize = 3;

/// If the prototype ends up longer than this length, we'll split it anyway
const MAX_LENGTH_BEFORE_MULTILINE: usize = 80;

fn raw_type_prefix(prefix: &str, t: &Ty, render_config: &TypeRenderConfig) -> String {
    if t.is_any() {
        String::new()
    } else {
        format!("{prefix}{}", t.display_with(render_config))
    }
}

fn render_function_prototype(
    function_name: &str,
    f: &DocFunction,
    render_config: &TypeRenderConfig,
) -> String {
    let ret_type = raw_type_prefix(" -> ", &f.ret.typ, render_config);
    let prefix = format!("def {function_name}");
    let one_line_params = f.params.render_code(None, render_config);
    let single_line_result = format!("{prefix}({one_line_params}){ret_type}");

    if f.params.doc_params().count() > MAX_ARGS_BEFORE_MULTILINE
        || single_line_result.len() > MAX_LENGTH_BEFORE_MULTILINE
    {
        let chunked_params = f.params.render_code(Some("    "), render_config);
        format!("{prefix}(\n{chunked_params}){ret_type}")
    } else {
        single_line_result
    }
}

fn render_strings_with_code_blocks(contents: &str, render_config: &TypeRenderConfig) -> String {
    let re = LazyLock::new(|| Regex::new(r"```([\s\S]*?)```").unwrap());
    re.replace_all(contents, |captures: &regex::Captures| {
        render_code_block(&captures[1], render_config)
    })
    .to_string()
}

// For LikedType render in markdown, for code block ``` ``` we cannot contain the link in it
// We need to use the html block here,
// example:
// <pre class="language-python">
//  <code>
//    def soome_function() -> <Link to="/path/to/type">Artifact</Link>
//  </code>
//</pre>
fn render_code_block(contents: &str, render_config: &TypeRenderConfig) -> String {
    match render_config {
        TypeRenderConfig::Default => format!("```python\n{contents}\n```"),
        TypeRenderConfig::LinkedType {
            render_linked_ty_starlark_value: _,
        } => {
            format!(r#"<pre class="language-python"><code>{contents}</code></pre>"#)
        }
    }
}

impl DocModule {
    pub(super) fn render_markdown_page_for_multipage_render(
        &self,
        name: &str,
        render_config: &RenderConfig,
    ) -> String {
        render_members(
            name,
            &self.docs,
            "",
            self.members
                .iter()
                .filter_map(|(n, m)| m.try_as_member().map(|m| (&**n, m))),
            None,
            render_config,
        )
    }
}

impl DocType {
    pub(super) fn render_markdown_page_for_multipage_render(
        &self,
        name: &str,
        render_config: &RenderConfig,
    ) -> String {
        render_doc_type(name, &format!("{name}."), self, render_config)
    }
}

/// Configuration for layout rendering.
pub enum LayoutRenderConfig {
    Default,
    /// Renders the summary + detail above function signature.
    SignatureAtBottom,
}
