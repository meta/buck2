/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::fmt;
use std::fmt::Display;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::provider::id::ProviderId;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::NonDefaultProvidersName;
use buck2_core::provider::label::ProviderName;
use buck2_core::provider::label::ProvidersName;
use buck2_error::internal_error;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use buck2_interpreter::types::provider::callable::ValueAsProviderCallableLike;
use display_container::fmt_container;
use dupe::Dupe;
use either::Either;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::pagable::SmallMapKeyDeserialize;
use starlark::pagable::StarlarkDeserialize;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::static_starlark_value;
use starlark::typing::Ty;
use starlark::values::AllocFrozenValue;
use starlark::values::AllocValue;
use starlark::values::Deferred;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenHeap;
use starlark::values::FrozenValueTyped;
use starlark::values::Heap;
use starlark::values::HeapSendable;
use starlark::values::HeapSyncable;
use starlark::values::OwnedFrozen;
use starlark::values::OwnedFrozenRef;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Tracer;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListRef;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark_map::Hashed;

use crate::actions::impls::json::SerializeOrFail;
use crate::interpreter::rule_defs::provider::DefaultInfo;
use crate::interpreter::rule_defs::provider::DefaultInfoCallable;
use crate::interpreter::rule_defs::provider::FrozenBuiltinProviderLike;
use crate::interpreter::rule_defs::provider::ValueAsProviderLike;

fn format_provider_keys_for_error(keys: &[String]) -> String {
    format!(
        "[{}]",
        keys.iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ProviderCollectionError {
    #[error("expected a list of Provider objects, got {repr}")]
    CollectionNotAList { repr: String },
    #[error("expected a Provider object, got {repr}")]
    CollectionElementNotAProvider { repr: String },
    #[error("provider of type `{provider_name}` specified twice ({original_repr} and {new_repr})")]
    CollectionSpecifiedProviderTwice {
        provider_name: String,
        original_repr: String,
        new_repr: String,
    },
    #[error("collection {repr} did not receive a `DefaultInfo` provider")]
    CollectionMissingDefaultInfo { repr: String },
    #[error(
        "requested sub target named `{0}` of target `{1}` is not available. Available subtargets are: `{2:?}`"
    )]
    RequestedInvalidSubTarget(ProviderName, ConfiguredProvidersLabel, Vec<String>),
    #[error(
        "Cannot handle flavor `{flavor}` on target `{target}`. Most flavors are unsupported in Buck2."
    )]
    UnknownFlavors { target: String, flavor: String },
    #[error(
        "provider collection operation {0} parameter type must be a provider type \
        but not and instance of provider (for example, `RunInfo` or user defined provider type), \
        got `{1}`"
    )]
    AtTypeNotProvider(GetOp, &'static str),
    #[error(
        "provider collection does not have a key `{0}`, available keys are: {}",
        format_provider_keys_for_error(_1)
    )]
    AtNotFound(String, Vec<String>),
}

#[derive(Debug, ProvidesStaticType, Allocative, StarlarkPagable)]
#[repr(C)]
pub struct ProviderCollection<'v> {
    /// Keys and map structure stay resident independently of provider values.
    pub(crate) providers: BuckMutInsertionMap<CollectionKey, Deferred<Value<'v>>>,
}

/// Newtype wrapper around `Arc<ProviderId>` used as the key type of
/// `ProviderCollection::providers`. Wraps because `Arc<ProviderId>` is
/// pagable-only (`buck2_core` cannot depend on `starlark`), so the
/// `BuckMutInsertionMap<K, V>: StarlarkSerialize/Deserialize` blanket can't apply
/// directly to the inner type. The newtype gets `StarlarkPagable` via
/// `StarlarkPagableViaPagable` and provides the matching
/// [`SmallMapKeyDeserialize`] impl.
#[derive(
    Debug,
    Allocative,
    Eq,
    PartialEq,
    Hash,
    pagable::Pagable,
    starlark::values::StarlarkPagableViaPagable
)]
pub struct CollectionKey(Arc<ProviderId>);

