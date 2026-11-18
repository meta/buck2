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

use std::iter;

use starlark::codemap::CodeMap;
use starlark::codemap::Pos;
use starlark::codemap::ResolvedSpan;
use starlark::codemap::Span;
use starlark::codemap::Spanned;
use starlark::syntax::AstModule;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::syntax::ast::ArgumentP;
use starlark_syntax::syntax::ast::AssignTargetP;
use starlark_syntax::syntax::ast::AstIdent;
use starlark_syntax::syntax::ast::AstLiteral;
use starlark_syntax::syntax::ast::AstNoPayload;
use starlark_syntax::syntax::ast::AstString;
use starlark_syntax::syntax::ast::Expr;
use starlark_syntax::syntax::ast::ExprP;
use starlark_syntax::syntax::ast::LoadArgP;
use starlark_syntax::syntax::ast::Stmt;
use starlark_syntax::syntax::ast::StmtP;
use starlark_syntax::syntax::module::AstModuleFields;
use starlark_syntax::syntax::top_level_stmts::top_level_stmts;
use starlark_syntax::syntax::uniplate::Visit;

use crate::bind::Assigner;
use crate::bind::Bind;
use crate::bind::Scope;
use crate::bind::scope;
use crate::exported::AstModuleExportedSymbols;
use crate::exported::Symbol;
use crate::loaded::AstModuleLoadedSymbols;
use crate::loaded::LoadedSymbol;

/// Converts an LSP column, counted in UTF-16 code units, into a byte offset within `line`.
///
/// A column past the end of the line maps to the end of the line.
pub(crate) fn utf16_column_to_byte_offset(line: &str, column: u32) -> usize {
    let mut units = 0;
    for (offset, c) in line.char_indices() {
        if units >= column {
            return offset;
        }
        units += c.len_utf16() as u32;
    }
    line.len()
}

/// The location of a definition for a given identifier. See [`AstModule::find_definition_at_location`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum IdentifierDefinition {
    /// The definition was found at this location in the current file.
    Location {
        source: ResolvedSpan,
        destination: ResolvedSpan,
        name: String,
    },
    /// The symbol was loaded from another file. "destination" is the position within the
    /// "load()" statement, but additionally, the path in that load statement, and the
    /// name of the symbol within that file are provided so that additional lookups can
    /// happen to find the original definition.
    LoadedLocation {
        source: ResolvedSpan,
        destination: ResolvedSpan,
        path: String,
        name: String,
    },
    /// The symbol is the path component of a `load` statement. This is the raw string
    /// that is in the AST, and needs to be properly resolved to a path to be useful.
    LoadPath { source: ResolvedSpan, path: String },
    /// A literal string in the source code. This can be a standalone string, or part of e.g. a
    /// list, a dictionary, a return value, etc.
    StringLiteral {
        source: ResolvedSpan,
        literal: String,
    },
    /// A named symbol was found, but it was not found in any of the current scopes. It
    /// should be considered a global symbol, and attempted to be resolved externally.
    Unresolved { source: ResolvedSpan, name: String },
    /// Either the provided location was not an access of a variable, or no definition
    /// could be found.
    NotFound,
}

impl IdentifierDefinition {
    fn source(&self) -> Option<ResolvedSpan> {
        match self {
            IdentifierDefinition::Location { source, .. }
            | IdentifierDefinition::LoadedLocation { source, .. }
            | IdentifierDefinition::LoadPath { source, .. }
            | IdentifierDefinition::StringLiteral { source, .. }
            | IdentifierDefinition::Unresolved { source, .. } => Some(*source),
            IdentifierDefinition::NotFound => None,
        }
    }
}

/// A definition as in [`IdentifierDefinition`], but the source is within a dot expression.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct DottedDefinition {
    /// The location of the member access that was requested.
    pub(crate) source: ResolvedSpan,
    /// The [`IdentifierDefinition`] for the left most component of the expression.
    ///
    /// For example, this would be the location of `x` if `source` enclosed `y` in `x.y.z`.
    pub(crate) root_definition_location: IdentifierDefinition,
    /// All of the identifiers up to the one that `source` includes.
    ///
    /// For example. if the `y` in `x.y.z` is the source, this would contain `x` and `y`.
    pub(crate) segments: Vec<String>,
}

/// An inner structure that only holds references and unresolved spans. Used while
/// finding locations for identifiers. Resolved to a fuller, owned structure
/// ([`IdentifierDefinition`]) after the entire AST is visited.
///
/// This references temporary variables, so cannot be returned to external users directly.
#[derive(Debug, Clone, Eq, PartialEq)]
enum TempIdentifierDefinition<'a> {
    /// The location of the definition of the symbol at the current line/column
    Location {
        source: Span,
        destination: Span,
        name: &'a str,
    },
    LoadedLocation {
        source: Span,
        destination: Span,
        path: &'a str,
        name: &'a str,
    },
    /// The definition was not found in the current scope but the name of an identifier
    /// was found at the given position. This should be checked in outer scopes
    /// to attempt to find the definition.
    Name { source: Span, name: &'a str },
    /// None of the accesses matched the position that was provided.
    NotFound,
}

