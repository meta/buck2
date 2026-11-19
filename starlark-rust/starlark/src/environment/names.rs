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

use std::cell::RefCell;

use allocative::Allocative;
use starlark_derive::StarlarkPagable;
use starlark_syntax::syntax::ast::Visibility;

use crate as starlark;
use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::environment::slots::ModuleSlotId;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::ProvidesStaticType;
use crate::values::StringValue;

/// The module's mapping from variable name to slot index, see the `environment` module doc.
///
/// A statement can define a variable (`x = ...`, `for x in ...`) after an expression that uses
/// it, and whether that expression means a global `x` or a not-yet-assigned module `x` depends on
/// the definition being there. So the compiler collects the definitions of a scope first
/// (`ModuleScopeBuilder` in `eval/compiler/scope.rs`), allocates them slots, and only then
/// replaces variables with slot numbers. Comprehension variables never get a module slot: they
/// are locals of the module's top-level frame, scoped by `ScopeNames::add_scoped` and `unscope`.
///
/// The names are strings at the module's brand: interned in the module's frozen heap, or in a
/// heap it references for names imported from another module.
#[derive(Debug)]
pub(crate) struct MutableNames<'v>(RefCell<SmallMap<StringValue<'v>, (ModuleSlotId, Visibility)>>);

/// The names of a frozen module, at the brand of the heap that holds them.
#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
pub(crate) struct FrozenNames<'v>(SmallMap<StringValue<'v>, (ModuleSlotId, Visibility)>);

impl<'v> MutableNames<'v> {
    pub(crate) fn new() -> Self {
        Self(RefCell::new(SmallMap::new()))
    }

    pub(crate) fn slot_count(&self) -> u32 {
        self.0.borrow().len().try_into().unwrap()
    }

    /// Try and go back from a slot to a name.
    /// Inefficient - only use in error paths.
    pub(crate) fn get_slot(&self, slot: ModuleSlotId) -> Option<StringValue<'v>> {
        for (s, (i, _vis)) in &*self.0.borrow() {
            if *i == slot {
                return Some(*s);
            }
        }
        None
    }

    pub(crate) fn get_name(&self, name: Hashed<&str>) -> Option<(ModuleSlotId, Visibility)> {
        self.0.borrow().get_hashed(name).copied()
    }

    /// Add a name with explicit visibility to the module.
    pub(crate) fn add_name_visibility(
        &self,
        name: StringValue<'v>,
        vis: Visibility,
    ) -> ModuleSlotId {
        let mut x = self.0.borrow_mut();
        match x.get_mut_hashed(name.get_hashed().as_ref()) {
            Some((slot, stored_vis)) => {
                // Public visibility wins.
                if *stored_vis == Visibility::Private {
                    *stored_vis = vis;
                }
                *slot
            }
            None => {
                let slot = ModuleSlotId::new(x.len().try_into().unwrap());
                x.insert_hashed(name.get_hashed(), (slot, vis));
                slot
            }
        }
    }

    // Add an exported name, or if it's already there, return the existing name
    pub(crate) fn add_name(&self, name: StringValue<'v>) -> ModuleSlotId {
        self.add_name_visibility(name, Visibility::Public)
    }

    pub(crate) fn hide_name(&self, name: &str) {
        self.0.borrow_mut().shift_remove(name);
    }

    pub(crate) fn all_names_and_slots(&self) -> Vec<(StringValue<'v>, ModuleSlotId)> {
        self.0
            .borrow()
            .iter()
            .map(|(name, (slot, _vis))| (*name, *slot))
            .collect()
    }

    pub(crate) fn all_names_and_visibilities(&self) -> Vec<(StringValue<'v>, Visibility)> {
        self.0
            .borrow()
            .iter()
            .map(|(name, (_slot, vis))| (*name, *vis))
            .collect()
    }

    pub(crate) fn all_names_slots_and_visibilities(
        &self,
    ) -> Vec<(StringValue<'v>, ModuleSlotId, Visibility)> {
        self.0
            .borrow()
            .iter()
            .map(|(name, (slot, vis))| (*name, *slot, *vis))
            .collect()
    }

    pub(crate) fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<FrozenNames<'fv>> {
        let names = self.0.into_inner();
        let mut frozen = SmallMap::with_capacity(names.len());
        for (name, slot) in names.into_iter_hashed() {
            let hash = name.hash();
            let name = name.into_key().freeze(freezer)?;
            // Freezing keeps a string's hash.
            frozen.insert_hashed_unique_unchecked(Hashed::new_unchecked(hash, name), slot);
        }
        Ok(FrozenNames(frozen))
    }
}

impl<'v> FrozenNames<'v> {
    pub(crate) fn get_name(&self, name: &str) -> Option<(ModuleSlotId, Visibility)> {
        self.0.get(name).copied()
    }

    /// Symbols including private.
    pub(crate) fn all_symbols(&self) -> impl Iterator<Item = (StringValue<'v>, ModuleSlotId)> + '_ {
        self.0.iter().map(|(name, (slot, _vis))| (*name, *slot))
    }

    /// Exported symbols.
    pub(crate) fn symbols(&self) -> impl Iterator<Item = (StringValue<'v>, ModuleSlotId)> + '_ {
        self.0.iter().filter_map(|(name, (slot, vis))| match vis {
            Visibility::Private => None,
            Visibility::Public => Some((*name, *slot)),
        })
    }
}