impl std::ops::Deref for CollectionKey {
    type Target = Arc<ProviderId>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Lets `BuckMutInsertionMap<CollectionKey, _>::get`/`contains_key` be called with a
/// borrowed `&ProviderId` directly, avoiding a cloned `Arc<ProviderId>` and a
/// `CollectionKey` wrapper at every lookup site.
impl starlark_map::Equivalent<CollectionKey> for ProviderId {
    fn equivalent(&self, key: &CollectionKey) -> bool {
        self == key.0.as_ref()
    }
}

impl<'fv> SmallMapKeyDeserialize<'fv> for CollectionKey {
    fn starlark_deserialize_hashed(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> starlark::Result<Hashed<Self>> {
        let k = Self::starlark_deserialize(ctx)?;
        Ok(Hashed::new(k))
    }
}

static_starlark_value!(EMPTY_PROVIDER_COLLECTION: ProviderCollection<'static> = ProviderCollection {
    providers: BuckMutInsertionMap::new(),
});

/// Type of a frozen provider collection.
// These are the hand-written equivalents of `starlark_complex_value!`,
// which we can't use because empty collections should be allocated as the
// statically interned empty collection.
impl<'v> AllocValue<'v> for ProviderCollection<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        if self.providers.is_empty() {
            EMPTY_PROVIDER_COLLECTION.at().to_value()
        } else {
            heap.alloc_complex(self)
        }
    }
}

impl<'fv> AllocFrozenValue<'fv> for ProviderCollection<'fv> {
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        if self.providers.is_empty() {
            EMPTY_PROVIDER_COLLECTION.at().to_value()
        } else {
            heap.alloc_simple_typed(self).to_value()
        }
    }
}

impl<'v> ProviderCollection<'v> {
    #[inline]
    pub fn from_value(x: Value<'v>) -> Option<&'v Self> {
        x.downcast_ref::<ProviderCollection<'v>>()
    }
}

impl<'v> StarlarkTypeRepr for &'v ProviderCollection<'v> {
    type Canonical = ProviderCollection<'v>;

    fn starlark_type_repr() -> Ty {
        <ProviderCollection as StarlarkValue>::get_type_starlark_repr()
    }
}

impl<'v> UnpackValue<'v> for &'v ProviderCollection<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(ProviderCollection::from_value(value))
    }
}

/// A provider entry for display: an unreadable one shows its error in place.
struct DisplayProvider<'a, 'v>(&'a Deferred<Value<'v>>);

impl Display for DisplayProvider<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.read() {
            Ok(v) => Display::fmt(v, f),
            Err(e) => write!(f, "<unreadable provider: {e:#}>"),
        }
    }
}

impl<'v> Display for ProviderCollection<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_container(
            f,
            "Providers([",
            "])",
            self.providers.iter().map(|(_, v)| DisplayProvider(v)),
        )
    }
}

impl<'v> Serialize for ProviderCollection<'v> {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.collect_map(self.providers.iter().map(|(id, v)| {
            (
                id.name(),
                SerializeOrFail(v.read().map_err(buck2_error::Error::from)),
            )
        }))
    }
}

/// Provider collection access operator.
#[derive(derive_more::Display, Debug)]
enum GetOp {
    #[display("[]")]
    At,
    #[display("in")]
    In,
    #[display(".get")]
    Get,
}

impl<'v> ProviderCollection<'v> {
    /// Create most of the collection but don't do final assembly, or validate DefaultInfo here.
    /// This is an internal detail
    fn try_from_value_impl(
        mut value: Value<'v>,
    ) -> buck2_error::Result<BuckMutInsertionMap<CollectionKey, Deferred<Value<'v>>>> {
        // Sometimes we might have a resolved promise here, in which case see through that
        value = StarlarkPromise::get_recursive(value);

        let list = match ListRef::from_value(value) {
            Some(v) => v,
            None => {
                return Err(ProviderCollectionError::CollectionNotAList {
                    repr: value.to_repr(),
                }
                .into());
            }
        };

        let mut providers = BuckMutInsertionMap::with_capacity(list.len());
        for value in list.iter() {
            match ValueAsProviderLike::unpack_value(value)? {
                Some(provider) => {
                    if let Some(existing_value) = providers
                        .insert(CollectionKey(provider.0.id().dupe()), Deferred::new(value))
                    {
                        return Err(ProviderCollectionError::CollectionSpecifiedProviderTwice {
                            provider_name: provider.0.id().name.clone(),
                            original_repr: existing_value.read()?.to_repr(),
                            new_repr: value.to_repr(),
                        }
                        .into());
                    };
                }
                None => {
                    return Err(ProviderCollectionError::CollectionElementNotAProvider {
                        repr: value.to_repr(),
                    }
                    .into());
                }
            }
        }

