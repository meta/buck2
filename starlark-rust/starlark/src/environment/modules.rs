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

//! The environment, called "Module" in [this spec](
//! https://github.com/bazelbuild/starlark/blob/master/spec.md)
//! is the list of variable in the current scope. It can be frozen, after which
//! all values from this environment become immutable.

use std::cell::Cell;
use std::cell::RefCell;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use dupe::Dupe;
use itertools::Itertools;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use starlark_derive::StarlarkPagable;
use starlark_syntax::syntax::ast::Visibility;
use static_assertions::assert_eq_size;

use crate as starlark;
use crate::collections::Hashed;
use crate::docs::DocModule;
use crate::docs::DocString;
use crate::docs::DocStringKind;
use crate::environment::EnvironmentError;
use crate::environment::Globals;
use crate::environment::names::FrozenNames;
use crate::environment::names::MutableNames;
use crate::environment::slots::FrozenSlots;
use crate::environment::slots::ModuleSlotId;
use crate::environment::slots::MutableSlots;
use crate::errors::did_you_mean::did_you_mean;
use crate::eval::ProfileData;
use crate::eval::runtime::profile::heap::RetainedHeapProfileMode;
use crate::register_starlark_any_complex;
use crate::singleton_heap_name;
use crate::values::FreezeResult;
use crate::values::FrozenHeap;
use crate::values::FrozenValueTyped;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenRef;
use crate::values::ProvidesStaticType;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::any_complex::StarlarkAnyComplex;
use crate::values::layout::heap::arena::HeapKind;
use crate::values::layout::heap::module_heaps::ModuleHeaps;
use crate::values::layout::heap::name::FrozenHeapName;
use crate::values::layout::heap::profile::aggregated::AggregateHeapProfileInfo;
use crate::values::layout::heap::profile::aggregated::RetainedHeapProfile;

#[derive(Debug, thiserror::Error)]
enum ModuleError {
    #[error("Retained memory profiling is not enabled")]
    RetainedMemoryProfileNotEnabled,
    #[error("Extra value already set to a value of type `{}`", .0)]
    ExtraValueAlreadySet(&'static str),
}

/// The result of freezing a [`Module`], making it and its contained values immutable.
///
/// This type is an owned carrier: an [`OwnedFrozen`] of the module's data (its names and slots,
/// `FrozenModuleData`) allocated on the module's sealed frozen heap. Every accessor is a
/// projection of that owner. [`get`](FrozenModule::get) hands a value out as an
/// `OwnedFrozen`, [`get_option_ref`](FrozenModule::get_option_ref) as an [`OwnedFrozenRef`], and
/// [`frozen_heap`](FrozenModule::frozen_heap) is the bare heap. The type is `Clone` and
/// `Send + Sync`, so one frozen module is shared by every module that loads it.
///
/// `load()` looks the name up in the frozen module's names, adds this module's heap as a
/// reference of the loading module's frozen heap ([`OwnedFrozenRef::add_to_frozen_heap`]), and
/// hands the slot's value out at the loading module's brand, so the loaded module stays alive as
/// long as the loading module's values do. The `branding` module in
/// `values/layout/heap/branding.rs` explains the brands and the heap references behind this.
#[derive(Debug, Clone, Dupe, Allocative)]
pub struct FrozenModule {
    data: OwnedFrozenModuleData,
    /// Module evaluation duration:
    /// * evaluation of the top-level statements
    /// * optimizations during that evaluation
    /// * freezing and optimizations during freezing
    /// * does not include parsing time
    pub(crate) eval_duration: Duration,
}

/// What a [`FrozenModule`] owns: its [`FrozenModuleData`], allocated on the module's heap.
type OwnedFrozenModuleData =
    OwnedFrozen<ValueTyped<'static, StarlarkAnyComplex<FrozenModuleData<'static>>>>;

/// A [`FrozenModule`]'s data as the value it is allocated as, at the brand of the module's heap.
pub(crate) type FrozenModuleValue<'v> =
    FrozenValueTyped<'v, StarlarkAnyComplex<FrozenModuleData<'v>>>;

impl PagableSerialize for FrozenModule {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        // `eval_duration` is runtime telemetry, not content. Skipping it keeps the
        // page-out `DataKey` stable across runs; restored as `ZERO` on page-in.
        self.data.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for FrozenModule {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        Ok(Self {
            data: OwnedFrozenModuleData::pagable_deserialize(deserializer)?,
            eval_duration: Duration::ZERO,
        })
    }
}

/// The contents of a [`FrozenModule`], at the brand of the heap they live in.
#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
pub(crate) struct FrozenModuleData<'v> {
    pub(crate) names: FrozenNames<'v>,
    pub(crate) slots: FrozenSlots<'v>,
    extra_value: Option<Value<'v>>,
    docstring: Option<String>,
    /// When heap profile enabled, this field stores retained memory info.
    /// Runtime profiling data — not meaningful to round-trip, so we skip
    /// serialization and restore as `None`. Boxed to keep every frozen module
    /// small: this is `None` unless the retained heap profiler is on.
    #[starlark_pagable(skip)]
    heap_profile: Option<Box<RetainedHeapProfile>>,
}

