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

//! Based on the reference lsp-server example at <https://github.com/rust-analyzer/lsp-server/blob/master/examples/goto_def.rs>.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::RwLock;

use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use dupe::OptionDupedExt;
use itertools::Itertools;
use lsp_server::Connection;
use lsp_server::Message;
use lsp_server::Notification;
use lsp_server::ProtocolError;
use lsp_server::Request;
use lsp_server::RequestId;
use lsp_server::Response;
use lsp_server::ResponseError;
use lsp_types::CompletionItem;
use lsp_types::CompletionItemKind;
use lsp_types::CompletionOptions;
use lsp_types::CompletionParams;
use lsp_types::CompletionResponse;
use lsp_types::DefinitionOptions;
use lsp_types::Diagnostic;
use lsp_types::DidChangeTextDocumentParams;
use lsp_types::DidCloseTextDocumentParams;
use lsp_types::DidOpenTextDocumentParams;
use lsp_types::Documentation;
use lsp_types::GotoDefinitionParams;
use lsp_types::GotoDefinitionResponse;
use lsp_types::Hover;
use lsp_types::HoverContents;
use lsp_types::HoverParams;
use lsp_types::HoverProviderCapability;
use lsp_types::InitializeParams;
use lsp_types::LanguageString;
use lsp_types::LocationLink;
use lsp_types::LogMessageParams;
use lsp_types::MarkedString;
use lsp_types::MarkupContent;
use lsp_types::MarkupKind;
use lsp_types::MessageType;
use lsp_types::OneOf;
use lsp_types::Position;
use lsp_types::PublishDiagnosticsParams;
use lsp_types::Range;
use lsp_types::ServerCapabilities;
use lsp_types::TextDocumentSyncCapability;
use lsp_types::TextDocumentSyncKind;
use lsp_types::TextEdit;
use lsp_types::Uri;
use lsp_types::WorkDoneProgressOptions;
use lsp_types::WorkspaceFolder;
use lsp_types::notification::DidChangeTextDocument;
use lsp_types::notification::DidCloseTextDocument;
use lsp_types::notification::DidOpenTextDocument;
use lsp_types::notification::LogMessage;
use lsp_types::notification::PublishDiagnostics;
use lsp_types::request::Completion;
use lsp_types::request::GotoDefinition;
use lsp_types::request::HoverRequest;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::DeserializeOwned;
use starlark::codemap::ResolvedSpan;
use starlark::codemap::Span;
use starlark::docs::DocItem;
use starlark::docs::DocMember;
use starlark::docs::DocModule;
use starlark::docs::markdown::render_doc_item_no_link;
use starlark::docs::markdown::render_doc_param;
use starlark::syntax::AstModule;
use starlark_syntax::codemap::ResolvedPos;
use starlark_syntax::syntax::ast::AstPayload;
use starlark_syntax::syntax::ast::LoadArgP;
use starlark_syntax::syntax::module::AstModuleFields;
use tower_lsp_server::UriExt as _;

use crate::completion::StringCompletionResult;
use crate::completion::StringCompletionType;
use crate::definition::Definition;
use crate::definition::DottedDefinition;
use crate::definition::IdentifierDefinition;
use crate::definition::LspModule;
use crate::inspect::AstModuleInspect;
use crate::inspect::AutocompleteType;
use crate::symbols::find_symbols_at_location;

/// The request to get the file contents for a starlark: URI
struct StarlarkFileContentsRequest {}

impl lsp_types::request::Request for StarlarkFileContentsRequest {
    type Params = StarlarkFileContentsParams;
    type Result = StarlarkFileContentsResponse;
    const METHOD: &'static str = "starlark/fileContents";
}

/// Params to get the file contents for a starlark: URI.
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")] // camelCase to match idioms in LSP spec / typescript land.
struct StarlarkFileContentsParams {
    uri: LspUri,
}

/// The contents of a starlark: URI if available.
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")] // camelCase to match idioms in LSP spec / typescript land.
struct StarlarkFileContentsResponse {
    contents: Option<String>,
}

/// Errors that can happen when converting LspUri and Uri to/from each other.
#[derive(thiserror::Error, Debug)]
pub enum LspUriError {
    /// The path component of the URI was not absolute. This is required for all supported
    /// URI types.
    #[error("`{}` does not have an absolute path component", **.0)]
    NotAbsolute(Uri),
    /// For some reason the PathBuf/Uri in the LspUri could not be converted back to a URI.
    #[error("`{}` could not be converted back to a URI", .0)]
    Unparsable(LspUri),
    #[error("invalid URI for file:// schema (possibly not absolute?): `{}`", **.0)]
    InvalidFileUri(Uri),
}

/// A URI that represents the two types (plus an "Other") of URIs that are supported.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Display)]
pub enum LspUri {
    /// A "file://" URI with a path sent from the LSP client.
    #[display("file://{}", _0.display())]
    File(PathBuf),
    /// A "starlark:" URI. This is mostly used for native types that don't actually
    /// exist on the filesystem. The path component always has a leading slash.
    #[display("starlark:{}", _0.display())]
    Starlark(PathBuf),
    /// Any other type. Often should just be ignored, or return an error.
    #[display("{}", **_0)]
    Other(Uri),
}

impl Serialize for LspUri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match Uri::try_from(self) {
            Ok(uri) => uri.serialize(serializer),
            Err(e) => Err(serde::ser::Error::custom(e.to_string())),
        }
    }
}

impl<'de> Deserialize<'de> for LspUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let uri = Uri::deserialize(deserializer)?;
        LspUri::try_from(uri).map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

impl LspUri {
    /// Returns the path component of the underlying URI
    pub fn path(&self) -> &Path {
        match self {
            LspUri::File(p) => p.as_path(),
            LspUri::Starlark(p) => p.as_path(),
            LspUri::Other(u) => Path::new(u.path().as_str()),
        }
    }
}

impl TryFrom<Uri> for LspUri {
    type Error = LspUriError;

    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        match uri.scheme().map(|scheme| scheme.as_str()) {
            Some("file") => {
                let file_path = uri
                    .to_file_path()
                    .ok_or_else(|| LspUriError::InvalidFileUri(uri.clone()))?;
                if file_path.is_absolute() {
                    Ok(Self::File(file_path.into_owned()))
                } else {
                    Err(LspUriError::NotAbsolute(uri))
                }
            }
            Some("starlark") => {
                // Need to perform the replace to standardize on / instead of \ on windows.
                let path = PathBuf::from(uri.path().as_str().replace('\\', "/"));
                // Use "starts with a /" because, while leading slashes are accepted on
                // windows, they do not report "true" from `is_absolute()`.
                if path.to_string_lossy().starts_with('/') {
                    Ok(Self::Starlark(path))
                } else {
                    Err(LspUriError::NotAbsolute(uri))
                }
            }
            _ => Ok(Self::Other(uri)),
        }
    }
}

impl TryFrom<LspUri> for Uri {
    type Error = LspUriError;

    fn try_from(uri: LspUri) -> Result<Self, Self::Error> {
        Uri::try_from(&uri)
    }
}

impl TryFrom<&LspUri> for Uri {
    type Error = LspUriError;

    fn try_from(uri: &LspUri) -> Result<Self, Self::Error> {
        match &uri {
            LspUri::File(p) => {
                Uri::from_file_path(p).ok_or_else(|| LspUriError::Unparsable(uri.clone()))
            }
            LspUri::Starlark(p) => Uri::from_str(&format!("starlark:{}", p.display()))
                .map_err(|_| LspUriError::Unparsable(uri.clone())),
            LspUri::Other(u) => Ok(u.clone()),
        }
    }
}

/// The result of resolving a StringLiteral when looking up a definition.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct StringLiteralResult {
    /// The path that a string literal resolves to.
    pub uri: LspUri,
    /// A function that takes the AstModule at path specified by `uri`, and
    /// allows resolving a location to jump to within the specific URI if desired.
    ///
    /// If `None`, then just jump to the URI. Do not attempt to load the file.
    #[derivative(Debug = "ignore")]
    pub location_finder: Option<Box<dyn FnOnce(&AstModule) -> Result<Option<Span>, String> + Send>>,
}

fn _assert_string_literal_result_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<StringLiteralResult>();
}

/// The result of evaluating a starlark program for use in the LSP.
#[derive(Default)]
pub struct LspEvalResult {
    /// The list of diagnostic issues that were encountered while evaluating a starlark program.
    pub diagnostics: Vec<Diagnostic>,
    /// If the program could be parsed, the parsed module.
    pub ast: Option<AstModule>,
}

/// Settings that the LspContext can provide to change what capabilities the server enables
/// or disables.
#[derive(Dupe, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LspServerSettings {
    /// Whether goto definition should work.
    pub enable_goto_definition: bool,
}

impl Default for LspServerSettings {
    fn default() -> Self {
        Self {
            enable_goto_definition: true,
        }
    }
}

/// An error encountered during one of the LSP operations
///
/// Consumed at the top level LSP loop and turned into an error that'll be shown to the user in an
/// IDE.
#[derive(derive_more::From)]
pub(crate) enum LspOpError {
    #[from(skip)]
    FromContext(String),
    Protocol(ProtocolError),
    Uri(LspUriError),
    Other(String),
}