        Ok(providers)
    }

    /// Takes a value, e.g. a return from a `rule()` implementation function, and builds a `ProviderCollection` from it.
    ///
    /// An error is returned if:
    ///  - `value` is not a list
    ///  - Two instances of the same provider are provided
    ///  - `DefaultInfo` is not provided
    pub fn try_from_value(value: Value<'v>) -> buck2_error::Result<ProviderCollection<'v>> {
        let providers = Self::try_from_value_impl(value)?;
        if !providers.contains_key(DefaultInfoCallable::provider_id().as_ref()) {
            return Err(ProviderCollectionError::CollectionMissingDefaultInfo {
                repr: value.to_repr(),
            }
            .into());
        }

        Ok(ProviderCollection { providers })
    }

    /// Takes a value, e.g. a value passed to `DefaultInfo(subtargets)`, and builds a `ProviderCollection` from it.
    ///
    /// An error is returned if:
    ///  - `value` is not a list
    ///  - Two instances of the same provider are provided
    ///
    /// Should only be used for subtargets, where an empty `DefaultInfo` can be inferred.
    pub fn try_from_value_subtarget(
        value: Value<'v>,
        heap: Heap<'v>,
    ) -> buck2_error::Result<ProviderCollection<'v>> {
        let mut providers = Self::try_from_value_impl(value)?;

        if !providers.contains_key(DefaultInfoCallable::provider_id().as_ref()) {
            providers.insert(
                CollectionKey(DefaultInfoCallable::provider_id().dupe()),
                Deferred::new(heap.alloc(DefaultInfo::empty(heap))),
            );
        }
        Ok(ProviderCollection { providers })
    }

    /// Takes a value, e.g. a return from a `dynamic_output` function, and builds a `ProviderCollection` from it.
    ///
    /// An error is returned if:
    ///  - `value` is not a list
    ///  - Two instances of the same provider are provided
    pub fn try_from_value_dynamic_output(
        value: Value<'v>,
    ) -> buck2_error::Result<ProviderCollection<'v>> {
        let providers = Self::try_from_value_impl(value)?;

        Ok(ProviderCollection { providers })
    }

    /// Common implementation of `[]`, `in`, and `.get`.
    fn get_impl(
        &self,
        index: Value<'v>,
        op: GetOp,
    ) -> buck2_error::Result<Either<Value<'v>, &'v ProviderId>> {
        match index.as_provider_callable() {
            Some(callable) => {
                let provider_id = callable.id()?.as_ref();
                match self.providers.get(provider_id) {
                    Some(v) => Ok(Either::Left(v.read()?.to_value())),
                    None => Ok(Either::Right(provider_id)),
                }
            }
            None => Err(ProviderCollectionError::AtTypeNotProvider(op, index.get_type()).into()),
        }
    }

    /// `.get` function implementation.
    pub(crate) fn get(
        &self,
        index: Value<'v>,
    ) -> buck2_error::Result<NoneOr<ValueOf<'v, ValueAsProviderLike<'v>>>> {
        match self.get_impl(index, GetOp::Get)? {
            Either::Left(v) => Ok(NoneOr::Other(ValueOf::new_err(v)?)),
            Either::Right(_) => Ok(NoneOr::None),
        }
    }
}

impl ProviderCollection<'static> {
    pub fn testing_new_default<'v>(heap: FrozenHeap<'v>) -> ValueTyped<'v, ProviderCollection<'v>> {
        heap.alloc_typed(ProviderCollection {
            providers: BuckMutInsertionMap::from_iter([(
                CollectionKey(DefaultInfoCallable::provider_id().dupe()),
                Deferred::new(DefaultInfo::testing_empty(heap).to_value()),
            )]),
        })
    }
}

/// Holds a set of providers.
///
/// Accessed by indexing with a provider type, e.g.
///
/// ```ignore
/// FooInfo = provider(fields=["bar"])
/// ....
/// collection.get(FooInfo) # None if absent, a FooInfo instance if present
/// ```
#[starlark_module]
fn provider_collection_methods(builder: &mut MethodsBuilder) {
    fn get<'v>(
        this: &ProviderCollection<'v>,
        index: Value<'v>,
    ) -> starlark::Result<NoneOr<ValueOf<'v, ValueAsProviderLike<'v>>>> {
        Ok(this.get(index)?)
    }
}

