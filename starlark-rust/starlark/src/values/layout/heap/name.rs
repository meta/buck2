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

//! Names of frozen heaps, for heap graph tracking and paging.

use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;

use strong_hash::StrongHash;

use crate::environment::GlobalFrozenHeapName;
use crate::environment::MethodFrozenHeapName;

/// Object-safe trait for user-defined heap names that supports hashing and downcasting.
///
/// Automatically implemented for any type that is `StrongHash + Any + Send + Sync + Debug`.
/// `StrongHash` is required (rather than `Hash`) because heap identities are
/// derived from this and must be deterministic across processes.
#[pagable::pagable_typetag]
pub trait UserHeapName:
    std::fmt::Display + pagable::typetag::PagableTagged + Any + Send + Sync + Debug + 'static
{
    /// Strong-hash this value through a trait object.
    fn dyn_strong_hash(&self, state: &mut dyn Hasher);
    /// Downcast support.
    fn as_any(&self) -> &dyn Any;
    /// Clone this name through a trait object.
    fn clone_name(&self) -> Box<dyn UserHeapName>;
}

impl<
    T: std::fmt::Display
        + pagable::typetag::PagableTagged
        + Clone
        + StrongHash
        + Any
        + Send
        + Sync
        + Debug
        + 'static,
> UserHeapName for T
{
    fn dyn_strong_hash(&self, mut state: &mut dyn Hasher) {
        self.strong_hash(&mut state);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_name(&self) -> Box<dyn UserHeapName> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn UserHeapName> {
    fn clone(&self) -> Box<dyn UserHeapName> {
        UserHeapName::clone_name(self.as_ref())
    }
}

/// Name/identifier for a frozen heap, used for heap graph tracking and metrics.
#[derive(Clone, derive_more::Display, Debug, pagable::Pagable)]
pub enum FrozenHeapName {
    /// For starlark Methods heaps.
    Method(MethodFrozenHeapName),
    /// For the global starlark environment heap.
    Global(GlobalFrozenHeapName),
    /// For starlark singleton heaps
    Singleton(SingletonFrozenHeapName),
    /// For user/downstream code.
    User(Box<dyn UserHeapName>),
}

impl FrozenHeapName {
    /// Create a user heap name backed by an owned string.
    pub fn user(name: impl Into<String>) -> Self {
        Self::User(Box::new(StringUserHeapName(name.into())))
    }
}

impl StrongHash for FrozenHeapName {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        // Inner Method/Global/Singleton variants implement `Hash` (deterministic
        // here because we control the `Hasher`); the User variant goes through
        // the `StrongHash` trait object.
        std::mem::discriminant(self).hash(state);
        match self {
            FrozenHeapName::Method(m) => m.hash(state),
            FrozenHeapName::Global(g) => g.hash(state),
            FrozenHeapName::Singleton(s) => s.hash(state),
            FrozenHeapName::User(b) => b.dyn_strong_hash(state),
        }
    }
}

/// Testing sentinel for starlark crate's own tests.
/// Used as `FrozenHeapName::User(Box::new(StarlarkTestHeapName))`.
#[derive(Debug, StrongHash, Hash, Clone, derive_more::Display, pagable::Pagable)]
#[pagable::pagable_typetag(UserHeapName)]
#[display("StarlarkTestHeapName")]
pub(crate) struct StarlarkTestHeapName;

impl StarlarkTestHeapName {
    pub(crate) fn frozen_heap_name() -> FrozenHeapName {
        FrozenHeapName::User(Box::new(Self))
    }
}

/// Owned-string user heap name for callers without a dedicated name type.
#[derive(Debug, StrongHash, Hash, Clone, derive_more::Display, pagable::Pagable)]
#[pagable::pagable_typetag(UserHeapName)]
#[display("{}", _0)]
pub struct StringUserHeapName(String);

/// A frozen heap name derived from source location, for singleton heaps.
///
/// This type can only be created via the [`singleton_heap_name!`](crate::singleton_heap_name)
/// macro, which captures `file!()`, `line!()`, and `column!()` at the call site.
/// This ensures each name is unique and stable across process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, pagable::Pagable)]
pub struct SingletonFrozenHeapName {
    file: pagable::StaticStr,
    line: u32,
    col: u32,
}

impl SingletonFrozenHeapName {
    /// Internal constructor. Do not call directly; use [`singleton_heap_name!`](crate::singleton_heap_name).
    #[doc(hidden)]
    pub const fn _new(file: pagable::StaticStr, line: u32, col: u32) -> Self {
        Self { file, line, col }
    }
}

impl std::fmt::Display for SingletonFrozenHeapName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

/// Create a [`SingletonFrozenHeapName`] capturing the current source location.
///
/// Each call site produces a unique, stable name based on `file!()`, `line!()`, `column!()`.
///
/// ```
/// use starlark::singleton_heap_name;
/// let name = singleton_heap_name!();
/// ```
#[macro_export]
macro_rules! singleton_heap_name {
    () => {{
        $crate::__derive_refs::static_str!(__SINGLETON_HEAP_FILE = file!());
        $crate::values::SingletonFrozenHeapName::_new(__SINGLETON_HEAP_FILE, line!(), column!())
    }};
}