impl LspOpError {
    fn format(self) -> String {
        match self {
            LspOpError::FromContext(e) => e,
            LspOpError::Protocol(e) => format!("{:#}", e),
            LspOpError::Uri(e) => format!("{:#}", e),
            LspOpError::Other(e) => format!("{:#}", e),
        }
    }
}

/// Various pieces of context to allow the LSP to interact with starlark parsers, etc.
pub trait LspContext {
    /// Parse a file with the given contents. The filename is used in the diagnostics.
    fn parse_file_with_contents(&self, uri: &LspUri, content: String) -> LspEvalResult;

    /// Resolve a path given in a `load()` statement.
    ///
    /// `path` is the string representation in the `load()` statement. Its meaning is
    ///        implementation defined.
    /// `current_file` is the file that is including the `load()` statement, and should be used
    ///                if `path` is "relative" in a semantic sense.
    fn resolve_load(
        &self,
        path: &str,
        current_file: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<LspUri, String>;

    /// Render the target URI to use as a path in a `load()` statement. If `target` is
    /// in the same package as `current_file`, the result is a relative path.
    ///
    /// `target` is the file that should be loaded by `load()`.
    /// `current_file` is the file that the `load()` statement will be inserted into.
    fn render_as_load(
        &self,
        target: &LspUri,
        current_file: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<String, String>;

    /// Resolve a string literal into a URI and a function that specifies a location within that
    /// target file.
    ///
    /// This can be used for things like file paths in string literals, build targets, etc.
    ///
    /// `current_file` is the file that is currently being evaluated
    fn resolve_string_literal(
        &self,
        literal: &str,
        current_file: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<Option<StringLiteralResult>, String>;

    /// Get the contents of a starlark program at a given path, if it exists.
    fn get_load_contents(&self, uri: &LspUri) -> Result<Option<String>, String>;

    /// Get the contents of a file at a given URI, and attempt to parse it.
    fn parse_file(&self, uri: &LspUri) -> Result<Option<LspEvalResult>, String> {
        let result = self
            .get_load_contents(uri)?
            .map(|content| self.parse_file_with_contents(uri, content));
        Ok(result)
    }

    /// Get the preloaded environment for a particular file.
    fn get_environment(&self, uri: &LspUri) -> DocModule;

    /// Get the LspUri for a global symbol if possible.
    ///
    /// The current file is provided in case different files have different global symbols
    /// defined.
    fn get_uri_for_global_symbol(
        &self,
        current_file: &LspUri,
        symbol: &str,
    ) -> Result<Option<LspUri>, String>;

    /// Get valid completion options if possible, based on the kind of string
    /// completion expected (e.g. any string literal, versus the path argument in
    /// a load statement).
    fn get_string_completion_options(
        &self,
        document_uri: &LspUri,
        kind: StringCompletionType,
        current_value: &str,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<StringCompletionResult>, String> {
        let _unused = (document_uri, kind, current_value, workspace_root);
        Ok(Vec::new())
    }
}

pub(crate) struct Backend<T: LspContext> {
    connection: Connection,
    pub(crate) context: T,
    /// The `AstModule` from the last time that a file was opened / changed and parsed successfully.
    /// Entries are evicted when the file is closed.
    pub(crate) last_valid_parse: RwLock<HashMap<LspUri, Arc<LspModule>>>,
}

/// The logic implementations of stuff
impl<T: LspContext> Backend<T> {
    fn server_capabilities(settings: LspServerSettings) -> ServerCapabilities {
        let definition_provider = settings.enable_goto_definition.then_some({
            OneOf::Right(DefinitionOptions {
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: None,
                },
            })
        });
        ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            definition_provider,
            completion_provider: Some(CompletionOptions::default()),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            ..ServerCapabilities::default()
        }
    }

    fn get_ast(&self, uri: &LspUri) -> Option<Arc<LspModule>> {
        let last_valid_parse = self.last_valid_parse.read().unwrap();
        last_valid_parse.get(uri).duped()
    }

    pub(crate) fn get_ast_or_load_from_disk(
        &self,
        uri: &LspUri,
    ) -> Result<Option<Arc<LspModule>>, LspOpError> {
        let module = match self.get_ast(uri) {
            Some(result) => Some(result),
            None => self
                .context
                .parse_file(uri)
                .map_err(LspOpError::FromContext)?
                .and_then(|eval_result| eval_result.ast.map(|ast| Arc::new(LspModule::new(ast)))),
        };
        Ok(module)
    }

    fn validate(&self, uri: Uri, version: Option<i64>, text: String) -> Result<(), LspOpError> {
        let lsp_uri = uri.clone().try_into()?;
        let eval_result = self.context.parse_file_with_contents(&lsp_uri, text);
        if let Some(ast) = eval_result.ast {
            let module = Arc::new(LspModule::new(ast));
            let mut last_valid_parse = self.last_valid_parse.write().unwrap();
            last_valid_parse.insert(lsp_uri, module);
        }
        self.publish_diagnostics(uri, eval_result.diagnostics, version);
        Ok(())
    }

    fn did_open(&self, params: DidOpenTextDocumentParams) -> Result<(), LspOpError> {
        self.validate(
            params.text_document.uri,
            Some(params.text_document.version as i64),
            params.text_document.text,
        )
    }

    fn did_change(&self, params: DidChangeTextDocumentParams) -> Result<(), LspOpError> {
        // We asked for Sync full, so can just grab all the text from params
        let change = params.content_changes.into_iter().next().unwrap();
        self.validate(
            params.text_document.uri,
            Some(params.text_document.version as i64),
            change.text,
        )
    }

    fn did_close(&self, params: DidCloseTextDocumentParams) -> Result<(), LspOpError> {
        {
            let mut last_valid_parse = self.last_valid_parse.write().unwrap();
            last_valid_parse.remove(&params.text_document.uri.clone().try_into()?);
        }
        self.publish_diagnostics(params.text_document.uri, Vec::new(), None);
        Ok(())
    }

    /// Go to the definition of the symbol at the current cursor if that definition is in
    /// the same file.
    ///
    /// NOTE: This uses the last valid parse of a file as a basis for symbol locations.
    /// If a file has changed and does result in a valid parse, then symbol locations may
    /// be slightly incorrect.
    fn goto_definition(
        &self,
        id: RequestId,
        params: GotoDefinitionParams,
        initialize_params: &InitializeParams,
    ) {
        self.send_response(new_response(
            id,
            self.find_definition(params, initialize_params),
        ));
    }

    /// Offers completion of known symbols in the current file.
    fn completion(
        &self,
        id: RequestId,
        params: CompletionParams,
        initialize_params: &InitializeParams,
    ) {
        self.send_response(new_response(
            id,
            self.completion_options(params, initialize_params),
        ));
    }

    /// Offers hover information for the symbol at the current cursor.
    fn hover(&self, id: RequestId, params: HoverParams, initialize_params: &InitializeParams) {
        self.send_response(new_response(id, self.hover_info(params, initialize_params)));
    }

    /// Get the file contents of a starlark: URI.
    fn get_starlark_file_contents(&self, id: RequestId, params: StarlarkFileContentsParams) {
        let response = match params.uri {
            LspUri::Starlark(_) => self
                .context
                .get_load_contents(&params.uri)
                .map(|contents| StarlarkFileContentsResponse { contents })
                .map_err(LspOpError::FromContext),
            _ => Err(LspOpError::Other(format!(
                "URI `{}` was expected to be of type starlark",
                params.uri
            ))),
        };
        self.send_response(new_response(id, response));
    }

    pub(crate) fn resolve_load_path(
        &self,
        path: &str,
        current_uri: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<LspUri, LspOpError> {
        match current_uri {
            LspUri::File(_) => self
                .context
                .resolve_load(path, current_uri, workspace_root)
                .map_err(LspOpError::FromContext),
            LspUri::Starlark(_) | LspUri::Other(_) => Err(LspOpError::Other(format!(
                "URI `{}` was expected to be of type file",
                current_uri
            ))),
        }
    }

    /// Simple helper to generate `Some(LocationLink)` objects in `resolve_definition_location`
    fn location_link<R: Into<Range> + Copy>(
        source: ResolvedSpan,
        uri: &LspUri,
        target_range: R,
    ) -> Result<Option<LocationLink>, LspOpError> {
        Ok(Some(LocationLink {
            origin_selection_range: Some(source.into()),
            target_uri: uri.try_into()?,
            target_range: target_range.into(),
            target_selection_range: target_range.into(),
        }))
    }

    /// Find the ultimate places that an identifier is defined.
    ///
    /// Takes a definition location and if necessary loads other files trying
    /// to find where the symbol was defined in a useful way. e.g. pointing to the
    /// symbol in a "load()" statement isn't useful, but going to the file it is
    /// loaded from and pointing at a function definition very much is.
    fn resolve_definition_location(
        &self,
        definition: IdentifierDefinition,
        source: ResolvedSpan,
        member: Option<&str>,
        uri: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<Option<LocationLink>, LspOpError> {
        let ret = match definition {
            IdentifierDefinition::Location {
                destination: target,
                ..
            } => Self::location_link(source, uri, target)?,
            IdentifierDefinition::LoadedLocation {
                destination: location,
                path,
                name,
                ..
            } => {
                let load_uri = self.resolve_load_path(&path, uri, workspace_root)?;
                let loaded_location =
                    self.get_ast_or_load_from_disk(&load_uri)?
                        .and_then(|ast| match member {
                            Some(member) => ast.find_exported_symbol_and_member(&name, member),
                            None => ast.find_exported_symbol_span(&name),
                        });
                match loaded_location {
                    None => Self::location_link(source, uri, location)?,
                    Some(loaded_location) => {
                        Self::location_link(source, &load_uri, loaded_location)?
                    }
                }
            }
            IdentifierDefinition::NotFound => None,
            IdentifierDefinition::LoadPath { path, .. } => {
                match self.resolve_load_path(&path, uri, workspace_root) {
                    Ok(load_uri) => Self::location_link(source, &load_uri, Range::default())?,
                    Err(_) => None,
                }
            }
            IdentifierDefinition::StringLiteral { literal, .. } => {
                let Ok(resolved_literal) =
                    self.context
                        .resolve_string_literal(&literal, uri, workspace_root)
                else {
                    return Ok(None);
                };
                match resolved_literal {
                    Some(StringLiteralResult {
                        uri,
                        location_finder: Some(location_finder),
                    }) => {
                        // If there's an error loading the file to parse it, at least
                        // try to get to the file.
                        let result =
                            self.get_ast_or_load_from_disk(&uri)
                                .and_then(|ast| match ast {
                                    Some(module) => location_finder(&module.ast)
                                        .map(|span| {
                                            span.map(|span| module.ast.codemap().resolve_span(span))
                                        })
                                        .map_err(LspOpError::FromContext),
                                    None => Ok(None),
                                });
                        #[allow(clippy::manual_unwrap_or_default)]
                        let result = match result {
                            Ok(result) => result,
                            Err(_e) => {
                                // FIXME(JakobDegen): wtf is happening here?
                                // eprintln!("Error jumping to definition: {e:#}");
                                None
                            }
                        };
                        let target_range = result.unwrap_or_default();
                        Self::location_link(source, &uri, target_range)?
                    }
                    Some(StringLiteralResult {
                        uri,
                        location_finder: None,
                    }) => Self::location_link(source, &uri, Range::default())?,
                    _ => None,
                }
            }
            IdentifierDefinition::Unresolved { name, .. } => {
                match self
                    .context
                    .get_uri_for_global_symbol(uri, &name)
                    .map_err(LspOpError::FromContext)?
                {
                    Some(uri) => {
                        let loaded_location =
                            self.get_ast_or_load_from_disk(&uri)?
                                .and_then(|ast| match member {
                                    Some(member) => {
                                        ast.find_exported_symbol_and_member(&name, member)
                                    }
                                    None => ast.find_exported_symbol_span(&name),
                                });

                        Self::location_link(source, &uri, loaded_location.unwrap_or_default())?
                    }
                    None => None,
                }
            }
        };
        Ok(ret)
    }

    fn find_definition(
        &self,
        params: GotoDefinitionParams,
        initialize_params: &InitializeParams,
    ) -> Result<GotoDefinitionResponse, LspOpError> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .try_into()?;
        let line = params.text_document_position_params.position.line;
        let character = params.text_document_position_params.position.character;
        let workspace_root =
            Self::get_workspace_root(initialize_params.workspace_folders.as_ref(), &uri);

        let location = match self.get_ast(&uri) {
            Some(ast) => {
                let location = ast.find_definition_at_location(line, character);
                let source = location.source().unwrap_or_default();
                match location {
                    Definition::Identifier(definition) => self.resolve_definition_location(
                        definition,
                        source,
                        None,
                        &uri,
                        workspace_root.as_deref(),
                    )?,
                    // In this case we don't pass the name along in the root_definition_location,
                    // so it's simpler to do the lookup here, rather than threading a ton of
                    // information through.
                    Definition::Dotted(DottedDefinition {
                        root_definition_location: IdentifierDefinition::Location { destination, .. },
                        segments,
                        ..
                    }) => {
                        let member_location = ast
                            .find_exported_symbol_and_member(
                                segments.first().expect("at least one segment").as_str(),
                                segments.get(1).expect("at least two segments").as_str(),
                            )
                            .unwrap_or(destination);
                        Self::location_link(source, &uri, member_location)?
                    }
                    Definition::Dotted(definition) => self.resolve_definition_location(
                        definition.root_definition_location,
                        source,
                        Some(
                            definition
                                .segments
                                .last()
                                .expect("to have at least one component")
                                .as_str(),
                        ),
                        &uri,
                        workspace_root.as_deref(),
                    )?,
                }
            }
            None => None,
        };

        let response = match location {
            Some(location) => vec![location],
            None => vec![],
        };
        Ok(GotoDefinitionResponse::Link(response))
    }

    fn completion_options(
        &self,
        params: CompletionParams,
        initialize_params: &InitializeParams,
    ) -> Result<CompletionResponse, LspOpError> {
        let uri = params.text_document_position.text_document.uri.try_into()?;
        let line = params.text_document_position.position.line;
        let character = params.text_document_position.position.character;

        let symbols: Option<Vec<_>> = match self.get_ast(&uri) {
            Some(document) => {
                // Figure out what kind of position we are in, to determine the best type of
                // autocomplete.
                let autocomplete_type = document.ast.get_auto_complete_type(line, character);
                let workspace_root =
                    Self::get_workspace_root(initialize_params.workspace_folders.as_ref(), &uri);

                match &autocomplete_type {
                    None | Some(AutocompleteType::None) => None,
                    Some(AutocompleteType::Default) => Some(
                        self.default_completion_options(
                            &uri,
                            &document,
                            line,
                            character,
                            workspace_root.as_deref(),
                        )
                        .collect(),
                    ),
                    Some(AutocompleteType::LoadPath {
                        current_value,
                        current_span,
                    })
                    | Some(AutocompleteType::String {
                        current_value,
                        current_span,
                    }) => Some(self.string_completion_options(
                        &uri,
                        if matches!(&autocomplete_type, Some(AutocompleteType::LoadPath { .. })) {
                            StringCompletionType::LoadPath
                        } else {
                            StringCompletionType::String
                        },
                        current_value,
                        *current_span,
                        workspace_root.as_deref(),
                    )?),
                    Some(AutocompleteType::LoadSymbol {
                        path,
                        current_span,
                        previously_loaded,
                    }) => Some(self.exported_symbol_options(
                        path,
                        *current_span,
                        previously_loaded,
                        &uri,
                        workspace_root.as_deref(),
                    )),
                    Some(AutocompleteType::Parameter {
                        function_name_span,
                        previously_used_named_parameters,
                        ..
                    }) => Some(
                        self.parameter_name_options(
                            function_name_span,
                            &document,
                            &uri,
                            previously_used_named_parameters,
                            workspace_root.as_deref(),
                        )
                        .chain(self.default_completion_options(
                            &uri,
                            &document,
                            line,
                            character,
                            workspace_root.as_deref(),
                        ))
                        .collect(),
                    ),
                    Some(AutocompleteType::Type) => Some(Self::type_completion_options().collect()),
                }
            }
            None => None,
        };

        Ok(CompletionResponse::Array(symbols.unwrap_or_default()))
    }

    /// Using all currently loaded documents, gather a list of known exported
    /// symbols. This list contains both the symbols exported from the loaded
    /// files, as well as symbols loaded in the open files. Symbols that are
    /// loaded from modules that are open are deduplicated.
    pub(crate) fn get_all_exported_symbols<F, S>(
        &self,
        except_from: Option<&LspUri>,
        symbols: &HashMap<String, S>,
        workspace_root: Option<&Path>,
        current_document: &LspUri,
        format_text_edit: F,
    ) -> Vec<CompletionItem>
    where
        F: Fn(&str, &str) -> TextEdit,
    {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        let all_documents = self.last_valid_parse.read().unwrap();

        for (doc_uri, doc) in all_documents
            .iter()
            .filter(|&(doc_uri, _)| match except_from {
                Some(uri) => doc_uri != uri,
                None => true,
            })
        {
            let Ok(load_path) =
                self.context
                    .render_as_load(doc_uri, current_document, workspace_root)
            else {
                continue;
            };

            for symbol in doc
                .get_exported_symbols()
                .into_iter()
                .filter(|symbol| !symbols.contains_key(&symbol.name))
            {
                seen.insert(format!("{load_path}:{}", symbol.name));

                let text_edits = Some(vec![format_text_edit(&load_path, &symbol.name)]);
                let mut completion_item: CompletionItem = symbol.into();
                completion_item.detail = Some(format!("Load from {load_path}"));
                completion_item.additional_text_edits = text_edits;

                result.push(completion_item)
            }
        }

        for (doc_uri, symbol) in all_documents
            .iter()
            .filter(|&(doc_uri, _)| match except_from {
                Some(uri) => doc_uri != uri,
                None => true,
            })
            .flat_map(|(doc_uri, doc)| {
                doc.get_loaded_symbols()
                    .into_iter()
                    .map(move |symbol| (doc_uri, symbol))
            })
            .filter(|(_, symbol)| !symbols.contains_key(symbol.name))
        {
            let Ok(uri) = self
                .context
                .resolve_load(symbol.loaded_from, doc_uri, workspace_root)
            else {
                continue;
            };
            let Ok(load_path) = self
                .context
                .render_as_load(&uri, current_document, workspace_root)
            else {
                continue;
            };

            if seen.insert(format!("{}:{}", load_path, symbol.name)) {
                result.push(CompletionItem {
                    label: symbol.name.to_owned(),
                    detail: Some(format!("Load from {}", load_path)),
                    kind: Some(CompletionItemKind::CONSTANT),
                    additional_text_edits: Some(vec![format_text_edit(&load_path, symbol.name)]),
                    ..Default::default()
                })
            }
        }

        result
    }

    pub(crate) fn get_global_symbol_completion_items(
        &self,
        current_document: &LspUri,
    ) -> impl Iterator<Item = CompletionItem> + '_ + use<'_, T> {
        self.context
            .get_environment(current_document)
            .members
            .into_iter()
            .map(|(symbol, documentation)| CompletionItem {
                label: symbol.clone(),
                kind: Some(match &documentation {
                    DocItem::Member(DocMember::Function { .. }) => CompletionItemKind::FUNCTION,
                    _ => CompletionItemKind::CONSTANT,
                }),
                detail: documentation.get_doc_summary().map(|str| str.to_owned()),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: render_doc_item_no_link(&symbol, &documentation),
                })),
                ..Default::default()
            })
    }

