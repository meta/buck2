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

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::iter;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr as _;

use itertools::Either;
use lsp_types::Uri;
use starlark::StarlarkResultExt;
use starlark::analysis::AstModuleLint;
use starlark::docs::DocModule;
use starlark::environment::FrozenModule;
use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::errors::EvalMessage;
use starlark::eval::Evaluator;
use starlark::eval::FileLoader;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;
use starlark::values::FrozenHeapName;
use starlark_lsp::error::eval_message_to_lsp_diagnostic;
use starlark_lsp::server::LspContext;
use starlark_lsp::server::LspEvalResult;
use starlark_lsp::server::LspUri;
use starlark_lsp::server::StringLiteralResult;
use tower_lsp_server::UriExt as _;

use crate::suppression::GlobLintSuppression;

#[derive(Debug)]
pub(crate) enum ContextMode {
    Check,
    Run,
}

#[derive(Debug, thiserror::Error)]
enum ContextError {
    /// The provided URI was not absolute and it needs to be.
    #[error("Path for URI `{}` was not absolute", .0)]
    NotAbsolute(LspUri),
    /// The scheme provided was not correct or supported.
    #[error("URI `{}` was expected to be of type `{}`", .1, .0)]
    WrongScheme(String, LspUri),
}

#[derive(Debug)]
pub(crate) struct Context<'v> {
    pub(crate) mode: ContextMode,
    pub(crate) print_non_none: bool,
    pub(crate) prelude: Vec<FrozenModule>,
    pub(crate) module: Option<Module<'v>>,
    pub(crate) dialect: Dialect,
    pub(crate) globals: Globals,
    pub(crate) builtin_docs: HashMap<LspUri, String>,
    pub(crate) builtin_symbols: HashMap<String, LspUri>,
    pub(crate) suppression_rules: Vec<GlobLintSuppression>,
}

impl<'v> FileLoader for Context<'v> {
    fn load(&self, path: &str) -> starlark::Result<FrozenModule> {
        self.load_path(Path::new(path))
    }
}

/// The outcome of evaluating (checking, parsing or running) given starlark code.
pub(crate) struct EvalResult<T: Iterator<Item = EvalMessage>> {
    /// The diagnostic and error messages from evaluating a given piece of starlark code.
    pub messages: T,
    /// If the code is only parsed, not run, and there were no errors, this will contain
    /// the parsed module. Otherwise, it will be `None`
    pub ast: Option<AstModule>,
}

/// Errors when [`LspContext::resolve_load()`] cannot resolve a given path.
#[derive(thiserror::Error, Debug)]
enum ResolveLoadError {
    /// Attempted to resolve a relative path, but no current_file_path was provided,
    /// so it is not known what to resolve the path against.
    #[error("Relative path `{}` provided, but current_file_path could not be determined", .0.display())]
    MissingCurrentFilePath(PathBuf),
    /// The scheme provided was not correct or supported.
    #[error("URI `{}` was expected to be of type `{}`", .1, .0)]
    WrongScheme(String, LspUri),
}

impl<'v> Context<'v> {
    pub(crate) fn new(
        mode: ContextMode,
        print_non_none: bool,
        prelude: &[PathBuf],
        module: Option<Module<'v>>,
        dialect: Dialect,
        globals: Globals,
        suppression_rules: Vec<GlobLintSuppression>,
    ) -> anyhow::Result<Self> {
        let mut builtin_docs: HashMap<LspUri, String> = HashMap::new();
        let mut builtin_symbols: HashMap<String, LspUri> = HashMap::new();
        for (name, item) in globals.documentation().members {
            let uri = Uri::from_str(&format!("starlark:/{name}.bzl"))?;
            let uri = LspUri::try_from(uri)?;
            builtin_docs.insert(uri.clone(), item.render_as_code(&name));
            builtin_symbols.insert(name, uri);
        }

        let mut ctx = Self {
            mode,
            print_non_none,
            prelude: Vec::new(),
            module,
            dialect,
            globals,
            builtin_docs,
            builtin_symbols,
            suppression_rules,
        };

        ctx.prelude = prelude
            .iter()
            .map(|x| ctx.load_path(x))
            .collect::<starlark::Result<_>>()
            .into_anyhow_result()?;

        if let Some(module) = ctx.module.as_ref() {
            for p in &ctx.prelude {
                module.import_public_symbols(p);
            }
        }

        Ok(ctx)
    }

    fn load_path(&self, path: &Path) -> starlark::Result<FrozenModule> {
        Module::with_temp_heap(|env| {
            {
                let mut eval = Evaluator::new(&env);
                eval.set_loader(self);
                let module = AstModule::parse_file(path, &self.dialect).into_anyhow_result()?;
                eval.eval_module(module, &self.globals)?;
            }
            Ok::<_, starlark::Error>(
                env.freeze_named(FrozenHeapName::user(path.to_string_lossy()))?,
            )
        })
    }