/// Reference version of [`DottedDefinitionLocation`]. Analogous to [`IdentifierDefinition`].
#[derive(Debug, Clone, Eq, PartialEq)]
struct TempDottedDefinition<'a> {
    /// The location of the member access that was requested.
    source: Span,
    /// The [`IdentifierDefinition`] for the left most component of the expression.
    ///
    /// For example, this would be the location of `x` if `source` enclosed `y` in `x.y.z`.
    root_definition_location: TempIdentifierDefinition<'a>,
    /// The variable you are pointing at, e.g. `x.y.z` would be `x`.
    variable: &'a AstIdent,
    /// All of the attributes up to the one that `source` includes.
    ///
    /// For example. if the `y` in `x.y.z` is the source, this would contain `y`.
    attributes: &'a [AstString],
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum TempDefinition<'a> {
    Identifier(TempIdentifierDefinition<'a>),
    Dotted(TempDottedDefinition<'a>),
}

impl<'a> From<TempIdentifierDefinition<'a>> for TempDefinition<'a> {
    fn from(def: TempIdentifierDefinition<'a>) -> Self {
        Self::Identifier(def)
    }
}

impl<'a> From<TempDottedDefinition<'a>> for TempDefinition<'a> {
    fn from(def: TempDottedDefinition<'a>) -> Self {
        Self::Dotted(def)
    }
}

/// The container for both definition locations of a standalone identifier, and
/// for ones that access members (via '.' syntax)
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum Definition {
    Identifier(IdentifierDefinition),
    Dotted(DottedDefinition),
}

impl From<IdentifierDefinition> for Definition {
    fn from(def: IdentifierDefinition) -> Self {
        Self::Identifier(def)
    }
}

impl From<DottedDefinition> for Definition {
    fn from(def: DottedDefinition) -> Self {
        Self::Dotted(def)
    }
}

impl Definition {
    /// Get the "destination" of this location, but only within the current module.
    ///
    /// Some definition location types do not have a local definition.
    fn local_destination(&self) -> Option<ResolvedSpan> {
        match self {
            Definition::Identifier(i)
            | Definition::Dotted(DottedDefinition {
                root_definition_location: i,
                ..
            }) => match i {
                IdentifierDefinition::Location { destination, .. }
                | IdentifierDefinition::LoadedLocation { destination, .. } => Some(*destination),
                _ => None,
            },
        }
    }

    pub(crate) fn source(&self) -> Option<ResolvedSpan> {
        match self {
            Definition::Identifier(i) => i.source(),
            Definition::Dotted(DottedDefinition { source, .. }) => Some(*source),
        }
    }
}

/// Container that holds an AST module and returns things like definition locations,
/// lists of symbols, etc.
pub(crate) struct LspModule {
    pub(crate) ast: AstModule,
}

impl LspModule {
    pub(crate) fn new(ast: AstModule) -> Self {
        Self { ast }
    }

    /// Attempts to find the location where a symbol is defined in the module.
    ///
    /// `line` and `col` are zero based indexes of a location of the symbol to attempt to lookup.
    ///
    /// This method also handles scoping properly (i.e. an access of "foo" in a function
    /// will return location of the parameter "foo", even if there is a global called "foo").
    pub(crate) fn find_definition_at_location(&self, line: u32, col: u32) -> Definition {
        // TODO(nmj): This should probably just store references to all of the AST nodes
        //            when the LSPModule object is created, and then we can do a much faster
        //            lookup, especially in cases where a file has not been changed, so the
        //            LSPModule doesn't need to reparse anything.

        let scope = scope(&self.ast);
        let line_span = match self.ast.codemap().line_span_opt(line as usize) {
            None => {
                // The document got edited to add new lines, just bail out
                return Definition::Identifier(IdentifierDefinition::NotFound);
            }
            Some(line_span) => line_span,
        };
        let line_text = self.ast.codemap().source_span(line_span);
        let current_pos = line_span.begin() + utf16_column_to_byte_offset(line_text, col) as u32;

        // Finalize the results after recursing down from and back up to the top level scope.
        match Self::find_definition_in_scope(&scope, current_pos) {
            TempDefinition::Identifier(def) => self
                .get_definition_location(def, &scope, current_pos)
                .into(),
            TempDefinition::Dotted(def) => DottedDefinition {
                source: self.ast.codemap().resolve_span(def.source),
                root_definition_location: self.get_definition_location(
                    def.root_definition_location,
                    &scope,
                    current_pos,
                ),
                segments: iter::once(def.variable.ident.clone())
                    .chain(def.attributes.map(|s| s.node.to_owned()))
                    .collect(),
            }
            .into(),
        }
    }

