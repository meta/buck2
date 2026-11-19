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

//! Collection of implementations for completions, and related types.

use std::collections::HashMap;
use std::path::Path;

use lsp_types::CompletionItem;
use lsp_types::CompletionItemKind;
use lsp_types::CompletionTextEdit;
use lsp_types::Documentation;
use lsp_types::MarkupContent;
use lsp_types::MarkupKind;
use lsp_types::Range;
use lsp_types::TextEdit;
use starlark::codemap::ResolvedSpan;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::markdown::render_doc_item_no_link;
use starlark::docs::markdown::render_doc_param;
use starlark_syntax::codemap::ResolvedPos;
use starlark_syntax::syntax::ast::StmtP;
use starlark_syntax::syntax::module::AstModuleFields;

use crate::definition::Definition;
use crate::definition::DottedDefinition;
use crate::definition::IdentifierDefinition;
use crate::definition::LspModule;
use crate::exported::SymbolKind as ExportedSymbolKind;
use crate::server::Backend;
use crate::server::LspContext;
use crate::server::LspOpError;
use crate::server::LspUri;
use crate::symbols::SymbolKind;
use crate::symbols::find_symbols_at_location;

/// The context in which to offer string completion options.
#[derive(Debug, PartialEq)]
pub enum StringCompletionType {
    /// The first argument to a `load` statement.
    LoadPath,
    /// A string in another context.
    String,
}

/// A possible result in auto-complete for a string context.
#[derive(Debug, PartialEq)]
pub struct StringCompletionResult {
    /// The value to complete.
    pub value: String,
    /// The text to insert, if different from the value.
    pub insert_text: Option<String>,
    /// From where to start the insertion, compared to the start of the string.
    pub insert_text_offset: usize,
    /// The kind of result, e.g. a file vs a folder.
    pub kind: CompletionItemKind,
}

