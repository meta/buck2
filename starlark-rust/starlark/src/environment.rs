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

//! Types representing Starlark modules ([`Module`] and [`FrozenModule`]) and global variables ([`Globals`]).
//!
//! Global functions and values are stored in [`Globals`], which are typically
//! built using [`GlobalsBuilder`].
//! User executions store their values in a [`Module`], which have to be converted to a
//! [`FrozenModule`] using [`freeze_named`](Module::freeze_named) before they can be `load()`'d as
//! a dependency.
//!
//! # Environments
//!
//! A Starlark program has one environment per scope:
//!
//! ```python
//! x = []
//! def foo():
//!     y = True
//!     def bar():
//!         z = 1
//!         list.append(x, 1)
//! ```
//!
//! * The global environment defines `len`, `list` and the other builtins (`list.append` is a
//!   method of the `list` type, reached through the value `list`, not a name of its own). It is a
//!   [`Globals`]: always frozen, and shared by every module evaluated with it.
//! * The module environment defines `x`. It is the [`Module`] the program is evaluated in.
//! * The environment of `foo` defines `y`; that of `bar` defines `z`.
//!
//! A scope can *access* the variables of the scopes around it, and often *mutate* the values
//! they hold (`list.append(x, 1)`, equivalently `x.append(1)`), but it cannot *assign* to them:
//! `x = 1` inside `bar` defines a local `x` that shadows the module's. That holds even if the
//! assignment comes after the use, or sits in a branch that never runs:
//!
//! ```python
//! x = 1
//! def f():
//!     print(x)
//!     if False:
//!         x = 2
//! ```
//!
//! Calling `f()` fails with `x` referenced before assignment. Mutation ends when the module is
//! frozen: another module can only `load()` a frozen module, and a `bar` called from there fails
//! to append to `x`.
//!
//! A comprehension is a scope of its own. `[x for x in [1, 2, 3]]` defines an `x` that is bound
//! at once, shadows any other `x`, and is gone after the comprehension; it never becomes a
//! module variable, even at the top level of a module.
//!
//! # Slots
//!
//! The compiler resolves every identifier before the program runs, so that at run time
//! variables are reached by index rather than by name:
//!
//! * A global is resolved to its value. The compiled code refers to the value in the [`Globals`]
//!   directly, and the module's frozen heap keeps the globals' heap alive.
//! * A module variable is resolved to a module slot. The [`Module`] maps names to slot indices
//!   and holds a value per slot; the mapping exists at run time because `load()`,
//!   [`Module::set`] and [`Module::import_public_symbols`] add names before and during
//!   evaluation, and because a [`FrozenModule`] is looked up by name when it is loaded.
//! * A local variable, comprehension variables included, is resolved to a slot in the frame of
//!   the function (or of the module's top level) it belongs to. Nothing can add a local at run
//!   time, so the names are kept for diagnostics only. A variable that a nested function
//!   captures is held in a cell that both frames point at.
//!
//! A slot that has not been assigned is empty, which is how a use before assignment is caught.

mod globals;
mod methods;
mod module_dump;
mod modules;
pub(crate) mod names;
pub(crate) mod slots;

pub use globals::*;
pub use methods::*;
pub use modules::*;
use thiserror::Error;

#[derive(Debug, Error)]
enum EnvironmentError {
    /// Cannot import private symbol, i.e. underscore prefixed
    #[error("Cannot import private symbol `{0}`")]
    CannotImportPrivateSymbol(String),
    #[error("Module has no symbol `{0}`")]
    ModuleHasNoSymbol(String),
    #[error("Module has no symbol `{0}`, did you mean `{1}`?")]
    ModuleHasNoSymbolDidYouMean(String, String),
    #[error("Module symbol `{0}` is not exported")]
    ModuleSymbolIsNotExported(String),
}
