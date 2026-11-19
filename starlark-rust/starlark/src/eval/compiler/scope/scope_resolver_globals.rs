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

use crate::const_frozen_string;
use crate::environment::GlobalsData;
use crate::values::HeapEdge;
use crate::values::Value;

/// The globals the compiler resolves identifiers against, at their own brand `'g`, with the edge
/// that brings their values to the frozen heap `'f` the compiler allocates on.
pub(crate) struct ScopeResolverGlobals<'a, 'f, 'g> {
    /// None if unknown.
    pub(crate) globals: Option<(&'a GlobalsData<'g>, HeapEdge<'f, 'g>)>,
}

impl<'a, 'f, 'g> ScopeResolverGlobals<'a, 'f, 'g> {
    pub(crate) fn unknown() -> Self {
        ScopeResolverGlobals { globals: None }
    }

    pub(crate) fn get_global(&self, name: &str) -> Option<Value<'f>> {
        match self.globals {
            Some((globals, edge)) => Some(edge.rebrand(globals.variables.get_str(name)?.value)),
            None => Some(const_frozen_string!("unknown-global").at().to_value()),
        }
    }

    /// The global names, sorted, for error messages.
    pub(crate) fn names(&self) -> Option<Vec<String>> {
        self.globals.map(|(globals, _)| {
            let mut names: Vec<String> = globals
                .variables
                .keys()
                .map(|s| s.as_str().to_owned())
                .collect();
            names.sort();
            names
        })
    }
}