    pub(crate) fn get_load_text_edit<P>(
        module: &str,
        symbol: &str,
        ast: &LspModule,
        last_load: Option<ResolvedSpan>,
        existing_load: Option<&(Vec<LoadArgP<P>>, Span)>,
    ) -> TextEdit
    where
        P: AstPayload,
    {
        match existing_load {
            Some((previously_loaded_symbols, load_span)) => {
                // We're already loading a symbol from this module path, construct
                // a text edit that amends the existing load.
                let load_span = ast.ast.codemap().resolve_span(*load_span);
                let mut load_args: Vec<(&str, &str)> = previously_loaded_symbols
                    .iter()
                    .map(|LoadArgP { local, their, .. }| {
                        (local.ident.as_str(), their.node.as_str())
                    })
                    .collect();
                load_args.push((symbol, symbol));
                load_args.sort_by_key(|(_, a)| *a);

                TextEdit::new(
                    load_span.into(),
                    format!(
                        "load(\"{module}\", {})",
                        load_args
                            .into_iter()
                            .map(|(assign, import)| {
                                if assign == import {
                                    format!("\"{import}\"")
                                } else {
                                    format!("{assign} = \"{import}\"")
                                }
                            })
                            .join(", ")
                    ),
                )
            }
            None => {
                // We're not yet loading from this module, construct a text edit that
                // inserts a new load statement after the last one we found.
                TextEdit::new(
                    match last_load {
                        Some(span) => Range::new(
                            Position::new(span.end.line as u32, span.end.column as u32),
                            Position::new(span.end.line as u32, span.end.column as u32),
                        ),
                        None => Range::new(Position::new(0, 0), Position::new(0, 0)),
                    },
                    format!(
                        "{}load(\"{module}\", \"{symbol}\"){}",
                        if last_load.is_some() { "\n" } else { "" },
                        if last_load.is_some() { "" } else { "\n\n" },
                    ),
                )
            }
        }
    }