    fn go(
        &self,
        file: &str,
        ast: AstModule,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<>> {
        let mut warnings = Either::Left(iter::empty());
        let mut errors = Either::Left(iter::empty());
        let final_ast = match self.mode {
            ContextMode::Check => {
                warnings = Either::Right(self.check(file, &ast));
                Some(ast)
            }
            ContextMode::Run => {
                errors = Either::Right(self.run(file, ast).messages);
                None
            }
        };
        EvalResult {
            messages: warnings.chain(errors),
            ast: final_ast,
        }
    }

    // Convert a result over iterator of EvalMessage, into an iterator of EvalMessage
    fn err<T: Iterator<Item = EvalMessage>>(
        file: &str,
        result: starlark::Result<EvalResult<T>>,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<T>> {
        match result {
            Err(e) => EvalResult {
                messages: Either::Left(iter::once(EvalMessage::from_error(Path::new(file), &e))),
                ast: None,
            },
            Ok(res) => EvalResult {
                messages: Either::Right(res.messages),
                ast: res.ast,
            },
        }
    }

    pub(crate) fn expression(
        &self,
        content: String,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<>> {
        let file = "expression";
        Self::err(
            file,
            AstModule::parse(file, content, &self.dialect).map(|module| self.go(file, module)),
        )
    }

    pub(crate) fn file(
        &self,
        file: &Path,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<>> {
        let filename = &file.to_string_lossy();
        Self::err(
            filename,
            fs::read_to_string(file)
                .map(|content| self.file_with_contents(filename, content))
                .map_err(|e| anyhow::Error::from(e).into()),
        )
    }

    pub(crate) fn file_with_contents(
        &self,
        filename: &str,
        content: String,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<>> {
        Self::err(
            filename,
            AstModule::parse(filename, content, &self.dialect)
                .map(|module| self.go(filename, module)),
        )
    }

    fn run(
        &self,
        file: &str,
        ast: AstModule,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<>> {
        match self.module.as_ref() {
            Some(module) => self.run_with_module(file, ast, module),
            None => Module::with_temp_heap(|module| {
                for p in &self.prelude {
                    module.import_public_symbols(p);
                }
                self.run_with_module(file, ast, &module)
            }),
        }
    }

    fn run_with_module(
        &self,
        file: &str,
        ast: AstModule,
        module: &Module,
    ) -> EvalResult<impl Iterator<Item = EvalMessage> + use<>> {
        let mut eval = Evaluator::new(module);
        eval.set_loader(self);
        eval.enable_terminal_breakpoint_console();
        Self::err(
            file,
            eval.eval_module(ast, &self.globals).map(|v| {
                if self.print_non_none && !v.is_none() {
                    println!("{v}");
                }
                EvalResult {
                    messages: iter::empty(),
                    ast: None,
                }
            }),
        )
    }

    fn is_suppressed(&self, file: &str, issue: &str) -> bool {
        self.suppression_rules
            .iter()
            .any(|rule| rule.is_suppressed(file, issue))
    }

    fn check(&self, file: &str, module: &AstModule) -> impl Iterator<Item = EvalMessage> + use<> {
        let globals = if self.prelude.is_empty() {
            None
        } else {
            let mut globals = HashSet::new();
            for modu in &self.prelude {
                for name in modu.names() {
                    globals.insert(name.to_owned());
                }
            }

            for global_symbol in self.builtin_symbols.keys() {
                globals.insert(global_symbol.to_owned());
            }

            Some(globals)
        };

        let mut lints = module.lint(globals.as_ref());
        lints.retain(|issue| !self.is_suppressed(file, &issue.short_name));
        lints.into_iter().map(EvalMessage::from)
    }
}

impl<'v> LspContext for Context<'v> {
    fn parse_file_with_contents(&self, uri: &LspUri, content: String) -> LspEvalResult {
        match uri {
            LspUri::File(uri) => {
                let EvalResult { messages, ast } =
                    self.file_with_contents(&uri.to_string_lossy(), content);
                LspEvalResult {
                    diagnostics: messages.map(eval_message_to_lsp_diagnostic).collect(),
                    ast,
                }
            }
            _ => LspEvalResult::default(),
        }
    }

    fn resolve_load(
        &self,
        path: &str,
        current_file: &LspUri,
        _workspace_root: Option<&Path>,
    ) -> Result<LspUri, String> {
        let path = PathBuf::from(path);
        match current_file {
            LspUri::File(current_file_path) => {
                let current_file_dir = current_file_path.parent();
                let absolute_path = match (current_file_dir, path.is_absolute()) {
                    (_, true) => Ok(path),
                    (Some(current_file_dir), false) => Ok(current_file_dir.join(&path)),
                    (None, false) => {
                        Err(ResolveLoadError::MissingCurrentFilePath(path).to_string())
                    }
                }?;
                LspUri::try_from(Uri::from_file_path(absolute_path).unwrap())
                    .map_err(|e| e.to_string())
            }
            _ => Err(
                ResolveLoadError::WrongScheme("file://".to_owned(), current_file.clone())
                    .to_string(),
            ),
        }
    }

    fn resolve_string_literal(
        &self,
        literal: &str,
        current_file: &LspUri,
        workspace_root: Option<&Path>,
    ) -> Result<Option<StringLiteralResult>, String> {
        self.resolve_load(literal, current_file, workspace_root)
            .map(|uri| {
                Some(StringLiteralResult {
                    uri,
                    location_finder: None,
                })
            })
    }

    fn get_load_contents(&self, uri: &LspUri) -> Result<Option<String>, String> {
        match uri {
            LspUri::File(path) => match path.is_absolute() {
                true => match fs::read_to_string(path) {
                    Ok(contents) => Ok(Some(contents)),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e.to_string()),
                },
                false => Err(ContextError::NotAbsolute(uri.clone()).to_string()),
            },
            LspUri::Starlark(_) => Ok(self.builtin_docs.get(uri).cloned()),
            _ => Err(ContextError::WrongScheme("file://".to_owned(), uri.clone()).to_string()),
        }
    }

    fn get_uri_for_global_symbol(
        &self,
        _current_file: &LspUri,
        symbol: &str,
    ) -> Result<Option<LspUri>, String> {
        Ok(self.builtin_symbols.get(symbol).cloned())
    }

    fn render_as_load(
        &self,
        _target: &LspUri,
        _current_file: &LspUri,
        _workspace_root: Option<&Path>,
    ) -> Result<String, String> {
        Err("Not yet implemented, render_as_load".to_owned())
    }

    fn get_environment(&self, _uri: &LspUri) -> DocModule {
        DocModule::default()
    }
}