#[starlark_value(type = "ProviderCollection", frozen_vtable)]
impl<'v> StarlarkValue<'v> for ProviderCollection<'v> {
    fn at(&self, index: Value<'v>, _heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        match self.get_impl(index, GetOp::At)? {
            Either::Left(v) => Ok(v),
            Either::Right(provider_id) => Err(buck2_error::Error::from(
                ProviderCollectionError::AtNotFound(
                    provider_id.name.clone(),
                    self.providers.keys().map(|k| k.name.clone()).collect(),
                ),
            )
            .into()),
        }
    }

    fn is_in(&self, other: Value<'v>) -> starlark::Result<bool> {
        Ok(self.get_impl(other, GetOp::In)?.is_left())
    }

    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        Some(PROVIDER_COLLECTION_METHODS.methods())
    }
}

starlark::methods_static!(PROVIDER_COLLECTION_METHODS = provider_collection_methods);

unsafe impl<'v> Trace<'v> for ProviderCollection<'v> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        self.providers.values_mut().for_each(|v| v.trace(tracer))
    }
}

impl<'v> Freeze<'v> for ProviderCollection<'v> {
    type Frozen<'fv> = ProviderCollection<'fv>;
    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        // N.B. collect::<Result<_>> sets the lower bound to zero,
        // which can cause over-allocations in frozen containers.
        let mut providers = BuckMutInsertionMap::with_capacity(self.providers.len());
        for (k, v) in self.providers {
            providers.insert(k, v.freeze(freezer)?);
        }

        Ok(ProviderCollection { providers })
    }
}

impl<'v> ProviderCollection<'v> {
    pub fn default_info(&self) -> buck2_error::Result<ValueTyped<'v, DefaultInfo<'v>>> {
        self.builtin_provider::<DefaultInfo>()?.ok_or_else(|| {
            internal_error!(
                "DefaultInfo should always be set for providers returned from rule function"
            )
        })
    }

    pub fn contains_provider(&self, provider_id: &ProviderId) -> bool {
        self.providers.contains_key(provider_id)
    }

    pub fn builtin_provider<T: FrozenBuiltinProviderLike>(
        &self,
    ) -> buck2_error::Result<Option<ValueTyped<'v, T::Reinfect<'v>>>>
    where
        T::Reinfect<'v>: StarlarkValue<'v> + Sized,
    {
        let Some(provider) = self.get_provider_raw(T::builtin_provider_id())? else {
            return Ok(None);
        };
        Ok(Some(
            ValueTyped::new(provider).expect("Incorrect provider type"),
        ))
    }

    /// The provider under `provider_id`, read if it was deferred.
    pub fn get_provider_raw(
        &self,
        provider_id: &ProviderId,
    ) -> buck2_error::Result<Option<Value<'v>>> {
        match self.providers.get(provider_id) {
            Some(v) => Ok(Some(*v.read()?)),
            None => Ok(None),
        }
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().map(|k| k.name.to_owned()).collect()
    }

    pub fn provider_ids(&self) -> Vec<&ProviderId> {
        self.providers.keys().map(|k| &***k).collect()
    }

    /// Iterate over `(ProviderId, Value)` pairs in this collection, reading
    /// each deferred value as it is reached.
    pub fn iter_providers(
        &self,
    ) -> impl Iterator<Item = buck2_error::Result<(&ProviderId, Value<'v>)>> {
        self.providers.iter().map(|(k, v)| Ok((&***k, *v.read()?)))
    }
}

/// A `ProviderCollection` kept alive by its owning frozen heap.
#[derive(Debug, Clone, Dupe, Allocative, starlark::StarlarkPagable)]
pub struct FrozenProviderCollectionValue {
    #[allocative(skip)] // TODO(nga): do not skip.
    #[starlark_pagable(pagable)]
    pub value: OwnedFrozen<ValueTyped<'static, ProviderCollection<'static>>>,
}

#[derive(Clone, Copy, Dupe)]
pub struct FrozenProviderCollectionValueRef<'f> {
    inner: OwnedFrozenRef<'f, FrozenValueTyped<'static, ProviderCollection<'static>>>,
}