    /// Look at the given scope and child scopes to try to find where the identifier
    /// accessed at Pos is defined.
    fn find_definition_in_scope(scope: &Scope, pos: Pos) -> TempDefinition<'_> {
        /// Look for a name in the given scope, with a given source, and return the right
        /// type of [`TempIdentifierDefinition`] based on whether / how the variable is bound.
        fn resolve_get_in_scope<'a>(
            scope: &'a Scope,
            name: &'a str,
            source: Span,
        ) -> TempIdentifierDefinition<'a> {
            match scope.bound.get(name) {
                Some((Assigner::Load { path, name }, span)) => {
                    TempIdentifierDefinition::LoadedLocation {
                        source,
                        destination: *span,
                        path,
                        name: name.as_str(),
                    }
                }
                Some((_, span)) => TempIdentifierDefinition::Location {
                    source,
                    destination: *span,
                    name,
                },
                // We know the symbol name, but it might only be available in
                // an outer scope.
                None => TempIdentifierDefinition::Name { source, name },
            }
        }

        for bind in &scope.inner {
            match bind {
                Bind::Get(g) if g.span.contains(pos) => {
                    return resolve_get_in_scope(scope, g.node.ident.as_str(), g.span).into();
                }
                Bind::Scope(inner_scope) => {
                    match Self::find_definition_in_scope(inner_scope, pos) {
                        TempDefinition::Identifier(TempIdentifierDefinition::Name {
                            source,
                            name,
                        }) => {
                            return resolve_get_in_scope(scope, name, source).into();
                        }
                        TempDefinition::Identifier(TempIdentifierDefinition::NotFound) => {}
                        TempDefinition::Dotted(TempDottedDefinition {
                            source,
                            root_definition_location:
                                TempIdentifierDefinition::Name {
                                    source: root_source,
                                    name: root_name,
                                },
                            variable,
                            attributes,
                        }) => {
                            return TempDefinition::Dotted(TempDottedDefinition {
                                source,
                                root_definition_location: resolve_get_in_scope(
                                    scope,
                                    root_name,
                                    root_source,
                                ),
                                variable,
                                attributes,
                            });
                        }
                        x @ TempDefinition::Identifier(TempIdentifierDefinition::Location {
                            ..
                        })
                        | x @ TempDefinition::Identifier(
                            TempIdentifierDefinition::LoadedLocation { .. },
                        )
                        | x @ TempDefinition::Dotted(_) => {
                            return x;
                        }
                    }
                }
                Bind::GetDotted(dotted) => {
                    if let Some((idx, source)) = dotted.contains(pos) {
                        let root_identifier = &dotted.variable;
                        let root_definition_location = resolve_get_in_scope(
                            scope,
                            root_identifier.node.ident.as_str(),
                            root_identifier.span,
                        );
                        // If someone clicks on the "root" identifier, just treat it as a "get"
                        match idx {
                            None => return root_definition_location.into(),
                            Some(idx) => {
                                return TempDottedDefinition {
                                    source,
                                    root_definition_location,
                                    variable: &dotted.variable,
                                    attributes: &dotted.attributes[0..idx + 1],
                                }
                                .into();
                            }
                        }
                    }
                }
                // For everything else, just ignore it. Note that the `Get` is ignored
                // because we already checked the pos above.
                Bind::Set(_, _) | Bind::Flow | Bind::Get(_) => {}
            }
        }
        TempIdentifierDefinition::NotFound.into()
    }

    /// Converts a `TempIdentifierDefinition` to an `IdentifierDefinition`, resolving spans
    /// against the current AST, owning strings, etc.
    fn get_definition_location(
        &self,
        definition: TempIdentifierDefinition,
        scope: &Scope,
        current_pos: Pos,
    ) -> IdentifierDefinition {
        match definition {
            TempIdentifierDefinition::Location {
                source,
                destination,
                name,
            } => IdentifierDefinition::Location {
                source: self.ast.codemap().resolve_span(source),
                destination: self.ast.codemap().resolve_span(destination),
                name: name.to_owned(),
            },
            TempIdentifierDefinition::Name { source, name } => match scope.bound.get(name) {
                None => IdentifierDefinition::Unresolved {
                    source: self.ast.codemap().resolve_span(source),
                    name: name.to_owned(),
                },
                Some((Assigner::Load { path, name }, span)) => {
                    IdentifierDefinition::LoadedLocation {
                        source: self.ast.codemap().resolve_span(source),
                        destination: self.ast.codemap().resolve_span(*span),
                        path: path.node.clone(),
                        name: name.node.clone(),
                    }
                }
                Some((_, span)) => IdentifierDefinition::Location {
                    source: self.ast.codemap().resolve_span(source),
                    destination: self.ast.codemap().resolve_span(*span),
                    name: name.to_owned(),
                },
            },
            // If we could not find the symbol, see if the current position is within
            // a load statement (these are not exposed as Get/Set in bind).
            TempIdentifierDefinition::NotFound => self.find_definition_from_ast(current_pos),
            TempIdentifierDefinition::LoadedLocation {
                source,
                destination,
                path,
                name,
            } => IdentifierDefinition::LoadedLocation {
                source: self.ast.codemap().resolve_span(source),
                destination: self.ast.codemap().resolve_span(destination),
                path: path.to_owned(),
                name: name.to_owned(),
            },
        }
    }

    /// Get the list of symbols exported by this module.
    pub(crate) fn get_exported_symbols(&self) -> Vec<Symbol> {
        self.ast.exported_symbols()
    }

    /// Get the list of symbols loaded by this module.
    pub(crate) fn get_loaded_symbols(&self) -> Vec<LoadedSymbol<'_>> {
        self.ast.loaded_symbols()
    }

    /// Attempt to find an exported symbol with the given name.
    pub(crate) fn find_exported_symbol(&self, name: &str) -> Option<Symbol> {
        self.ast
            .exported_symbols()
            .into_iter()
            .find(|symbol| symbol.name == name)
    }

    /// Attempt to find the location in this module where an exported symbol is defined.
    pub(crate) fn find_exported_symbol_span(&self, name: &str) -> Option<ResolvedSpan> {
        self.find_exported_symbol(name)
            .map(|symbol| symbol.span.resolve_span())
    }

    /// Attempt to find the location in this module where a member of a struct (named `name`)
    /// is defined.
    ///
    /// This helps with the common idiom of
    /// ```python
    /// ...
    /// Foo = struct(
    ///     member1 = _member1,
    /// )
    /// ```
    ///
    /// which is imported by other files and used as `Foo.member1`. Rather than jumping to `Foo`,
    /// this would jump to `_member1` if it exists.
    pub(crate) fn find_exported_symbol_and_member(
        &self,
        name: &str,
        member: &str,
    ) -> Option<ResolvedSpan> {
        // If we can't fully resolve the symbol, try to get as "close" as possible.
        // Whether this is the left hand side of an assign statement, or maybe the
        // left side of a named argument in the struct that matches the member name.
        let mut arg_span = None;
        let mut identifier_span = None;
        let mut symbol_to_lookup = None;

        'outer: for v in top_level_stmts(self.ast.statement()) {
            if let StmtP::Assign(assign) = &v.node {
                let main_assign_span = match &assign.lhs.node {
                    AssignTargetP::Identifier(main_assign_id) if main_assign_id.ident == name => {
                        main_assign_id.span
                    }
                    _ => {
                        continue 'outer;
                    }
                };
                // If nothing else, go to the left hand of the assignment expression.
                if identifier_span.is_none() {
                    identifier_span = Some(main_assign_span);
                }

                // Look for a function call to `struct`.
                if let ExprP::Call(function_name, args) = &assign.rhs.node {
                    match &function_name.node {
                        ExprP::Identifier(function_name)
                            if function_name.node.ident == "struct" => {}
                        _ => {
                            continue 'outer;
                        }
                    }

                    for arg in &args.args {
                        if let ArgumentP::Named(arg_name, arg_expr) = &arg.node {
                            if arg_name.node != member {
                                continue;
                            }
                            if arg_span.is_none() {
                                arg_span = Some(arg_name.span);
                            }
                            if let ExprP::Identifier(arg_value_id) = &arg_expr.node {
                                symbol_to_lookup = Some(arg_value_id.span);
                                break 'outer;
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Try to find the symbol that is assigned, but if not, try to get to that "closest" span.
        symbol_to_lookup
            .and_then(|span| {
                let resolved = self.ast.codemap().resolve_span(span);
                self.find_definition_at_location(
                    resolved.begin.line as u32,
                    resolved.begin.column as u32,
                )
                .local_destination()
            })
            .or_else(|| match (arg_span, identifier_span) {
                (Some(span), _) => Some(self.ast.codemap().resolve_span(span)),
                (None, Some(span)) => Some(self.ast.codemap().resolve_span(span)),
                (None, None) => None,
            })
    }

    fn find_definition_from_ast(&self, pos: Pos) -> IdentifierDefinition {
        fn visit_node(
            codemap: &CodeMap,
            pos: Pos,
            ret: &mut Option<IdentifierDefinition>,
            node: Visit<AstNoPayload>,
        ) {
            if ret.is_some() {
                return;
            }
            match node {
                Visit::Expr(Spanned {
                    node: Expr::Literal(AstLiteral::String(s)),
                    ..
                }) if s.span.contains(pos) => {
                    *ret = Some(IdentifierDefinition::StringLiteral {
                        source: codemap.resolve_span(s.span),
                        literal: s.node.to_owned(),
                    });
                }
                Visit::Stmt(Spanned {
                    node: Stmt::Load(load),
                    ..
                }) => {
                    *ret = if load.module.span.contains(pos) {
                        Some(IdentifierDefinition::LoadPath {
                            source: codemap.resolve_span(load.module.span),
                            path: load.module.node.to_owned(),
                        })
                    } else {
                        load.args.iter().find_map(|LoadArgP { local, their, .. }| {
                            if local.span.contains(pos) || their.span.contains(pos) {
                                Some(IdentifierDefinition::LoadedLocation {
                                    source: codemap.resolve_span(their.span),
                                    destination: codemap.resolve_span(their.span),
                                    path: load.module.node.to_owned(),
                                    name: their.node.to_owned(),
                                })
                            } else {
                                None
                            }
                        })
                    }
                }
                v => v.visit_children(|node| visit_node(codemap, pos, ret, node)),
            }
        }

        let mut ret = None;
        visit_node(
            self.ast.codemap(),
            pos,
            &mut ret,
            Visit::Stmt(self.ast.statement()),
        );
        ret.unwrap_or(IdentifierDefinition::NotFound)
    }
}

#[cfg(test)]
pub(crate) mod helpers {
    use std::collections::HashMap;
    use std::collections::hash_map::Entry;

    use starlark::syntax::Dialect;
    use starlark_syntax::codemap::ResolvedPos;
    use textwrap::dedent;

    use super::*;

    /// Result of parsing a starlark fixture that has range markers in it. See `FixtureWithRanges::from_fixture`
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct FixtureWithRanges {
        filename: String,
        /// The starlark program with markers removed.
        program: String,
        /// The location of all of the symbols that were indicated by the test fixture.
        ranges: HashMap<String, ResolvedSpan>,
    }

    impl FixtureWithRanges {
        /// Parse a program that has markers in it indicating a "range" by name.
        ///
        /// Markers are simple `<\w+>` and `</\w+>` tags. The returned program is the
        /// program content with these markers removed.
        ///
        /// For example:
        /// ```#starlark
        /// load("foo.star", <a>"bar"</a>);
        ///
        /// x = 1
        /// <bar_highlight><bar_click>b</bar_click>ar</bar_highlight>(<x>x</x>)
        /// ```
        ///
        /// Would return a program body of:
        /// ```#starlark
        /// load("foo.star", "bar");
        ///
        /// x = 1
        /// bar(x)
        /// ```
        ///
        /// And ranges of:
        ///  `a`: 0:17-0:22
        ///  `bar_highlight`: 3:0-3:2
        ///  `bar_click`: 3:0-3:1
        ///  `x`: 3:4-3:5
        pub(crate) fn from_fixture(filename: &str, fixture: &str) -> anyhow::Result<Self> {
            let re = regex::Regex::new(r#"<(/)?(\w+)>"#).unwrap();
            let mut program = String::new();
            let mut ranges: HashMap<String, (Option<usize>, Option<usize>)> = HashMap::new();

            let mut fixture_idx = 0;
            for matches in re.captures_iter(fixture) {
                let full_tag = matches.get_match();
                let is_end_tag = matches.get(1).is_some();
                let identifier = matches.get(2).unwrap().as_str().to_owned();

                program.push_str(&fixture[fixture_idx..full_tag.start()]);
                fixture_idx = full_tag.end();

                match (is_end_tag, ranges.entry(identifier.clone())) {
                    (false, Entry::Occupied(_)) => {
                        return Err(anyhow::anyhow!("duplicate open tag `{}` found", identifier));
                    }
                    (false, Entry::Vacant(e)) => {
                        e.insert((Some(program.len()), None));
                    }
                    (true, Entry::Occupied(mut e)) => {
                        e.insert((e.get().0, Some(program.len())));
                    }
                    (true, Entry::Vacant(_)) => {
                        return Err(anyhow::anyhow!(
                            "found closing tag for `{}`, but no open tag",
                            identifier
                        ));
                    }
                }
            }
            program.push_str(&fixture[fixture_idx..fixture.len()]);

            let code_map = CodeMap::new(filename.to_owned(), program.clone());
            let spans: HashMap<String, ResolvedSpan> = ranges
                .into_iter()
                .map(|(id, (start, end))| {
                    let span = Span::new(
                        Pos::new(start.unwrap() as u32),
                        Pos::new(end.unwrap() as u32),
                    );
                    (id, code_map.resolve_span(span))
                })
                .collect();

            Ok(Self {
                filename: filename.to_owned(),
                program,
                ranges: spans,
            })
        }

        pub(crate) fn begin_line(&self, identifier: &str) -> u32 {
            self.ranges
                .get(identifier)
                .expect("identifier to be present")
                .begin
                .line as u32
        }

        pub(crate) fn begin_column(&self, identifier: &str) -> u32 {
            self.ranges
                .get(identifier)
                .expect("identifier to be present")
                .begin
                .column as u32
        }

        pub(crate) fn resolved_span(&self, identifier: &str) -> ResolvedSpan {
            *self
                .ranges
                .get(identifier)
                .expect("identifier to be present")
        }

        pub(crate) fn module(&self) -> starlark::Result<LspModule> {
            Ok(LspModule::new(AstModule::parse(
                &self.filename,
                self.program.clone(),
                // TODO(nga): use dialect of current module.
                &Dialect::AllOptionsInternal,
            )?))
        }

        pub(crate) fn program(&self) -> String {
            self.program.clone()
        }
    }

    #[test]
    fn parses_fixtures() -> anyhow::Result<()> {
        let fixture = dedent(
            r#"
            load("foo.star", <a>"bar"</a>);

            x = 1
            <bar_highlight><bar_click>b</bar_click>ar</bar_highlight>(<x>x</x>)
            "#,
        )
        .trim()
        .to_owned();

        let expected_program = dedent(
            r#"
            load("foo.star", "bar");

            x = 1
            bar(x)
            "#,
        )
        .trim()
        .to_owned();

        let expected_locations = [
            ("a", 0, 17, 0, 22),
            ("bar_highlight", 3, 0, 3, 3),
            ("bar_click", 3, 0, 3, 1),
            ("x", 3, 4, 3, 5),
        ]
        .into_iter()
        .map(|(id, begin_line, begin_column, end_line, end_column)| {
            let span = ResolvedSpan {
                begin: ResolvedPos {
                    line: begin_line,
                    column: begin_column,
                },
                end: ResolvedPos {
                    line: end_line,
                    column: end_column,
                },
            };
            (id.to_owned(), span)
        })
        .collect();

        let expected = FixtureWithRanges {
            filename: "test.star".to_owned(),
            program: expected_program,
            ranges: expected_locations,
        };

        let parsed = FixtureWithRanges::from_fixture("test.star", &fixture)?;

        assert_eq!(expected, parsed);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use textwrap::dedent;

    use super::helpers::*;
    use super::*;

    #[test]
    fn find_definition_loaded_symbol() -> starlark::Result<()> {
        let contents = dedent(
            r#"
        load("bar.star", <print>print</print> = "other_print");

        x = 1
        y = 2

        def add(y: int):
            return x + y

        def invalid_symbol():
            return y + z

        <print_click><p1>p</p1>r<p2>i</p2>n<p3>t</p3></print_click>(x)
        add(3)
        invalid_symbol()
        "#,
        )
        .trim()
        .to_owned();
        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        let expected: Definition = IdentifierDefinition::LoadedLocation {
            source: parsed.resolved_span("print_click"),
            destination: parsed.resolved_span("print"),
            path: "bar.star".to_owned(),
            name: "other_print".to_owned(),
        }
        .into();
        assert_eq!(
            expected,
            module.find_definition_at_location(parsed.begin_line("p1"), parsed.begin_column("p1"))
        );
        assert_eq!(
            expected,
            module.find_definition_at_location(parsed.begin_line("p2"), parsed.begin_column("p2"))
        );
        assert_eq!(
            expected,
            module.find_definition_at_location(parsed.begin_line("p3"), parsed.begin_column("p3"))
        );
        Ok(())
    }

    #[test]
    fn find_definition_function_calls() -> starlark::Result<()> {
        let contents = dedent(
            r#"
        load("bar.star", "print");

        x = 1
        y = 2

        def <add>add</add>(y: int):
            return x + y

        def <invalid_symbol>invalid_symbol</invalid_symbol>():
            return y + z

        print(x)
        <add_click><a1>a</a1><a2>d</a2><a3>d</a3></add_click>(3)
        <invalid_symbol_click><i1>i</i1>nv<i2>a</i2>lid_symbo<i3>l</i3></invalid_symbol_click>()
        "#,
        )
        .trim()
        .to_owned();
        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        let expected_add = Definition::from(IdentifierDefinition::Location {
            source: parsed.resolved_span("add_click"),
            destination: parsed.resolved_span("add"),
            name: "add".to_owned(),
        });
        let expected_invalid = Definition::from(IdentifierDefinition::Location {
            source: parsed.resolved_span("invalid_symbol_click"),
            destination: parsed.resolved_span("invalid_symbol"),
            name: "invalid_symbol".to_owned(),
        });

        assert_eq!(
            expected_add,
            module.find_definition_at_location(parsed.begin_line("a1"), parsed.begin_column("a1"))
        );
        assert_eq!(
            expected_add,
            module.find_definition_at_location(parsed.begin_line("a2"), parsed.begin_column("a2"))
        );
        assert_eq!(
            expected_add,
            module.find_definition_at_location(parsed.begin_line("a3"), parsed.begin_column("a3"))
        );

        assert_eq!(
            expected_invalid,
            module.find_definition_at_location(parsed.begin_line("i1"), parsed.begin_column("i1"))
        );
        assert_eq!(
            expected_invalid,
            module.find_definition_at_location(parsed.begin_line("i2"), parsed.begin_column("i2"))
        );
        assert_eq!(
            expected_invalid,
            module.find_definition_at_location(parsed.begin_line("i3"), parsed.begin_column("i3"))
        );
        Ok(())
    }

    #[test]
    fn find_definition_function_params() -> starlark::Result<()> {
        let contents = dedent(
            r#"
        load("bar.star", "print");

        <x>x</x> = 1
        y = 2

        def add(y: int):
            return x + y

        def invalid_symbol():
            return y + z

        print(<x_param>x</x_param>)
        add(3)
        invalid_symbol()
        "#,
        )
        .trim()
        .to_owned();
        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        assert_eq!(
            Definition::from(IdentifierDefinition::Location {
                source: parsed.resolved_span("x_param"),
                destination: parsed.resolved_span("x"),
                name: "x".to_owned(),
            }),
            module.find_definition_at_location(
                parsed.begin_line("x_param"),
                parsed.begin_column("x_param")
            )
        );
        Ok(())
    }

    #[test]
    fn find_definition_scopes_locals() -> starlark::Result<()> {
        let contents = dedent(
            r#"
        load("bar.star", "print");

        <x>x</x> = 1
        <y1>y</y1> = 2

        def add(<y2>y</y2>: int):
            return <x_var>x</x_var> + <y_var1>y</y_var1>

        def invalid_symbol():
            return <y_var2>y</y_var2> + <z_var>z</z_var>

        print(x)
        add(3)
        invalid_symbol()
        "#,
        )
        .trim()
        .to_owned();
        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        assert_eq!(
            Definition::from(IdentifierDefinition::Location {
                source: parsed.resolved_span("x_var"),
                destination: parsed.resolved_span("x"),
                name: "x".to_owned(),
            }),
            module.find_definition_at_location(
                parsed.begin_line("x_var"),
                parsed.begin_column("x_var")
            )
        );
        assert_eq!(
            Definition::from(IdentifierDefinition::Location {
                source: parsed.resolved_span("y_var1"),
                destination: parsed.resolved_span("y2"),
                name: "y".to_owned(),
            }),
            module.find_definition_at_location(
                parsed.begin_line("y_var1"),
                parsed.begin_column("y_var1")
            )
        );

        assert_eq!(
            Definition::from(IdentifierDefinition::Location {
                source: parsed.resolved_span("y_var2"),
                destination: parsed.resolved_span("y1"),
                name: "y".to_owned(),
            }),
            module.find_definition_at_location(
                parsed.begin_line("y_var2"),
                parsed.begin_column("y_var2")
            )
        );
        assert_eq!(
            Definition::from(IdentifierDefinition::Unresolved {
                source: parsed.resolved_span("z_var"),
                name: "z".to_owned()
            }),
            module.find_definition_at_location(
                parsed.begin_line("z_var"),
                parsed.begin_column("z_var")
            )
        );
        Ok(())
    }

    #[test]
    fn find_definition_unknown_clicks() -> starlark::Result<()> {
        let contents = dedent(
            r#"
        load("bar.star", "print");

        x = 1
        y = 2

        def <no_def>add</no_def>(y: int):
            return x + y

        def invalid_symbol():
            return y + z

        print(x)
        add(3)
        invalid_symbol()
        "#,
        )
        .trim()
        .to_owned();
        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        assert_eq!(
            Definition::from(IdentifierDefinition::NotFound),
            module.find_definition_at_location(
                parsed.begin_line("no_def"),
                parsed.begin_column("no_def")
            )
        );

        Ok(())
    }

    #[test]
    fn finds_definition_in_strings() -> starlark::Result<()> {
        let contents = dedent(
            r#"
        <foo1_click>"f<foo1>o</foo1>o1"</foo1_click>
        [
            <foo2_click>"f<foo2>o</foo2>o2"</foo2_click>,
            "ignored"
        ]
        {
            <foo3_click>"f<foo3>o</foo3>o3"</foo3_click>: "ignored",
            "ignored": <foo4_click>"f<foo4>o</foo4>o4"</foo4_click>,
            "ignored_other": "ignored",
        }

        def f(x = <foo5_click>"f<foo5>o</foo5>o5"</foo5_click>):
            <foo6_click>"f<foo6>o</foo6>o6"</foo6_click>
            [
                <foo7_click>"f<foo7>o</foo7>o7"</foo7_click>,
                "ignored"
            ]
            {
                <foo8_click>"f<foo8>o</foo8>o8"</foo8_click>: "ignored",
                "ignored": <foo9_click>"f<foo9>o</foo9>o9"</foo9_click>,
                "ignored_other": "ignored",
            }
            if x == <foo10_click>"f<foo10>o</foo10>o10"</foo10_click>:
                <foo11_click>"f<foo11>o</foo11>o11"</foo11_click>
                [
                    <foo12_click>"f<foo12>o</foo12>o12"</foo12_click>,
                    "ignored"
                ]
                {
                    <foo13_click>"f<foo13>o</foo13>o13"</foo13_click>: "ignored",
                    "ignored": <foo14_click>"f<foo14>o</foo14>o14"</foo14_click>,
                    "ignored_other": "ignored",
                }
            return <foo15_click>"f<foo15>o</foo15>o15"</foo15_click>

        foo16 = <foo16_click>"f<foo16>o</foo16>o16"</foo16_click>
        "#,
        )
        .trim()
        .to_owned();

        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        fn test(parsed: &FixtureWithRanges, module: &LspModule, name: &str) {
            let expected = Definition::from(IdentifierDefinition::StringLiteral {
                source: parsed.resolved_span(&format!("{name}_click")),
                literal: name.to_owned(),
            });
            let actual = module
                .find_definition_at_location(parsed.begin_line(name), parsed.begin_column(name));

            assert_eq!(
                expected, actual,
                "Expected `{expected:?}` == `{actual:?}` for test `{name}`"
            );
        }

        test(&parsed, &module, "foo1");
        test(&parsed, &module, "foo2");
        test(&parsed, &module, "foo3");
        test(&parsed, &module, "foo4");
        test(&parsed, &module, "foo5");
        test(&parsed, &module, "foo6");
        test(&parsed, &module, "foo7");
        test(&parsed, &module, "foo8");
        test(&parsed, &module, "foo9");
        test(&parsed, &module, "foo10");
        test(&parsed, &module, "foo11");
        test(&parsed, &module, "foo12");
        test(&parsed, &module, "foo13");
        test(&parsed, &module, "foo14");
        test(&parsed, &module, "foo15");
        test(&parsed, &module, "foo16");

        Ok(())
    }

    #[test]
    fn find_definition_dot_access_unresolved_root() -> starlark::Result<()> {
        let contents = dedent(
            r#"
            <foo_root><foo>f<foo_click>o</foo_click>o</foo></foo_root>.bar.baz().quz
            <bar_root>foo</bar_root>.<bar>b<bar_click>a</bar_click>r</bar>.baz().quz
            <baz_root>foo</baz_root>.bar.<baz>b<baz_click>a</baz_click>z</baz>().quz
            <quz_root>foo</quz_root>.bar.baz().<quz>q<quz_click>u</quz_click>z</quz>
            "#,
        )
        .trim()
        .to_owned();

        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        let expected = |span_id: &str, segments: &[&str]| -> Definition {
            let root_definition_location = IdentifierDefinition::Unresolved {
                source: parsed.resolved_span(&format!("{span_id}_root")),
                name: "foo".to_owned(),
            };
            if segments.len() > 1 {
                DottedDefinition {
                    source: parsed.resolved_span(span_id),
                    root_definition_location,
                    segments: segments.iter().map(|s| (*s).to_owned()).collect(),
                }
                .into()
            } else {
                root_definition_location.into()
            }
        };

        let find_definition = |span_id: &str| {
            module.find_definition_at_location(
                parsed.begin_line(span_id),
                parsed.begin_column(span_id),
            )
        };

        let expected_foo = expected("foo", &["foo"]);
        let expected_bar = expected("bar", &["foo", "bar"]);
        let expected_baz = expected("baz", &["foo", "bar", "baz"]);
        let expected_quz = expected("quz", &["foo", "bar", "baz", "quz"]);

        assert_eq!(expected_foo, find_definition("foo_click"));
        assert_eq!(expected_bar, find_definition("bar_click"));
        assert_eq!(expected_baz, find_definition("baz_click"));
        assert_eq!(expected_quz, find_definition("quz_click"));

        Ok(())
    }

    #[test]
    fn find_definition_dot_access_loaded_root() -> starlark::Result<()> {
        let contents = dedent(
            r#"
            load("defs.bzl", <root>"foo"</root>);

            <foo_root><foo>f<foo_click>o</foo_click>o</foo></foo_root>.bar.baz().quz
            <bar_root>foo</bar_root>.<bar>b<bar_click>a</bar_click>r</bar>.baz().quz
            <baz_root>foo</baz_root>.bar.<baz>b<baz_click>a</baz_click>z</baz>().quz
            <quz_root>foo</quz_root>.bar.baz().<quz>q<quz_click>u</quz_click>z</quz>
            "#,
        )
        .trim()
        .to_owned();

        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        let expected = |span_id: &str, segments: &[&str]| -> Definition {
            let root_definition_location = IdentifierDefinition::LoadedLocation {
                source: parsed.resolved_span(&format!("{span_id}_root")),
                destination: parsed.resolved_span("root"),
                path: "defs.bzl".to_owned(),
                name: "foo".to_owned(),
            };
            if segments.len() > 1 {
                DottedDefinition {
                    source: parsed.resolved_span(span_id),
                    root_definition_location,
                    segments: segments.iter().map(|s| (*s).to_owned()).collect(),
                }
                .into()
            } else {
                root_definition_location.into()
            }
        };

        let find_definition = |span_id: &str| {
            module.find_definition_at_location(
                parsed.begin_line(span_id),
                parsed.begin_column(span_id),
            )
        };

        let expected_foo = expected("foo", &["foo"]);
        let expected_bar = expected("bar", &["foo", "bar"]);
        let expected_baz = expected("baz", &["foo", "bar", "baz"]);
        let expected_quz = expected("quz", &["foo", "bar", "baz", "quz"]);

        assert_eq!(expected_foo, find_definition("foo_click"));
        assert_eq!(expected_bar, find_definition("bar_click"));
        assert_eq!(expected_baz, find_definition("baz_click"));
        assert_eq!(expected_quz, find_definition("quz_click"));

        Ok(())
    }

    #[test]
    fn find_definition_dot_access_local_root() -> starlark::Result<()> {
        let contents = dedent(
            r#"
            def func_1(<root>foo</root>):
                <foo_root><foo>f<foo_click>o</foo_click>o</foo></foo_root>.bar.baz().quz
                <bar_root>foo</bar_root>.<bar>b<bar_click>a</bar_click>r</bar>.baz().quz
                <baz_root>foo</baz_root>.bar.<baz>b<baz_click>a</baz_click>z</baz>().quz
                <quz_root>foo</quz_root>.bar.baz().<quz>q<quz_click>u</quz_click>z</quz>
            "#,
        )
        .trim()
        .to_owned();

        let parsed = FixtureWithRanges::from_fixture("foo.star", &contents)?;
        let module = parsed.module()?;

        let expected = |span_id: &str, segments: &[&str]| -> Definition {
            let root_definition_location = IdentifierDefinition::Location {
                source: parsed.resolved_span(&format!("{span_id}_root")),
                destination: parsed.resolved_span("root"),
                name: "foo".to_owned(),
            };
            if segments.len() > 1 {
                DottedDefinition {
                    source: parsed.resolved_span(span_id),
                    root_definition_location,
                    segments: segments.iter().map(|s| (*s).to_owned()).collect(),
                }
                .into()
            } else {
                root_definition_location.into()
            }
        };

        let find_definition = |span_id: &str| {
            module.find_definition_at_location(
                parsed.begin_line(span_id),
                parsed.begin_column(span_id),
            )
        };

        let expected_foo = expected("foo", &["foo"]);
        let expected_bar = expected("bar", &["foo", "bar"]);
        let expected_baz = expected("baz", &["foo", "bar", "baz"]);
        let expected_quz = expected("quz", &["foo", "bar", "baz", "quz"]);

        assert_eq!(expected_foo, find_definition("foo_click"));
        assert_eq!(expected_bar, find_definition("bar_click"));
        assert_eq!(expected_baz, find_definition("baz_click"));
        assert_eq!(expected_quz, find_definition("quz_click"));

        Ok(())
    }
}