    /// Get completion items for each language keyword.
    pub(crate) fn get_keyword_completion_items() -> impl Iterator<Item = CompletionItem> {
        [
            // Actual keywords
            "and", "else", "load", "break", "for", "not", "continue", "if", "or", "def", "in",
            "pass", "elif", "return", "lambda",
        ]
        .into_iter()
        .map(|keyword| CompletionItem {
            label: keyword.to_owned(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
    }

    /// Get hover information for a given position in a document.
    fn hover_info(
        &self,
        params: HoverParams,
        initialize_params: &InitializeParams,
    ) -> Result<Hover, LspOpError> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .try_into()?;
        let line = params.text_document_position_params.position.line;
        let character = params.text_document_position_params.position.character;
        let workspace_root =
            Self::get_workspace_root(initialize_params.workspace_folders.as_ref(), &uri);

        // Return an empty result as a "not found"
        let not_found = Hover {
            contents: HoverContents::Array(vec![]),
            range: None,
        };

        Ok(match self.get_ast(&uri) {
            Some(document) => {
                let location = document.find_definition_at_location(line, character);
                match location {
                    Definition::Identifier(identifier_definition) => self
                        .get_hover_for_identifier_definition(
                            identifier_definition,
                            &document,
                            &uri,
                            workspace_root.as_deref(),
                        )?,
                    Definition::Dotted(DottedDefinition {
                        root_definition_location,
                        ..
                    }) => {
                        // Not something we really support yet, so just provide hover information for
                        // the root definition.
                        self.get_hover_for_identifier_definition(
                            root_definition_location,
                            &document,
                            &uri,
                            workspace_root.as_deref(),
                        )?
                    }
                }
                .unwrap_or(not_found)
            }
            None => not_found,
        })
    }

    fn get_hover_for_identifier_definition(
        &self,
        identifier_definition: IdentifierDefinition,
        document: &LspModule,
        document_uri: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<Option<Hover>, LspOpError> {
        Ok(match identifier_definition {
            IdentifierDefinition::Location {
                destination,
                name,
                source,
            } => {
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
                .remove(&name)
                .and_then(|symbol| {
                    symbol
                        .doc
                        .map(|docs| Hover {
                            contents: HoverContents::Array(vec![MarkedString::String(
                                render_doc_item_no_link(&symbol.name, &docs),
                            )]),
                            range: Some(source.into()),
                        })
                        .or_else(|| {
                            symbol.param.map(|(starred_name, doc)| Hover {
                                contents: HoverContents::Array(vec![MarkedString::String(
                                    render_doc_param(starred_name, &doc),
                                )]),
                                range: Some(source.into()),
                            })
                        })
                })
            }
            IdentifierDefinition::LoadedLocation {
                path, name, source, ..
            } => {
                // Symbol loaded from another file. Find the file and get the definition
                // from there, hopefully including the docs.
                let load_uri = self.resolve_load_path(&path, document_uri, workspace_root)?;
                self.get_ast_or_load_from_disk(&load_uri)?.and_then(|ast| {
                    ast.find_exported_symbol(&name).and_then(|symbol| {
                        symbol.docs.map(|docs| Hover {
                            contents: HoverContents::Array(vec![MarkedString::String(
                                render_doc_item_no_link(&symbol.name, &docs),
                            )]),
                            range: Some(source.into()),
                        })
                    })
                })
            }
            IdentifierDefinition::StringLiteral { source, literal } => {
                let Ok(resolved_literal) =
                    self.context
                        .resolve_string_literal(&literal, document_uri, workspace_root)
                else {
                    // We might just be hovering a string that's not a file/target/etc,
                    // so just return nothing.
                    return Ok(None);
                };
                match resolved_literal {
                    Some(StringLiteralResult {
                        uri,
                        location_finder: Some(location_finder),
                    }) => {
                        // If there's an error loading the file to parse it, at least
                        // try to get to the file.
                        let module = match self.get_ast_or_load_from_disk(&uri) {
                            Ok(Some(ast)) => ast,
                            _ => {
                                return Ok(None);
                            }
                        };
                        let result =
                            location_finder(&module.ast).map_err(LspOpError::FromContext)?;

                        result.map(|location| Hover {
                            contents: HoverContents::Array(vec![MarkedString::LanguageString(
                                LanguageString {
                                    language: "python".to_owned(),
                                    value: module.ast.codemap().source_span(location).to_owned(),
                                },
                            )]),
                            range: Some(source.into()),
                        })
                    }
                    _ => None,
                }
            }
            IdentifierDefinition::Unresolved { source, name } => {
                // Try to resolve as a global symbol.
                self.context
                    .get_environment(document_uri)
                    .members
                    .into_iter()
                    .find(|symbol| symbol.0 == name)
                    .map(|symbol| Hover {
                        contents: HoverContents::Array(vec![MarkedString::String(
                            render_doc_item_no_link(&symbol.0, &symbol.1),
                        )]),
                        range: Some(source.into()),
                    })
            }
            IdentifierDefinition::LoadPath { .. } | IdentifierDefinition::NotFound => None,
        })
    }

    fn get_workspace_root(
        workspace_roots: Option<&Vec<WorkspaceFolder>>,
        target: &LspUri,
    ) -> Option<PathBuf> {
        match target {
            LspUri::File(target) => workspace_roots.and_then(|roots| {
                roots
                    .iter()
                    .filter_map(|root| root.uri.to_file_path())
                    .find(|root| target.starts_with(root))
                    .map(Cow::into_owned)
            }),
            _ => None,
        }
    }
}

/// The library style pieces
impl<T: LspContext> Backend<T> {
    fn send_notification(&self, x: Notification) {
        self.connection
            .sender
            .send(Message::Notification(x))
            .unwrap()
    }

    fn send_response(&self, x: Response) {
        self.connection.sender.send(Message::Response(x)).unwrap()
    }

    fn maybe_log_error(&self, res: Result<(), LspOpError>) {
        if let Err(e) = res {
            self.log_message(MessageType::ERROR, &e.format());
        }
    }

    fn log_message(&self, typ: MessageType, message: &str) {
        self.send_notification(new_notification::<LogMessage>(LogMessageParams {
            typ,
            message: message.to_owned(),
        }))
    }

    fn publish_diagnostics(&self, uri: Uri, diags: Vec<Diagnostic>, version: Option<i64>) {
        self.send_notification(new_notification::<PublishDiagnostics>(
            PublishDiagnosticsParams::new(uri, diags, version.map(|i| i as i32)),
        ));
    }

    fn main_loop(&self, initialize_params: InitializeParams) -> Result<(), ProtocolError> {
        self.log_message(MessageType::INFO, "Starlark server initialised");
        for msg in &self.connection.receiver {
            match msg {
                Message::Request(req) => {
                    // TODO(nmj): Also implement DocumentSymbols so that some logic can
                    //            be handled client side.
                    if let Some(params) = as_request::<GotoDefinition>(&req) {
                        self.goto_definition(req.id, params, &initialize_params);
                    } else if let Some(params) = as_request::<StarlarkFileContentsRequest>(&req) {
                        self.get_starlark_file_contents(req.id, params);
                    } else if let Some(params) = as_request::<Completion>(&req) {
                        self.completion(req.id, params, &initialize_params);
                    } else if let Some(params) = as_request::<HoverRequest>(&req) {
                        self.hover(req.id, params, &initialize_params);
                    } else if self.connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    // Currently don't handle any other requests
                }
                Message::Notification(x) => {
                    if let Some(params) = as_notification::<DidOpenTextDocument>(&x) {
                        self.maybe_log_error(self.did_open(params));
                    } else if let Some(params) = as_notification::<DidChangeTextDocument>(&x) {
                        self.maybe_log_error(self.did_change(params));
                    } else if let Some(params) = as_notification::<DidCloseTextDocument>(&x) {
                        self.maybe_log_error(self.did_close(params));
                    }
                }
                Message::Response(_) => {
                    // Don't expect any of these
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LspServerError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    BadInitializeParams(serde_json::Error),
    #[error(transparent)]
    Stdio(std::io::Error),
}

/// Instantiate an LSP server that reads on stdin, and writes to stdout
pub fn stdio_server<T: LspContext>(context: T) -> Result<(), LspServerError> {
    // Note that  we must have our logging only write out to stderr.
    eprintln!("Starting Rust Starlark server");

    let (connection, io_threads) = Connection::stdio();
    server_with_connection(connection, context)?;
    // Make sure that the io threads stop properly too.
    io_threads.join().map_err(LspServerError::Stdio)?;

    eprintln!("Stopping Rust Starlark server");
    Ok(())
}

/// Instantiate an LSP server that reads and writes using the given connection.
pub fn server_with_connection<T: LspContext>(
    connection: Connection,
    context: T,
) -> Result<(), LspServerError> {
    // Run the server and wait for the main thread to end (typically by trigger LSP Exit event).
    let (init_request_id, init_value) = connection.initialize_start()?;

    let initialization_params: InitializeParams =
        serde_json::from_value(init_value).map_err(LspServerError::BadInitializeParams)?;
    let server_settings = initialization_params
        .initialization_options
        .as_ref()
        .and_then(|opts| serde_json::from_value(opts.clone()).ok())
        .unwrap_or_default();
    let capabilities_payload = Backend::<T>::server_capabilities(server_settings);
    let server_capabilities = serde_json::to_value(capabilities_payload).unwrap();

    let initialize_data = serde_json::json!({
            "capabilities": server_capabilities,
    });
    connection.initialize_finish(init_request_id, initialize_data)?;

    Backend {
        connection,
        context,
        last_valid_parse: RwLock::default(),
    }
    .main_loop(initialization_params)?;

    Ok(())
}

fn as_notification<T>(x: &Notification) -> Option<T::Params>
where
    T: lsp_types::notification::Notification,
    T::Params: DeserializeOwned,
{
    if x.method == T::METHOD {
        let params = serde_json::from_value(x.params.clone()).unwrap_or_else(|err| {
            panic!(
                "Invalid notification\nMethod: {}\n error: {}",
                x.method, err
            )
        });
        Some(params)
    } else {
        None
    }
}

fn as_request<T>(x: &Request) -> Option<T::Params>
where
    T: lsp_types::request::Request,
    T::Params: DeserializeOwned,
{
    if x.method == T::METHOD {
        let params = serde_json::from_value(x.params.clone()).unwrap_or_else(|err| {
            panic!(
                "Invalid request\n  method: {}\n  error: {}\n  request: {:?}\n",
                x.method, err, x
            )
        });
        Some(params)
    } else {
        None
    }
}

/// Create a new `Notification` object with the correct name from the given params.
pub(crate) fn new_notification<T>(params: T::Params) -> Notification
where
    T: lsp_types::notification::Notification,
{
    Notification {
        method: T::METHOD.to_owned(),
        params: serde_json::to_value(&params).unwrap(),
    }
}

fn new_response<T>(id: RequestId, params: Result<T, LspOpError>) -> Response
where
    T: serde::Serialize,
{
    match params {
        Ok(params) => Response {
            id,
            response_result: Ok(serde_json::to_value(params).unwrap()),
        },
        Err(e) => Response {
            id,
            response_result: Err(ResponseError {
                code: 0,
                message: e.format(),
                data: None,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;
    use std::str::FromStr as _;

    use anyhow::Context;
    use lsp_server::Request;
    use lsp_server::RequestId;
    use lsp_types::DidOpenTextDocumentParams;
    use lsp_types::GotoDefinitionParams;
    use lsp_types::GotoDefinitionResponse;
    use lsp_types::LocationLink;
    use lsp_types::Position;
    use lsp_types::Range;
    use lsp_types::TextDocumentIdentifier;
    use lsp_types::TextDocumentItem;
    use lsp_types::TextDocumentPositionParams;
    use lsp_types::Uri;
    use lsp_types::notification::DidOpenTextDocument;
    use lsp_types::notification::PublishDiagnostics;
    use lsp_types::request::GotoDefinition;
    use starlark::codemap::ResolvedSpan;
    use starlark::wasm::is_wasm;
    use textwrap::dedent;
    use tower_lsp_server::UriExt as _;

    use crate::definition::helpers::FixtureWithRanges;
    use crate::server::LspServerSettings;
    use crate::server::LspUri;
    use crate::server::StarlarkFileContentsParams;
    use crate::server::StarlarkFileContentsRequest;
    use crate::server::StarlarkFileContentsResponse;
    use crate::server::new_notification;
    use crate::test::TestServer;

    fn goto_definition_request(
        server: &mut TestServer,
        uri: Uri,
        line: u32,
        character: u32,
    ) -> Request {
        server.new_request::<GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    fn goto_definition_response_location(
        server: &mut TestServer,
        request_id: RequestId,
    ) -> anyhow::Result<LocationLink> {
        let response = server.get_response::<GotoDefinitionResponse>(request_id)?;
        match response {
            GotoDefinitionResponse::Link(locations) if locations.len() == 1 => {
                Ok(locations[0].clone())
            }
            _ => Err(anyhow::anyhow!("Got invalid message type: {:?}", response)),
        }
    }

    fn expected_location_link(
        uri: Uri,
        source_line: u32,
        source_start_col: u32,
        source_end_col: u32,
        dest_line: u32,
        dest_start_col: u32,
        dest_end_col: u32,
    ) -> LocationLink {
        let source_range = Range::new(
            Position::new(source_line, source_start_col),
            Position::new(source_line, source_end_col),
        );
        let dest_range = Range::new(
            Position::new(dest_line, dest_start_col),
            Position::new(dest_line, dest_end_col),
        );
        LocationLink {
            origin_selection_range: Some(source_range),
            target_uri: uri,
            target_range: dest_range,
            target_selection_range: dest_range,
        }
    }

    fn expected_location_link_from_spans(
        uri: Uri,
        source_span: ResolvedSpan,
        dest_span: ResolvedSpan,
    ) -> LocationLink {
        LocationLink {
            origin_selection_range: Some(source_span.into()),
            target_uri: uri,
            target_range: dest_span.into(),
            target_selection_range: dest_span.into(),
        }
    }

    #[cfg(windows)]
    fn temp_file_uri(rel_path: &str) -> Uri {
        Uri::from_file_path(PathBuf::from("C:/tmp").join(rel_path)).unwrap()
    }

    #[cfg(not(windows))]
    fn temp_file_uri(rel_path: &str) -> Uri {
        Uri::from_file_path(PathBuf::from("/tmp").join(rel_path)).unwrap()
    }

    // Converts PathBuf to string that can be used in starlark load statements within "" quotes.
    // Replaces \ with / (for Windows paths).
    fn path_to_load_string(p: &Path) -> String {
        p.to_str().unwrap().replace('\\', "/")
    }

    fn uri_to_load_string(uri: &Uri) -> String {
        path_to_load_string(&uri.to_file_path().unwrap())
    }

    #[test]
    fn sends_empty_goto_definition_on_nonexistent_file() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let mut server = TestServer::new()?;
        let req = goto_definition_request(&mut server, temp_file_uri("nonexistent"), 0, 0);

        let request_id = server.send_request(req)?;
        let response: GotoDefinitionResponse = server.get_response(request_id)?;
        match response {
            GotoDefinitionResponse::Array(definitions) if definitions.is_empty() => Ok(()),
            response => Err(anyhow::anyhow!(
                "Expected empty definitions, got `{:?}`",
                response
            )),
        }
    }

    #[test]
    fn sends_empty_goto_definition_on_non_access_symbol() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let uri = temp_file_uri("file.star");

        let mut server = TestServer::new()?;
        let contents = "y = 1\ndef nothing():\n    pass\nprint(nothing())\n";
        server.open_file(uri.clone(), contents.to_owned())?;

        let goto_definition = goto_definition_request(&mut server, uri, 1, 6);

        let request_id = server.send_request(goto_definition)?;
        let response = server.get_response::<GotoDefinitionResponse>(request_id)?;
        match response {
            GotoDefinitionResponse::Array(definitions) if definitions.is_empty() => Ok(()),
            response => Err(anyhow::anyhow!(
                "Expected empty definitions, got `{:?}`",
                response
            )),
        }
    }

    #[test]
    fn goes_to_definition() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let uri = temp_file_uri("file.star");
        let expected_location = expected_location_link(uri.clone(), 3, 6, 13, 1, 4, 11);

        let mut server = TestServer::new()?;
        let contents = "y = 1\ndef nothing():\n    pass\nprint(nothing())\n";
        server.open_file(uri.clone(), contents.to_owned())?;

        let goto_definition = goto_definition_request(&mut server, uri, 3, 6);

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn returns_old_definitions_if_current_file_does_not_parse() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let uri = temp_file_uri("file.star");
        let expected_location = expected_location_link(uri.clone(), 3, 6, 13, 1, 4, 11);

        let mut server = TestServer::new()?;
        let contents = "y = 1\ndef nothing():\n    pass\nprint(nothing())\n";
        server.open_file(uri.clone(), contents.to_owned())?;
        server.change_file(uri.clone(), "\"invalid parse".to_owned())?;

        let goto_definition = goto_definition_request(&mut server, uri, 3, 6);

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn jumps_to_definition_from_opened_loaded_file() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("{load}", "baz")
            <baz_click><baz>b</baz>az</baz_click>()
            "#,
        )
        .replace("{load}", &uri_to_load_string(&bar_uri))
        .trim()
        .to_owned();
        let bar_contents = "def <baz>baz</baz>():\n    pass";
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;

        let expected_location = expected_location_link_from_spans(
            bar_uri.clone(),
            foo.resolved_span("baz_click"),
            bar.resolved_span("baz"),
        );

        let mut server = TestServer::new()?;
        // Initialize with "junk" on disk so that we make sure we're using the contents from the
        // client (potentially indicating an unsaved, modified file)
        server.set_file_contents(&bar_uri, "some_symbol = 1".to_owned())?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.open_file(bar_uri, bar.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("baz"),
            foo.begin_column("baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn jumps_to_definition_from_closed_loaded_file() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("{load}", "baz")
            <baz_click><baz>b</baz>az</baz_click>()
            "#,
        )
        .replace("{load}", &uri_to_load_string(&bar_uri))
        .trim()
        .to_owned();
        eprintln!("foo_contents: {foo_contents}");
        let bar_contents = "def <baz>baz</baz>():\n    pass";
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;

        let expected_location = expected_location_link_from_spans(
            bar_uri.clone(),
            foo.resolved_span("baz_click"),
            bar.resolved_span("baz"),
        );

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.set_file_contents(&bar_uri, bar.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("baz"),
            foo.begin_column("baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn passes_cwd_for_relative_loads() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("bar.star", "baz")
            <baz_click><baz>b</baz>az</baz_click>()
            "#,
        )
        .trim()
        .to_owned();
        let bar_contents = "def <baz>baz</baz>():\n    pass";
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;

        let expected_location = expected_location_link_from_spans(
            bar_uri.clone(),
            foo.resolved_span("baz_click"),
            bar.resolved_span("baz"),
        );

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.set_file_contents(&bar_uri, bar.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("baz"),
            foo.begin_column("baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn does_not_jump_to_definition_if_invalid_file() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");

        let foo_contents = dedent(
            r#"
            load("{load}", <baz_loc>"baz"</baz_loc>)
            <baz_click><baz>b</baz>az</baz_click>()
            "#,
        )
        .replace("{load}", foo_uri.path().as_str())
        .trim()
        .to_owned();
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let expected_location = expected_location_link_from_spans(
            foo_uri.clone(),
            foo.resolved_span("baz_click"),
            foo.resolved_span("baz_loc"),
        );

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("baz"),
            foo.begin_column("baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn does_not_jump_to_definition_if_symbol_not_found() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("bar.star", <not_baz_loc>"not_baz"</not_baz_loc>)
            <not_baz>not_baz</not_baz>()
            "#,
        )
        .trim()
        .to_owned();
        let bar_contents = "def baz():\n    pass";
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;

        let expected_location = expected_location_link_from_spans(
            foo_uri.clone(),
            foo.resolved_span("not_baz"),
            foo.resolved_span("not_baz_loc"),
        );

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.set_file_contents(&bar_uri, bar.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("not_baz"),
            foo.begin_column("not_baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn jumps_to_definition_in_load_statement() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("{load}", <baz_click>"<baz>b</baz>az"</baz_click>)
            baz()
            "#,
        )
        .replace("{load}", &uri_to_load_string(&bar_uri))
        .trim()
        .to_owned();
        let bar_contents = "def <baz>baz</baz>():\n    pass";
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;

        let expected_location = expected_location_link_from_spans(
            bar_uri.clone(),
            foo.resolved_span("baz_click"),
            bar.resolved_span("baz"),
        );

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.set_file_contents(&bar_uri, bar.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("baz"),
            foo.begin_column("baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn does_not_jump_to_definition_in_load_statement_if_not_found() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("bar.star", <not_baz_loc>"no<not_baz>t</not_baz>_baz"</not_baz_loc>)
            not_baz()
            "#,
        )
        .trim()
        .to_owned();
        let bar_contents = "def baz():\n    pass";
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;

        let expected_location = expected_location_link_from_spans(
            foo_uri.clone(),
            foo.resolved_span("not_baz_loc"),
            foo.resolved_span("not_baz_loc"),
        );

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.set_file_contents(&bar_uri, bar.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("not_baz"),
            foo.begin_column("not_baz"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);
        Ok(())
    }

    #[test]
    fn jumps_to_file_in_load_statement() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");
        let load_path = bar_uri
            .to_file_path()
            .unwrap()
            .parent()
            .unwrap()
            .join("<bar>b</bar>ar<dot>.</dot>star");
        let bar_load_string = path_to_load_string(&load_path);

        let foo_contents = dedent(
            r#"
            load(<bar_click>"{load}"</bar_click>, "baz")
            baz()
            "#,
        )
        .replace("{load}", &bar_load_string)
        .trim()
        .to_owned();
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;

        let expected_location = LocationLink {
            origin_selection_range: Some(foo.resolved_span("bar_click").into()),
            target_uri: bar_uri,
            target_range: Default::default(),
            target_selection_range: Default::default(),
        };

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri.clone(),
            foo.begin_line("bar"),
            foo.begin_column("bar"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("dot"),
            foo.begin_column("dot"),
        );

        let request_id = server.send_request(goto_definition)?;
        let location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_location, location);

        Ok(())
    }

    #[test]
    fn jumps_to_definitions_in_strings() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let bar_contents = r#""Just <bar>a string</bar>""#;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), bar_contents)?;
        let bar_resolved_span = bar.resolved_span("bar");
        let bar_span_str = format!(
            "{}:{}",
            bar_resolved_span.begin.column, bar_resolved_span.end.column
        );

        let foo_contents = dedent(
            r#"
            <foo1_click>"ba<foo1>r</foo1>.star"</foo1_click>
            [
                <foo2_click>"ba<foo2>r</foo2>.star"</foo2_click>,
                "ignored"
            ]
            {
                <foo3_click>"ba<foo3>r</foo3>.star"</foo3_click>: "ignored",
                "ignored": <foo4_click>"ba<foo4>r</foo4>.star"</foo4_click>,
                "ignored_other": "ignored",
            }

            def f1(x = <foo5_click>"ba<foo5>r</foo5>.star"</foo5_click>):
                <foo6_click>"ba<foo6>r</foo6>.star"</foo6_click>
                [
                    <foo7_click>"ba<foo7>r</foo7>.star"</foo7_click>,
                    "ignored"
                ]
                {
                    <foo8_click>"ba<foo8>r</foo8>.star"</foo8_click>: "ignored",
                    "ignored": <foo9_click>"ba<foo9>r</foo9>.star"</foo9_click>,
                    "ignored_other": "ignored",
                }
                if x == <foo10_click>"ba<foo10>r</foo10>.star"</foo10_click>:
                    <foo11_click>"ba<foo11>r</foo11>.star"</foo11_click>
                    [
                        <foo12_click>"ba<foo12>r</foo12>.star"</foo12_click>,
                        "ignored"
                    ]
                    {
                        <foo13_click>"ba<foo13>r</foo13>.star"</foo13_click>: "ignored",
                        "ignored": <foo14_click>"ba<foo14>r</foo14>.star"</foo14_click>,
                        "ignored_other": "ignored",
                    }
                return <foo15_click>"ba<foo15>r</foo15>.star"</foo15_click>

            foo16 = <foo16_click>"ba<foo16>r</foo16>.star"</foo16_click>

            <baz1_click>"ba<baz1>r</baz1>.star--{bar_range}"</baz1_click>
            [
                <baz2_click>"ba<baz2>r</baz2>.star--{bar_range}"</baz2_click>,
                "ignored"
            ]
            {
                <baz3_click>"ba<baz3>r</baz3>.star--{bar_range}"</baz3_click>: "ignored",
                "ignored": <baz4_click>"ba<baz4>r</baz4>.star--{bar_range}"</baz4_click>,
                "ignored_other": "ignored",
            }

            def f2(x = <baz5_click>"ba<baz5>r</baz5>.star--{bar_range}"</baz5_click>):
                <baz6_click>"ba<baz6>r</baz6>.star--{bar_range}"</baz6_click>
                [
                    <baz7_click>"ba<baz7>r</baz7>.star--{bar_range}"</baz7_click>,
                    "ignored"
                ]
                {
                    <baz8_click>"ba<baz8>r</baz8>.star--{bar_range}"</baz8_click>: "ignored",
                    "ignored": <baz9_click>"ba<baz9>r</baz9>.star--{bar_range}"</baz9_click>,
                    "ignored_other": "ignored",
                }
                if x == <baz10_click>"ba<baz10>r</baz10>.star--{bar_range}"</baz10_click>:
                    <baz11_click>"ba<baz11>r</baz11>.star--{bar_range}"</baz11_click>
                    [
                        <baz12_click>"ba<baz12>r</baz12>.star--{bar_range}"</baz12_click>,
                        "ignored"
                    ]
                    {
                        <baz13_click>"ba<baz13>r</baz13>.star--{bar_range}"</baz13_click>: "ignored",
                        "ignored": <baz14_click>"ba<baz14>r</baz14>.star--{bar_range}"</baz14_click>,
                        "ignored_other": "ignored",
                    }
                return <baz15_click>"ba<baz15>r</baz15>.star--{bar_range}"</baz15_click>

            baz16 = <baz16_click>"ba<baz16>r</baz16>.star--{bar_range}"</baz16_click>
            "#,
        )
        .trim()
        .replace("{bar_range}", &bar_span_str);
        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.open_file(bar_uri.clone(), bar.program())?;

        let mut test = |name: &str, expect_range: bool| -> anyhow::Result<()> {
            let range = if expect_range {
                bar_resolved_span
            } else {
                Default::default()
            };
            let expected_location = expected_location_link_from_spans(
                bar_uri.clone(),
                foo.resolved_span(&format!("{name}_click")),
                range,
            );

            let goto_definition = goto_definition_request(
                &mut server,
                foo_uri.clone(),
                foo.begin_line(name),
                foo.begin_column(name),
            );
            let request_id = server.send_request(goto_definition)?;
            let location = goto_definition_response_location(&mut server, request_id)?;

            assert_eq!(expected_location, location);
            Ok(())
        };

        test("foo1", false)?;
        test("foo2", false)?;
        test("foo3", false)?;
        test("foo4", false)?;
        test("foo5", false)?;
        test("foo6", false)?;
        test("foo7", false)?;
        test("foo8", false)?;
        test("foo9", false)?;
        test("foo10", false)?;
        test("foo11", false)?;
        test("foo12", false)?;
        test("foo13", false)?;
        test("foo14", false)?;
        test("foo15", false)?;
        test("foo16", false)?;

        test("baz1", true)?;
        test("baz2", true)?;
        test("baz3", true)?;
        test("baz4", true)?;
        test("baz5", true)?;
        test("baz6", true)?;
        test("baz7", true)?;
        test("baz8", true)?;
        test("baz9", true)?;
        test("baz10", true)?;
        test("baz11", true)?;
        test("baz12", true)?;
        test("baz13", true)?;
        test("baz14", true)?;
        test("baz15", true)?;
        test("baz16", true)?;

        Ok(())
    }

    #[test]
    fn handles_paths_that_are_not_starlark() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");
        let baz_uri = temp_file_uri("baz");
        let dir1_uri = temp_file_uri("dir1");
        let dir2_uri = temp_file_uri("dir2.star");

        let foo_contents = dedent(
            r#"
            <bar>"b<bar_click>a</bar_click>r.star"</bar>
            <baz>"b<baz_click>a</baz_click>z"</baz>
            <dir1>"d<dir1_click>i</dir1_click>r1"</dir1>
            <dir2>"d<dir2_click>i</dir2_click>r2.star"</dir2>
            "#,
        )
        .trim()
        .to_owned();

        let bar_contents = dedent(
            r#"
            def ba r():
                # This has broken syntax
                pass
            "#,
        )
        .trim()
        .to_owned();

        let foo = FixtureWithRanges::from_fixture("foo.star", &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture("bar.star", &bar_contents)?;

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.set_file_contents(&bar_uri, bar.program())?;
        server.mkdir(dir1_uri.clone());
        server.mkdir(dir2_uri.clone());

        // File with broken syntax
        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri.clone(),
            foo.begin_line("bar_click"),
            foo.begin_column("bar_click"),
        );
        let req_id = server.send_request(goto_definition)?;
        let response = goto_definition_response_location(&mut server, req_id)?;

        let expected = LocationLink {
            origin_selection_range: Some(foo.resolved_span("bar").into()),
            target_uri: bar_uri,
            target_range: Default::default(),
            target_selection_range: Default::default(),
        };
        assert_eq!(expected, response);

        // File that is not starlark at all
        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri.clone(),
            foo.begin_line("baz_click"),
            foo.begin_column("baz_click"),
        );
        let req_id = server.send_request(goto_definition)?;
        let response = goto_definition_response_location(&mut server, req_id)?;

        let expected = LocationLink {
            origin_selection_range: Some(foo.resolved_span("baz").into()),
            target_uri: baz_uri,
            target_range: Default::default(),
            target_selection_range: Default::default(),
        };
        assert_eq!(expected, response);

        // Directory that doesn't look like a starlark file.
        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri.clone(),
            foo.begin_line("dir1_click"),
            foo.begin_column("dir1_click"),
        );
        let req_id = server.send_request(goto_definition)?;
        let response = goto_definition_response_location(&mut server, req_id)?;

        let expected = LocationLink {
            origin_selection_range: Some(foo.resolved_span("dir1").into()),
            target_uri: dir1_uri,
            target_range: Default::default(),
            target_selection_range: Default::default(),
        };
        assert_eq!(expected, response);

        // Directory that looks like a starlark file by name, but isn't
        // File that is not starlark at all
        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("dir2_click"),
            foo.begin_column("dir2_click"),
        );
        let req_id = server.send_request(goto_definition)?;
        let response = goto_definition_response_location(&mut server, req_id)?;

        let expected = LocationLink {
            origin_selection_range: Some(foo.resolved_span("dir2").into()),
            target_uri: dir2_uri,
            target_range: Default::default(),
            target_selection_range: Default::default(),
        };
        assert_eq!(expected, response);
        Ok(())
    }

    #[test]
    fn disables_goto_definition() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let server = TestServer::new_with_settings(Some(LspServerSettings {
            enable_goto_definition: false,
        }))?;

        let goto_definition_disabled = server
            .initialization_result()
            .unwrap()
            .capabilities
            .definition_provider
            .is_none();

        assert!(goto_definition_disabled);

        let server = TestServer::new_with_settings(Some(LspServerSettings {
            enable_goto_definition: true,
        }))?;

        let goto_definition_enabled = server
            .initialization_result()
            .unwrap()
            .capabilities
            .definition_provider
            .is_some();

        assert!(goto_definition_enabled);
        Ok(())
    }

    #[test]
    fn returns_starlark_file_contents() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let mut server = TestServer::new()?;

        let uri = LspUri::try_from(Uri::from_str("starlark:/native/builtin.bzl")?)?;
        let req = server.new_request::<StarlarkFileContentsRequest>(StarlarkFileContentsParams {
            uri: uri.clone(),
        });
        let request_id = server.send_request(req)?;
        let response = server.get_response::<StarlarkFileContentsResponse>(request_id)?;
        assert_eq!(
            server.docs_as_code(&uri).unwrap(),
            response.contents.unwrap()
        );

        let req = server.new_request::<StarlarkFileContentsRequest>(StarlarkFileContentsParams {
            uri: LspUri::try_from(Uri::from_str("starlark:/native/not_builtin.bzl")?)?,
        });
        let request_id = server.send_request(req)?;
        let response = server.get_response::<StarlarkFileContentsResponse>(request_id)?;
        assert!(response.contents.is_none());

        Ok(())
    }

    fn resolve_range_in_string(s: &str, r: Range) -> &str {
        let byte_of_pos = |p: Position| {
            let l = if p.line == 0 {
                0
            } else {
                s.char_indices()
                    .filter(|(_, c)| *c == '\n')
                    .nth((p.line - 1).try_into().unwrap())
                    .unwrap()
                    .0
                    + 1
            };
            l + s[l..]
                .char_indices()
                .nth((p.character).try_into().unwrap())
                .unwrap()
                .0
        };
        let start = byte_of_pos(r.start);
        let end = byte_of_pos(r.end);
        &s[start..end]
    }

    #[test]
    fn goto_works_for_native_symbols() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let native_uri = Uri::from_str("starlark:/native/builtin.bzl")?;

        let mut server = TestServer::new()?;

        let foo_contents = dedent(
            r#"
            <click_n1>na<n1>t</n1>ive_function1</click_n1>()
            def f(<n2_loc>native_function1</n2_loc>):
                print(<click_n2>nat<n2>i</n2>ve_function1</click_n2>)
            mi<n3>s</n3>sing_global()
            "#,
        )
        .trim()
        .to_owned();

        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;

        server.open_file(foo_uri.clone(), foo.program())?;

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri.clone(),
            foo.begin_line("n1"),
            foo.begin_column("n1"),
        );
        let request_id = server.send_request(goto_definition)?;
        let n1_location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(
            n1_location.origin_selection_range,
            Some(foo.resolved_span("click_n1").into())
        );
        assert_eq!(n1_location.target_uri, native_uri);
        let native_gen_code = server
            .docs_as_code(&native_uri.try_into().unwrap())
            .unwrap();
        let target_str = resolve_range_in_string(&native_gen_code, n1_location.target_range);
        assert_eq!(target_str, "native_function1");

        let expected_n2_location = expected_location_link_from_spans(
            foo_uri.clone(),
            foo.resolved_span("click_n2"),
            foo.resolved_span("n2_loc"),
        );

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri.clone(),
            foo.begin_line("n2"),
            foo.begin_column("n2"),
        );
        let request_id = server.send_request(goto_definition)?;
        let n2_location = goto_definition_response_location(&mut server, request_id)?;

        assert_eq!(expected_n2_location, n2_location);

        let goto_definition = goto_definition_request(
            &mut server,
            foo_uri,
            foo.begin_line("n3"),
            foo.begin_column("n3"),
        );
        let request_id = server.send_request(goto_definition)?;
        let n3_response = server.get_response::<GotoDefinitionResponse>(request_id)?;
        match n3_response {
            GotoDefinitionResponse::Array(definitions) if definitions.is_empty() => Ok(()),
            response => Err(anyhow::anyhow!(
                "Expected empty definitions, got `{:?}`",
                response
            )),
        }?;

        Ok(())
    }

    #[test]
    fn jumps_to_original_member_definition() -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }

        let foo_uri = temp_file_uri("foo.star");
        let bar_uri = temp_file_uri("bar.star");

        let foo_contents = dedent(
            r#"
            load("{load}", "loaded")

            def <dest_baz>_baz</dest_baz>():
                pass

            <dest_quz>_quz</dest_quz> = 6

            <dest_root><dest_foobar>FooBarModule</dest_foobar></dest_root> = 5
            FooModule = struct(<dest_foo>foo</dest_foo> = 5)
            # Member value does not exist
            BarModule = struct(<dest_bar>bar</dest_bar> = bar)
            BazModule = struct(
                bar = bar,
                baz = _baz,
            )
            QuzModule = struct(bar = bar, baz = _baz, quz = _quz)

            <root>Foo<root_click>Bar</root_click>Module</root>.<foobar>f<foobar_click>o</foobar_click>obar</foobar>
            FooModule.<foo>f<foo_click>o</foo_click>o</foo>
            BarModule.<bar>b<bar_click>a</bar_click>r</bar>
            BazModule.<baz>b<baz_click>a</baz_click>z</baz>
            QuzModule.<quz>q<quz_click>u</quz_click>z</quz>
            loaded.<x><x_click>x</x_click></x>
            loaded.<y><y_click>y</y_click></y>
            "#,
        )
        .replace("{load}", &uri_to_load_string(&bar_uri))
        .trim()
        .to_owned();

        let bar_contents = dedent(
            r#"
            def <dest_x>_x</dest_x>():
                pass
            <dest_y>loaded</dest_y> = struct(x = _x)
            "#,
        )
        .trim()
        .to_owned();

        let foo = FixtureWithRanges::from_fixture(foo_uri.path().as_str(), &foo_contents)?;
        let bar = FixtureWithRanges::from_fixture(bar_uri.path().as_str(), &bar_contents)?;

        let mut server = TestServer::new()?;
        server.open_file(foo_uri.clone(), foo.program())?;
        server.open_file(bar_uri.clone(), bar.program())?;

        let cases = [
            (&foo, &foo_uri, "root"),
            (&foo, &foo_uri, "foobar"),
            (&foo, &foo_uri, "foo"),
            (&foo, &foo_uri, "bar"),
            (&foo, &foo_uri, "baz"),
            (&foo, &foo_uri, "quz"),
            (&bar, &bar_uri, "x"),
            (&bar, &bar_uri, "y"),
        ];

        let expected_results = cases
            .iter()
            .map(|(fixture, uri, id)| {
                expected_location_link_from_spans(
                    (*uri).clone(),
                    foo.resolved_span(id),
                    fixture.resolved_span(&format!("dest_{id}")),
                )
            })
            .collect::<Vec<_>>();

        let requests = cases
            .iter()
            .map(|(_, _, id)| {
                goto_definition_request(
                    &mut server,
                    foo_uri.clone(),
                    foo.begin_line(&format!("{id}_click")),
                    foo.begin_column(&format!("{id}_click")),
                )
            })
            .collect::<Vec<_>>();

        for (case, request, expected) in itertools::izip!(cases, requests, expected_results) {
            let req_id = server.send_request(request)?;
            let response = goto_definition_response_location(&mut server, req_id)
                .context(format!("getting response for case `{}`", case.2))?;

            assert_eq!(
                expected, response,
                "Incorrect response for case `{}`",
                case.2
            );
        }
        Ok(())
    }