impl<T: LspContext> Backend<T> {
    pub(crate) fn default_completion_options(
        &self,
        document_uri: &LspUri,
        document: &LspModule,
        line: u32,
        character: u32,
        workspace_root: Option<&Path>,
    ) -> impl Iterator<Item = CompletionItem> + '_ + use<'_, T> {
        let cursor_position = ResolvedPos {
            line: line as usize,
            column: character as usize,
        };

        // Scan through current document
        let mut symbols: HashMap<_, _> = find_symbols_at_location(
            document.ast.codemap(),
            document.ast.statement(),
            cursor_position,
        )
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                CompletionItem {
                    kind: Some(match value.kind {
                        SymbolKind::Method => CompletionItemKind::METHOD,
                        SymbolKind::Variable => CompletionItemKind::VARIABLE,
                    }),
                    detail: value.detail,
                    documentation: value
                        .doc
                        .map(|doc| {
                            Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: render_doc_item_no_link(&value.name, &doc),
                            })
                        })
                        .or_else(|| {
                            value.param.map(|(starred_name, doc)| {
                                Documentation::MarkupContent(MarkupContent {
                                    kind: MarkupKind::Markdown,
                                    value: render_doc_param(starred_name, &doc),
                                })
                            })
                        }),
                    label: value.name,
                    ..Default::default()
                },
            )
        })
        .collect();

        // Discover exported symbols from other documents
        let docs = self.last_valid_parse.read().unwrap();
        if docs.len() > 1 {
            // Find the position of the last load in the current file.
            let mut last_load = None;
            let mut loads = HashMap::new();
            document.ast.statement().visit_stmt(|node| {
                if let StmtP::Load(load) = &node.node {
                    last_load = Some(node.span);
                    loads.insert(load.module.node.clone(), (load.args.clone(), node.span));
                }
            });
            let last_load = last_load.map(|span| document.ast.codemap().resolve_span(span));

            symbols.extend(
                self.get_all_exported_symbols(
                    Some(document_uri),
                    &symbols,
                    workspace_root,
                    document_uri,
                    |module, symbol| {
                        Self::get_load_text_edit(
                            module,
                            symbol,
                            document,
                            last_load,
                            loads.get(module),
                        )
                    },
                )
                .into_iter()
                .map(|item| (item.label.clone(), item)),
            );
        }

        symbols
            .into_values()
            .chain(self.get_global_symbol_completion_items(document_uri))
            .chain(Self::get_keyword_completion_items())
    }

    pub(crate) fn exported_symbol_options(
        &self,
        load_path: &str,
        current_span: ResolvedSpan,
        previously_loaded: &[String],
        document_uri: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Vec<CompletionItem> {
        self.context
            .resolve_load(load_path, document_uri, workspace_root)
            // FIXME(JakobDegen): Why are we throwing away errors?
            .map_err(|_| ())
            .and_then(|uri| self.get_ast_or_load_from_disk(&uri).map_err(|_| ()))
            .into_iter()
            .flatten()
            .flat_map(|ast| {
                ast.get_exported_symbols()
                    .into_iter()
                    .filter(|symbol| !previously_loaded.iter().any(|s| s == &symbol.name))
                    .map(|symbol| {
                        let mut item: CompletionItem = symbol.into();
                        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                            range: current_span.into(),
                            new_text: item.label.clone(),
                        }));
                        item
                    })
            })
            .collect()
    }

    pub(crate) fn parameter_name_options(
        &self,
        function_name_span: &ResolvedSpan,
        document: &LspModule,
        document_uri: &LspUri,
        previously_used_named_parameters: &[String],
        workspace_root: Option<&Path>,
    ) -> impl Iterator<Item = CompletionItem> + use<T> {
        match document.find_definition_at_location(
            function_name_span.begin.line as u32,
            function_name_span.begin.column as u32,
        ) {
            Definition::Identifier(identifier) => self
                .parameter_name_options_for_identifier_definition(
                    &identifier,
                    document,
                    document_uri,
                    previously_used_named_parameters,
                    workspace_root,
                )
                .unwrap_or_default(),
            Definition::Dotted(DottedDefinition {
                root_definition_location,
                ..
            }) => self
                .parameter_name_options_for_identifier_definition(
                    &root_definition_location,
                    document,
                    document_uri,
                    previously_used_named_parameters,
                    workspace_root,
                )
                .unwrap_or_default(),
        }
        .into_iter()
        .flatten()
    }

    fn parameter_name_options_for_identifier_definition(
        &self,
        identifier_definition: &IdentifierDefinition,
        document: &LspModule,
        document_uri: &LspUri,
        previously_used_named_parameters: &[String],
        workspace_root: Option<&Path>,
    ) -> Result<Option<Vec<CompletionItem>>, LspOpError> {
        Ok(match identifier_definition {
            IdentifierDefinition::Location {
                destination, name, ..
            } => {
                // Can we resolve it again at that location?
                // TODO: This seems very inefficient. Once the document starts
                // holding the `Scope` including AST nodes, this indirection
                // should be removed.
                find_symbols_at_location(
                    document.ast.codemap(),
                    document.ast.statement(),
                    ResolvedPos {
                        line: destination.begin.line,
                        column: destination.begin.column,
                    },
                )
                .remove(name)
                .and_then(|symbol| match symbol.kind {
                    SymbolKind::Method => symbol.doc,
                    SymbolKind::Variable => None,
                })
                .and_then(|docs| match docs {
                    DocItem::Member(DocMember::Function(doc_function)) => Some(
                        doc_function
                            .params
                            .regular_params()
                            .filter(|p| !previously_used_named_parameters.contains(&p.name))
                            .map(|p| CompletionItem {
                                label: p.name.to_owned(),
                                kind: Some(CompletionItemKind::PROPERTY),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                    _ => None,
                })
            }
            IdentifierDefinition::LoadedLocation { path, name, .. } => {
                let load_uri = self.resolve_load_path(path, document_uri, workspace_root)?;
                self.get_ast_or_load_from_disk(&load_uri)?
                    .and_then(|ast| ast.find_exported_symbol(name))
                    .and_then(|symbol| match symbol.kind {
                        ExportedSymbolKind::Any => None,
                        ExportedSymbolKind::Function { argument_names } => Some(
                            argument_names
                                .into_iter()
                                .map(|name| CompletionItem {
                                    label: name,
                                    kind: Some(CompletionItemKind::PROPERTY),
                                    ..Default::default()
                                })
                                .collect(),
                        ),
                    })
            }
            IdentifierDefinition::Unresolved { name, .. } => {
                // Maybe it's a global symbol.
                match self
                    .context
                    .get_environment(document_uri)
                    .members
                    .into_iter()
                    .find(|symbol| &symbol.0 == name)
                {
                    Some(symbol) => match symbol.1 {
                        DocItem::Member(DocMember::Function(doc_function)) => Some(
                            doc_function
                                .params
                                .regular_params()
                                .map(|param| CompletionItem {
                                    label: param.name.to_owned(),
                                    kind: Some(CompletionItemKind::PROPERTY),
                                    ..Default::default()
                                })
                                .collect(),
                        ),
                        _ => None,
                    },
                    _ => None,
                }
            }
            // None of these can be functions, so can't have any parameters.
            IdentifierDefinition::LoadPath { .. }
            | IdentifierDefinition::StringLiteral { .. }
            | IdentifierDefinition::NotFound => None,
        })
    }

    pub(crate) fn string_completion_options(
        &self,
        document_uri: &LspUri,
        kind: StringCompletionType,
        current_value: &str,
        current_span: ResolvedSpan,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<CompletionItem>, LspOpError> {
        Ok(self
            .context
            .get_string_completion_options(document_uri, kind, current_value, workspace_root)
            .map_err(LspOpError::FromContext)?
            .into_iter()
            .map(|result| {
                let mut range: Range = current_span.into();
                range.start.character += result.insert_text_offset as u32;

                CompletionItem {
                    label: result.value.clone(),
                    kind: Some(result.kind),
                    insert_text: result.insert_text.clone(),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: result.insert_text.unwrap_or(result.value),
                    })),
                    ..Default::default()
                }
            })
            .collect())
    }

    pub(crate) fn type_completion_options() -> impl Iterator<Item = CompletionItem> {
        ["str", "int", "bool", "None", "float"]
            .into_iter()
            .map(|type_| CompletionItem {
                label: type_.to_owned(),
                kind: Some(CompletionItemKind::TYPE_PARAMETER),
                ..Default::default()
            })
    }
}