assert_eq_size!(FrozenModuleData<'static>, [usize; 9]);

register_starlark_any_complex!(frozen FrozenModuleData<'_>);

/// The module environment a program is evaluated in: its variables, and the heaps its values
/// live on. The [module documentation](crate::environment) describes how variables are resolved.
///
/// A module owns two heaps. Values the program creates go on its [`Heap`]
/// ([`heap`](Module::heap)); the compiler's products (bytecode, constants, interned names) and
/// anything allocated through [`frozen_heap`](Module::frozen_heap) go on its frozen heap, and
/// freezing the module ([`freeze_named`](Module::freeze_named)) copies the variables onto that
/// heap too and seals it into the [`FrozenModule`].
///
/// The variables are a mapping from name to slot and a value per slot (`MutableNames` and
/// `MutableSlots`, both at `'v`). [`set`](Module::set) and
/// [`import_public_symbols`](Module::import_public_symbols) add names before or during
/// evaluation; [`get`](Module::get) looks one up.
#[derive(Debug)]
pub struct Module<'v> {
    heaps: ModuleHeaps<'v>,
    names: MutableNames<'v>,
    slots: MutableSlots<'v>,
    docstring: RefCell<Option<String>>,
    /// Module evaluation duration:
    /// * evaluation of the top-level statements
    /// * optimizations during that evaluation
    /// * does not include freezing time
    /// * does not include parsing time
    eval_duration: Cell<Duration>,
    /// Field that can be used for any purpose you want.
    extra_value: Cell<Option<Value<'v>>>,
    /// When `Some`, heap profile is collected on freeze.
    heap_profile_on_freeze: Cell<Option<RetainedHeapProfileMode>>,
}

impl FrozenModule {
    /// Convert items in `globals` into a `FrozenModule`.
    /// This function can be used to implement starlark module
    /// using the `#[starlark_module]` attribute.
    ///
    /// This function does not return an error,
    /// but we prefer not to panic if there's some high level logic error.
    pub fn from_globals(globals: &Globals) -> FreezeResult<FrozenModule> {
        Module::with_temp_heap(|module| {
            for (name, value) in globals.iter() {
                module.set(name, value.add_to_heap(module.heap()));
            }

            if let Some(docstring) = globals.docstring() {
                module.set_docstring(String::from(docstring));
            }

            module.freeze_named(FrozenHeapName::Singleton(singleton_heap_name!()))
        })
    }

    /// Read the module's data at the brand of its heap.
    pub(crate) fn with_data<R>(&self, f: impl for<'fv> FnOnce(&FrozenModuleData<'fv>) -> R) -> R {
        self.data.by_ref(|data| f(&data.value))
    }

    /// The slot of `name` and its visibility, if the module defines `name` and assigned it.
    fn lookup(&self, name: &str) -> Option<(ModuleSlotId, Visibility)> {
        self.with_data(|data| data.lookup(name))
    }

    fn lookup_err(&self, name: &str) -> anyhow::Result<(ModuleSlotId, Visibility)> {
        self.lookup(name)
            .ok_or_else(|| match did_you_mean(name, self.names()) {
                Some(better) => EnvironmentError::ModuleHasNoSymbolDidYouMean(
                    name.to_owned(),
                    better.to_owned(),
                )
                .into(),
                None => EnvironmentError::ModuleHasNoSymbol(name.to_owned()).into(),
            })
    }

    /// The slot of the exported `name`: `None` if the module does not define it, an error if it
    /// is private.
    fn exported_slot(&self, name: &str) -> anyhow::Result<Option<ModuleSlotId>> {
        match self.lookup(name) {
            None => Ok(None),
            Some((_, Visibility::Private)) => {
                Err(EnvironmentError::ModuleSymbolIsNotExported(name.to_owned()).into())
            }
            Some((slot, Visibility::Public)) => Ok(Some(slot)),
        }
    }

    /// The value of a slot that [`lookup`](FrozenModule::lookup) found, kept alive by this
    /// module's heap.
    fn slot_owned(&self, slot: ModuleSlotId) -> OwnedFrozen<Value<'static>> {
        self.data
            .dupe()
            .maybe_map::<Value<'static>, _>(|data| data.value.slots.get_slot(slot))
            .expect("`lookup` found the slot assigned")
    }

    /// Like [`slot_owned`](FrozenModule::slot_owned), borrowing this module instead of sharing
    /// ownership of its heap.
    fn slot_ref(&self, slot: ModuleSlotId) -> OwnedFrozenRef<'_, Value<'static>> {
        self.data
            .as_ref()
            .maybe_map::<Value<'static>, _>(|data| data.value.slots.get_slot(slot))
            .expect("`lookup` found the slot assigned")
    }

    /// Get value, exported or private by name, kept alive by this module's heap.
    // TODO(nga): separate private visibility into private (`_foo`) and imported (`load('foo')`).
    //   Users might want to access private variables, but not imported.
    #[doc(hidden)]
    pub fn get_any_visibility(
        &self,
        name: &str,
    ) -> anyhow::Result<(OwnedFrozen<Value<'static>>, Visibility)> {
        let (slot, vis) = self.lookup_err(name)?;
        Ok((self.slot_owned(slot), vis))
    }

    /// Get the value of the exported variable `name`, kept alive by this module's heap.
    ///
    /// # Returns
    /// * `None` if symbol is not found
    /// * error if symbol is private
    pub fn get_option(&self, name: &str) -> anyhow::Result<Option<OwnedFrozen<Value<'static>>>> {
        Ok(self.exported_slot(name)?.map(|slot| self.slot_owned(slot)))
    }

    /// Like [`get_option`](FrozenModule::get_option), but borrowing this module
    /// instead of sharing ownership of its heap.
    pub fn get_option_ref(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<OwnedFrozenRef<'_, Value<'static>>>> {
        Ok(self.exported_slot(name)?.map(|slot| self.slot_ref(slot)))
    }

    /// Get the value of the exported variable `name`, kept alive by this module's heap.
    /// Returns an error if the variable isn't defined in the module or it is private.
    pub fn get(&self, name: &str) -> anyhow::Result<OwnedFrozen<Value<'static>>> {
        match self.lookup_err(name)? {
            (_, Visibility::Private) => {
                Err(EnvironmentError::ModuleSymbolIsNotExported(name.to_owned()).into())
            }
            (slot, Visibility::Public) => Ok(self.slot_owned(slot)),
        }
    }

    /// Iterate through all the names defined in this module.
    /// Only includes symbols that are publicly exposed.
    pub fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.data.as_ref().value().as_ref().value.names()
    }

    /// The heap which owns the storage of all values defined in this module.
    pub fn frozen_heap(&self) -> OwnedFrozenRef<'_, ()> {
        self.data.owner()
    }

    /// Print out some approximation of the module definitions.
    pub fn describe(&self) -> String {
        self.with_data(|data| data.describe())
    }

    /// The documentation for the module, and all of its top level values
    ///
    /// Returns `(<module documentation>, { <symbol> : <that symbol's documentation> })`
    pub fn documentation(&self) -> DocModule {
        self.with_data(|data| DocModule {
            docs: data.documentation(),
            // Only public symbols are shown in the documentation.
            members: data
                .items()
                .map(|(k, v)| (k.as_str().to_owned(), v.documentation()))
                .collect(),
        })
    }

    /// Retained memory info, or error if not enabled.
    pub fn heap_profile(&self) -> anyhow::Result<ProfileData> {
        self.with_data(|data| match &data.heap_profile {
            None => Err(ModuleError::RetainedMemoryProfileNotEnabled.into()),
            Some(p) => Ok(p.to_profile()),
        })
    }

    /// `extra_value` field from `Module`, frozen, kept alive by this module's heap.
    pub fn extra_value(&self) -> Option<OwnedFrozen<Value<'static>>> {
        self.data
            .dupe()
            .maybe_map::<Value<'static>, _>(|data| data.value.extra_value)
    }
}