impl Serialize for FrozenProviderCollectionValue {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.value.by_ref(|v| v.as_ref().serialize(s))
    }
}

impl FrozenProviderCollectionValue {
    pub fn try_from_value(value: OwnedFrozen<Value<'static>>) -> buck2_error::Result<Self> {
        Ok(Self {
            value: value.downcast_starlark::<ProviderCollection>()?,
        })
    }

    pub fn provider_collection<'f>(&'f self) -> &'f ProviderCollection<'f> {
        self.as_ref().value().as_ref()
    }

    pub fn as_ref<'f>(&'f self) -> FrozenProviderCollectionValueRef<'f> {
        let inner = self
            .value
            .as_ref()
            .map::<FrozenValueTyped<'static, ProviderCollection<'static>>, _>(|v| {
                FrozenValueTyped::new(v.to_value()).expect("value is in a frozen heap")
            });
        FrozenProviderCollectionValueRef { inner }
    }

    pub fn add_heap_ref<'v>(&self, heap: Heap<'v>) -> FrozenValueTyped<'v, ProviderCollection<'v>> {
        self.as_ref().add_heap_ref(heap)
    }

    pub fn add_frozen_heap_ref<'v>(
        &self,
        heap: FrozenHeap<'v>,
    ) -> FrozenValueTyped<'v, ProviderCollection<'v>> {
        self.as_ref().add_frozen_heap_ref(heap)
    }

    pub fn lookup_inner<'f>(
        &'f self,
        label: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValueRef<'f>> {
        self.as_ref().lookup_inner(label)
    }

    /// Get a provider from the collection, keeping it alive by its owner heap.
    pub fn builtin_provider_value<T: FrozenBuiltinProviderLike>(
        &self,
    ) -> buck2_error::Result<Option<OwnedFrozen<ValueTyped<'static, T>>>>
    where
        for<'x> T::Reinfect<'x>: StarlarkValue<'x> + Sized,
        for<'x> ValueTyped<'x, T::Reinfect<'x>>: HeapSendable<'x> + HeapSyncable<'x>,
    {
        // `Err(None)` stands for an absent provider, so a read error is kept apart from it.
        match self
            .value
            .dupe()
            .try_map::<ValueTyped<'static, T>, Option<buck2_error::Error>, _>(|v| {
                v.as_ref()
                    .builtin_provider::<T>()
                    .map_err(Some)?
                    .ok_or(None)
            }) {
            Ok(v) => Ok(Some(v)),
            Err(None) => Ok(None),
            Err(Some(e)) => Err(e),
        }
    }
}

impl<'f> FrozenProviderCollectionValueRef<'f> {
    /// Creates a new `FrozenProviderCollectionValueRef` from the underlying projection.
    pub fn from_inner(
        inner: OwnedFrozenRef<'f, FrozenValueTyped<'static, ProviderCollection<'static>>>,
    ) -> Self {
        FrozenProviderCollectionValueRef { inner }
    }

    pub fn value(self) -> FrozenValueTyped<'f, ProviderCollection<'f>> {
        self.inner.value()
    }