    /// Opens a file and discards its diagnostics, so that programs with lints can be used.
    fn open_without_checking_diagnostics(
        server: &mut TestServer,
        uri: &Uri,
        text: &str,
    ) -> anyhow::Result<()> {
        server.send_notification(new_notification::<DidOpenTextDocument>(
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: String::new(),
                    version: 1,
                    text: text.to_owned(),
                },
            },
        ))?;
        server.get_notification::<PublishDiagnostics>()?;
        Ok(())
    }

    fn goto_definition_links(
        server: &mut TestServer,
        uri: &Uri,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<LocationLink>> {
        let request = goto_definition_request(server, uri.clone(), line, character);
        let id = server.send_request(request)?;
        match server.get_response::<GotoDefinitionResponse>(id)? {
            GotoDefinitionResponse::Link(links) => Ok(links),
            // An empty `Link` response deserializes as an empty `Array`.
            GotoDefinitionResponse::Array(locations) if locations.is_empty() => Ok(Vec::new()),
            other => Err(anyhow::anyhow!("unexpected response: {other:?}")),
        }
    }

    fn range(begin_line: u32, begin_column: u32, end_line: u32, end_column: u32) -> Range {
        Range::new(
            Position::new(begin_line, begin_column),
            Position::new(end_line, end_column),
        )
    }

    /// The cursor column counts UTF-16 code units; after non-ASCII text on the same line,
    /// go-to-definition still resolves the token under the cursor.
    #[test]
    fn goto_definition_after_non_ascii_text_resolves_the_token_under_the_cursor()
    -> anyhow::Result<()> {
        if is_wasm() {
            return Ok(());
        }
        let uri = temp_file_uri("non_ascii.star");
        let mut server = TestServer::new()?;
        open_without_checking_diagnostics(&mut server, &uri, "x = 1\ny = (\"日本語\", x)\n")?;
        let links = goto_definition_links(&mut server, &uri, 1, 12)?;
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].origin_selection_range, Some(range(1, 12, 1, 13)));
        assert_eq!(links[0].target_range, range(0, 0, 0, 1));
        Ok(())
    }
}