impl<'v> FrozenModuleData<'v> {
    /// The slot of `name` and its visibility, if the module defines `name` and assigned it.
    fn lookup(&self, name: &str) -> Option<(ModuleSlotId, Visibility)> {
        let (slot, vis) = self.names.get_name(name)?;
        self.slots.get_slot(slot)?;
        Some((slot, vis))
    }

    fn names(&self) -> impl Iterator<Item = &'v str> + '_ {
        self.names.symbols().map(|(name, _slot)| name.as_str())
    }

    fn describe(&self) -> String {
        self.items()
            .map(|(name, val)| val.describe(&name))
            .join("\n")
    }

    /// The exported symbols and their values.
    fn items(&self) -> impl Iterator<Item = (StringValue<'v>, Value<'v>)> + '_ {
        self.names
            .symbols()
            .filter_map(|(name, slot)| Some((name, self.slots.get_slot(slot)?)))
    }

    /// All symbols and their values, including the private and the imported ones.
    pub(crate) fn all_items(&self) -> impl Iterator<Item = (StringValue<'v>, Value<'v>)> + '_ {
        self.names
            .all_symbols()
            .filter_map(|(name, slot)| Some((name, self.slots.get_slot(slot)?)))
    }

    pub(crate) fn get_slot(&self, slot: ModuleSlotId) -> Option<Value<'v>> {
        self.slots.get_slot(slot)
    }

    /// Try and go back from a slot to a name.
    /// Inefficient - only use in error paths.
    pub(crate) fn get_slot_name(&self, slot: ModuleSlotId) -> Option<StringValue<'v>> {
        for (s, i) in self.names.symbols() {
            if i == slot {
                return Some(s);
            }
        }
        None
    }

    fn documentation(&self) -> Option<DocString> {
        self.docstring
            .as_ref()
            .and_then(|d| DocString::from_docstring(DocStringKind::Starlark, d))
    }
}