    pub fn owner(self) -> OwnedFrozenRef<'f, ()> {
        self.inner.owner()
    }

    pub fn to_owned(self) -> FrozenProviderCollectionValue {
        let value = self
            .inner
            .to_owned()
            .map::<ValueTyped<'static, ProviderCollection<'static>>, _>(|v| v.to_value_typed());
        FrozenProviderCollectionValue { value }
    }

    pub fn add_heap_ref<'v>(self, heap: Heap<'v>) -> FrozenValueTyped<'v, ProviderCollection<'v>> {
        self.inner.add_to_heap(heap)
    }

    pub fn add_frozen_heap_ref<'v>(
        self,
        heap: FrozenHeap<'v>,
    ) -> FrozenValueTyped<'v, ProviderCollection<'v>> {
        self.inner.add_to_frozen_heap(heap)
    }

    pub fn lookup_inner(
        self,
        label: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValueRef<'f>> {
        match label.name() {
            ProvidersName::Default => buck2_error::Ok(self),
            ProvidersName::NonDefault(flavor) => match flavor.as_ref() {
                NonDefaultProvidersName::Named(provider_names) => {
                    let inner = self.inner.try_map::<FrozenValueTyped<
                        'static,
                        ProviderCollection<'static>,
                    >, buck2_error::Error, _>(
                        |collection_value| {
                            let mut collection_value = collection_value.to_value_typed();
                            for provider_name in &**provider_names {
                                let maybe_di = collection_value
                                    .default_info()?
                                    .get_sub_target_providers(provider_name.as_str())?;

                                match maybe_di {
                                    Some(inner) => {
                                        collection_value = inner;
                                    }
                                    None => {
                                        return Err(
                                            ProviderCollectionError::RequestedInvalidSubTarget(
                                                provider_name.clone(),
                                                label.dupe(),
                                                collection_value
                                                    .default_info()?
                                                    .sub_targets()?
                                                    .keys()
                                                    .map(|s| (*s).to_owned())
                                                    .collect(),
                                            )
                                            .into(),
                                        );
                                    }
                                }
                            }
                            // This wrapper type's constructors only accept collections stored in
                            // frozen heaps, and sub-target collections of a frozen `DefaultInfo`
                            // are themselves frozen.
                            Ok(FrozenValueTyped::new(collection_value.to_value())
                                .expect("wrapper holds a frozen collection"))
                        },
                    )?;
                    Ok(FrozenProviderCollectionValueRef { inner })
                }
                NonDefaultProvidersName::UnrecognizedFlavor(flavor) => {
                    Err(ProviderCollectionError::UnknownFlavors {
                        target: label.unconfigured().to_string(),
                        flavor: (**flavor).to_owned(),
                    }
                    .into())
                }
            },
        }
    }
}

pub mod tester {
    use buck2_interpreter::types::provider::callable::ValueAsProviderCallableLike;
    use dupe::Dupe;
    use starlark::environment::GlobalsBuilder;
    use starlark::values::Value;

    use crate::interpreter::rule_defs::provider::ProviderCollection;

    #[starlark_module]
    pub fn collection_creator(builder: &mut GlobalsBuilder) {
        fn create_collection<'v>(value: Value<'v>) -> starlark::Result<ProviderCollection<'v>> {
            Ok(ProviderCollection::try_from_value(value)?)
        }

        fn get_default_info_default_outputs<'v>(value: Value<'v>) -> starlark::Result<Value<'v>> {
            assert!(value.is_frozen(), "a frozen value to fetch DefaultInfo");
            let collection = value.downcast_ref::<ProviderCollection>().ok_or_else(|| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::StarlarkError,
                    "{:?} was not a ProviderCollection",
                    value
                )
            })?;

            let ret = collection.default_info()?.default_outputs_raw();
            Ok(ret)
        }

        fn get_default_info_sub_targets<'v>(value: Value<'v>) -> starlark::Result<Value<'v>> {
            assert!(value.is_frozen(), "a frozen value to fetch DefaultInfo");
            let collection = value.downcast_ref::<ProviderCollection>().ok_or_else(|| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::StarlarkError,
                    "{:?} was not a ProviderCollection",
                    value
                )
            })?;

            let ret = collection.default_info()?.sub_targets_raw();
            Ok(ret)
        }

        fn contains_provider<'v>(
            collection: Value<'v>,
            provider: Value<'v>,
        ) -> starlark::Result<bool> {
            let id = provider
                .as_provider_callable()
                .unwrap()
                .id()
                .unwrap()
                .dupe();

            assert!(collection.is_frozen(), "a frozen value");
            let res = collection
                .downcast_ref::<ProviderCollection>()
                .ok_or_else(|| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::StarlarkError,
                        "{:?} was not a ProviderCollection",
                        collection
                    )
                })?
                .contains_provider(&id);

            Ok(res)
        }

        fn providers_list<'v>(collection: Value<'v>) -> starlark::Result<Vec<String>> {
            assert!(collection.is_frozen(), "a frozen value");
            Ok(collection
                .downcast_ref::<ProviderCollection>()
                .ok_or_else(|| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::StarlarkError,
                        "{:?} was not a ProviderCollection",
                        collection
                    )
                })?
                .provider_names())
        }
    }
}

#[starlark_module]
#[starlark_types(
    ProviderCollection<'static> as ProviderCollection
)]
pub(crate) fn register_provider_collection(globals: &mut GlobalsBuilder) {}
