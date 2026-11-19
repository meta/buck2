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

use std::borrow::Borrow;
use std::hash::Hash;
use std::ops::Deref;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use pagable::StaticStr;

use crate::util::arc_or_static::ArcOrStatic;

/// Wrapper for `Arc<str>`.
#[derive(
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Debug,
    derive_more::Display,
    Allocative,
    Pagable
)]
#[display("{}", &**self)]
pub struct ArcStr(ArcOrStatic<str>);

impl ArcStr {
    /// Create from a `&'static str`. Allocates an `Arc<str>` — for zero-copy
    /// round-trippable static strings, register via `pagable::static_str!`
    /// and use [`ArcStr::new_static_value`].
    pub fn new_static(s: StaticStr) -> ArcStr {
        // ArcStr(ArcOrStatic::new_arc(Arc::from(s)))
        ArcStr(ArcOrStatic::new_static(s))
    }

    /// Create from a pre-registered [`StaticStr`] — zero-copy and preserved
    /// across pagable round-trips by index.
    pub fn new_static_value(s: StaticStr) -> ArcStr {
        ArcStr(ArcOrStatic::new_static(s))
    }

    /// Get the `str`.
    pub fn as_str(&self) -> &str {
        self
    }
}

impl Deref for ArcStr {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.deref()
    }
}

impl Borrow<str> for ArcStr {
    fn borrow(&self) -> &str {
        self
    }
}

pagable::static_str!(EMPTY_STR = "");

impl<'a> From<&'a str> for ArcStr {
    fn from(s: &'a str) -> Self {
        if s.is_empty() {
            ArcStr(ArcOrStatic::new_static(EMPTY_STR))
        } else {
            ArcStr(ArcOrStatic::new_arc(Arc::from(s)))
        }
    }
}