impl<'v> Module<'v> {
    /// Create a new module environment with no contents and make it available to the user.
    ///
    /// Module is discarded after the function returns.
    pub fn with_temp_heap<R, F>(f: F) -> R
    where
        F: for<'v2> FnOnce(Module<'v2>) -> R,
    {
        Heap::temp(|h| {
            // SAFETY: Mostly not
            unsafe {
                h.allow_gc();
            }
            f(Module::with_heap(h))
        })
    }

    /// Like `with_temp_heap`, but async.
    pub async fn with_temp_heap_async<R, F>(f: F) -> R
    where
        F: for<'v2> AsyncFnOnce(Module<'v2>) -> R,
    {
        Heap::temp_async(async |h| {
            // SAFETY: Mostly not
            unsafe {
                h.allow_gc();
            }
            f(Module::with_heap(h)).await
        })
        .await
    }

    /// Create a new module environment with no contents that will use the provided heap.
    // FIXME(JakobDegen): Private for now because this disables GC and it's hard to make it sound in
    // combination with that. Once GC is fixed, can be made public.
    pub(crate) fn with_heap(heap: Heap<'v>) -> Self {
        Self {
            heaps: ModuleHeaps::new(heap),
            names: MutableNames::new(),
            slots: MutableSlots::new(),
            docstring: RefCell::new(None),
            eval_duration: Cell::new(Duration::ZERO),
            extra_value: Cell::new(None),
            heap_profile_on_freeze: Cell::new(None),
        }
    }

    pub(crate) fn enable_retained_heap_profile(&self, mode: RetainedHeapProfileMode) {
        self.heap_profile_on_freeze.set(Some(mode));
    }

    /// Get the heap on which values are allocated by this module.
    pub fn heap(&self) -> Heap<'v> {
        self.heaps.heap()
    }

    /// Allocate on the frozen heap of this module.
    ///
    /// The handle is branded with a lifetime private to `f`, so values allocated through it must
    /// be rebranded with the [`HeapEdge`] to be used with values of this module. They stay alive
    /// as long as values of this module's [`heap`](Module::heap) do, whether the module is frozen
    /// or dropped.
    pub fn frozen_heap<R>(
        &self,
        f: impl for<'fm> FnOnce(FrozenHeap<'fm>, HeapEdge<'v, 'fm>) -> R,
    ) -> R {
        self.heaps.frozen_heap(|fh, edge, _seal_edge| f(fh, edge))
    }

    /// The module's heaps, for the compiler, which needs the edge back from the value heap.
    pub(crate) fn heaps(&self) -> &ModuleHeaps<'v> {
        &self.heaps
    }

    pub(crate) fn frozen_heap_allocated_bytes(&self) -> usize {
        self.heaps.frozen_heap_allocated_bytes()
    }

    /// Iterate through all the names defined in this module.
    /// Only includes symbols that are publicly exposed.
    pub fn names(&self) -> impl Iterator<Item = &'v str> + '_ {
        self.names_and_visibilities()
            .filter_map(|(name, vis)| match vis {
                Visibility::Public => Some(name),
                Visibility::Private => None,
            })
    }

    pub(crate) fn values_by_slot_id(&self) -> Vec<(ModuleSlotId, Value<'v>)> {
        self.slots().values_by_slot_id()
    }

    /// Iterate through all the names defined in this module, including those that are private.
    pub fn names_and_visibilities(&self) -> impl Iterator<Item = (&'v str, Visibility)> + '_ {
        self.names
            .all_names_and_visibilities()
            .into_iter()
            .map(|(name, vis)| (name.as_str(), vis))
    }

    pub(crate) fn mutable_names(&self) -> &MutableNames<'v> {
        &self.names
    }

    pub(crate) fn slots(&self) -> &MutableSlots<'v> {
        &self.slots
    }

    /// Get value, exported or private by name.
    pub(crate) fn get_any_visibility(&self, name: Hashed<&str>) -> Option<(Value<'v>, Visibility)> {
        let (slot, vis) = self.names.get_name(name)?;
        let value = self.slots().get_slot(slot)?;
        Some((value, vis))
    }

    /// Get the value of the exported variable `name`.
    /// Returns [`None`] if the variable isn't defined in the module or it is private.
    pub fn get(&self, name: &str) -> Option<Value<'v>> {
        self.get_any_visibility(Hashed::new(name))
            .and_then(|(v, vis)| match vis {
                Visibility::Private => None,
                Visibility::Public => Some(v),
            })
    }

    /// Freeze the environment, all its value will become immutable afterwards.
    ///
    /// When the `pagable` feature is enabled, this method is hidden to enforce
    /// that all heaps are named. Use [`freeze_named`](Self::freeze_named) instead.
    #[cfg(not(feature = "pagable"))]
    pub fn freeze(self) -> FreezeResult<FrozenModule> {
        self.freeze_impl(None)
    }

    /// Freeze the environment and assign a name to the contained frozen heap.
    ///
    /// The `name` identifies the contained frozen heap and should be unique.
    /// See [`OwnedFrozen::name`] for more details.
    pub fn freeze_named(self, name: FrozenHeapName) -> FreezeResult<FrozenModule> {
        self.freeze_impl(Some(name))
    }

    fn freeze_impl(self, name: Option<FrozenHeapName>) -> FreezeResult<FrozenModule> {
        let Module {
            heaps,
            names,
            slots,
            docstring,
            eval_duration,
            extra_value,
            heap_profile_on_freeze,
        } = self;
        #[cfg(not(target_arch = "wasm32"))]
        let start = Instant::now();
        let heap = heaps.heap();
        // This is when we do the GC/freeze, using the module slots as roots
        // Note that we even freeze anonymous slots, since they are accessed by
        // slot-index in the code, and we don't walk into them, so don't know if
        // they are used.
        let data = heaps.seal_with(name, |freezer, edge, seal_edge| {
            let fh = freezer.frozen_heap();
            let names = names.freeze(freezer)?;
            let slots = slots.freeze(freezer)?;
            let extra_value = extra_value
                .into_inner()
                .map(|v| freezer.freeze(v))
                .transpose()?;
            let heap_profile = if let Some(mode) = heap_profile_on_freeze.get() {
                // TODO(nga): retained heap profile does not store information about data
                //   allocated in frozen heap before freeze starts.
                let heap_profile = AggregateHeapProfileInfo::collect(heap, Some(HeapKind::Frozen));
                Some(Box::new(RetainedHeapProfile {
                    info: heap_profile,
                    mode,
                }))
            } else {
                None
            };
            let data = fh.alloc_simple_typed(StarlarkAnyComplex::new(FrozenModuleData {
                names,
                slots,
                extra_value,
                docstring: docstring.into_inner(),
                heap_profile,
            }));
            let module = FrozenValueTyped::from_typed(data).expect("allocated in a frozen heap");
            for frozen_def in freezer.frozen_defs.borrow().as_slice() {
                frozen_def.post_freeze(module, heap, fh, edge, seal_edge);
            }
            FreezeResult::Ok(data)
        })?;

        Ok(FrozenModule {
            data,
            #[cfg(not(target_arch = "wasm32"))]
            eval_duration: start.elapsed() + eval_duration.get(),
            #[cfg(target_arch = "wasm32")]
            eval_duration: eval_duration.get(),
        })
    }

    /// Set the value of a variable in the environment.
    /// Modifying these variables while executing is ongoing can have
    /// surprising effects.
    pub fn set(&self, name: &str, value: Value<'v>) {
        let name = self.frozen_heap(|fh, edge| edge.rebrand(fh.alloc_str(name)));
        let slot = self.names.add_name(name);
        let slots = self.slots();
        slots.ensure_slot(slot);
        slots.set_slot(slot, value);
    }

    /// Symbols starting with underscore are considered private.
    pub(crate) fn default_visibility(symbol: &str) -> Visibility {
        match symbol.starts_with('_') {
            true => Visibility::Private,
            false => Visibility::Public,
        }
    }

    /// Set the value of a variable in the environment. Set its visibliity to
    /// "private" to ensure that it is not re-exported
    pub(crate) fn set_private(&self, name: StringValue<'v>, value: Value<'v>) {
        let slot = self.names.add_name_visibility(name, Visibility::Private);
        let slots = self.slots();
        slots.ensure_slot(slot);
        slots.set_slot(slot, value);
    }

    /// Import symbols from a module, similar to what is done during `load()`.
    pub fn import_public_symbols(&self, module: &FrozenModule) {
        self.frozen_heap(|fh, edge| {
            let data = module.data.as_ref().add_to_frozen_heap(fh);
            for (k, slot) in data.value.names.symbols() {
                if Self::default_visibility(&k) == Visibility::Public {
                    if let Some(value) = data.value.slots.get_slot(slot) {
                        self.set_private(edge.rebrand(k), edge.rebrand(value));
                    }
                }
            }
        })
    }

    pub(crate) fn load_symbol(
        &self,
        module: &FrozenModule,
        symbol: &str,
    ) -> crate::Result<Value<'v>> {
        if Self::default_visibility(symbol) != Visibility::Public {
            return Err(crate::Error::new_other(
                EnvironmentError::CannotImportPrivateSymbol(symbol.to_owned()),
            ));
        }
        match module.lookup_err(symbol)? {
            (slot, Visibility::Public) => {
                // The compiler folds loaded values into constants in the frozen heap, so the
                // loaded heap has to be kept alive from there, not just from the value heap:
                // freezing would carry the reference over, but the constants exist before that.
                Ok(self.frozen_heap(|fh, edge| {
                    edge.rebrand(module.slot_ref(slot).add_to_frozen_heap(fh))
                }))
            }
            (_, Visibility::Private) => Err(crate::Error::new_other(
                EnvironmentError::ModuleSymbolIsNotExported(symbol.to_owned()),
            )),
        }
    }

    pub(crate) fn set_docstring(&self, docstring: String) {
        self.docstring.replace(Some(docstring));
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn add_eval_duration(&self, duration: Duration) {
        self.eval_duration.set(self.eval_duration.get() + duration);
    }

    pub(crate) fn trace(&self, tracer: &Tracer<'v>) {
        self.slots().get_slots_mut().trace(tracer);

        let extra_value = self.extra_value();
        if let Some(mut extra_value) = extra_value {
            extra_value.trace(tracer);
            self.set_extra_value(extra_value);
        }

        self.heap().trace_interner(tracer);
    }

    /// Field that can be used for any purpose you want.
    pub fn set_extra_value(&self, v: Value<'v>) {
        // Cast lifetime.
        self.extra_value.set(Some(v));
    }

    /// Set extra value, but fail if it's already set.
    pub fn set_extra_value_no_overwrite(&self, v: Value<'v>) -> anyhow::Result<()> {
        if let Some(existing) = self.extra_value() {
            return Err(ModuleError::ExtraValueAlreadySet(existing.get_type()).into());
        }
        self.set_extra_value(v);
        Ok(())
    }

    /// Field that can be used for any purpose you want.
    pub fn extra_value(&self) -> Option<Value<'v>> {
        self.extra_value.get()
    }
}

