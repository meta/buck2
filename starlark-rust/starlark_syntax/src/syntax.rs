/*
 * Copyright 2018 The Starlark in Rust Authors.
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

//! The AST of Starlark as [`AstModule`], along with a [`parse`](AstModule::parse) function.

pub use ast_load::AstLoad;
pub use module::AstModule;

pub use crate::dialect::Dialect;
pub use crate::dialect::DialectTypes;

pub mod ast;
pub mod ast_load;
pub mod call;
pub mod def;
#[cfg(test)]
mod grammar_tests;
pub mod grammar_util;
mod lint_suppressions;
pub mod module;
pub mod payload_map;
pub(crate) mod state;
// Far too much input to parse interpreted.
#[cfg(all(test, not(miri)))]
mod testcases;
pub mod top_level_stmts;
pub mod type_expr;
pub mod uniplate;
pub mod validate;

pub(crate) mod parser_rd;