#[test]
fn test_send_sync()
where
    FrozenModule: Send + Sync,
{
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::environment::FrozenModule;
    use crate::environment::Globals;
    use crate::environment::GlobalsBuilder;
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::eval::runtime::profile::mode::ProfileMode;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;
    use crate::values::NoSerialize;
    use crate::values::ProvidesStaticType;
    use crate::values::StarlarkPagablePanic;
    use crate::values::StarlarkValue;
    use crate::values::Trace;
    use crate::values::Value;
    use crate::values::layout::heap::name::StarlarkTestHeapName;
    use crate::values::list::ListRef;
    use crate::values::starlark_value;

    #[test]
    fn test_gen_heap_summary_profile() {
        Module::with_temp_heap(|module| {
            {
                let mut eval = Evaluator::new(&module);
                eval.enable_profile(&ProfileMode::HeapSummaryRetained)
                    .unwrap();
                eval.eval_module(
                    AstModule::parse(
                        "x.star",
                        r"
def f(x):
    return list([x])

x = f(1)
"
                        .to_owned(),
                        &Dialect::AllOptionsInternal,
                    )
                    .unwrap(),
                    &Globals::standard(),
                )
                .unwrap();
            }
            let module = module.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;
            let heap_summary = module.heap_profile().unwrap().gen_csv().unwrap();
            // Smoke test.
            assert!(heap_summary.contains("\"x.star.f\""), "{heap_summary:?}");
            crate::Result::Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_frozen_module_from_globals() {
        #[starlark_module]
        fn some_globals(globals: &mut GlobalsBuilder) {
            fn foo() -> anyhow::Result<i32> {
                Ok(17)
            }

            const BAR: Vec<String> = Vec::new();
        }

        let mut globals = GlobalsBuilder::new();
        some_globals(&mut globals);
        let globals = globals.build();

        let module = FrozenModule::from_globals(&globals).unwrap();
        assert_eq!(
            "function",
            module.get("foo").unwrap().as_ref().value().get_type()
        );
        assert_eq!(
            0,
            ListRef::from_value(module.get("BAR").unwrap().as_ref().value())
                .unwrap()
                .len()
        );
    }

    /// A value of the module's frozen heap, stored in a list on the value heap.
    fn frozen_heap_string_in_value_heap<'v>(module: &Module<'v>, expected: &str) -> Value<'v> {
        let s = module.frozen_heap(|fh, edge| edge.rebrand(fh.alloc(expected)));
        module.heap().alloc(vec![s])
    }

    fn assert_list_of_string(list: Value, expected: &str) {
        let list = ListRef::from_value(list).unwrap();
        assert_eq!(list.content()[0].unpack_str(), Some(expected));
    }

    #[test]
    fn test_dropped_module_keeps_its_frozen_heap_alive() {
        Module::with_temp_heap(|module| {
            let expected = "a string that lives on the module's frozen heap".repeat(8);
            let list = frozen_heap_string_in_value_heap(&module, &expected);
            drop(module);
            assert_list_of_string(list, &expected);
        })
    }

    #[test]
    fn test_failed_freeze_keeps_the_frozen_heap_alive() {
        #[derive(
            Debug,
            derive_more::Display,
            ProvidesStaticType,
            Trace,
            Allocative,
            NoSerialize,
            StarlarkPagablePanic
        )]
        #[display("unfreezable")]
        struct Unfreezable;

        #[starlark_value(type = "unfreezable")]
        impl<'v> StarlarkValue<'v> for Unfreezable {}

        Module::with_temp_heap(|module| {
            let expected = "a string that lives on the module's frozen heap".repeat(8);
            let list = frozen_heap_string_in_value_heap(&module, &expected);
            module.set("x", module.heap().alloc_complex_no_freeze(Unfreezable));
            assert!(
                module
                    .freeze_named(StarlarkTestHeapName::frozen_heap_name())
                    .is_err()
            );
            assert_list_of_string(list, &expected);
        })
    }
}
