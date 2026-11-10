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

use std::sync::Arc;
use std::sync::OnceLock;

use allocative::Allocative;
use dupe::Dupe;
use itertools::Itertools;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::StaticStr;

use crate as starlark;
use crate::__derive_refs::components::NativeCallableComponents;
use crate::any::IsStaticType;
use crate::collections::SmallMap;
use crate::collections::symbol::map::SymbolMap;
use crate::docs::DocItem;
use crate::docs::DocModule;
use crate::docs::DocString;
use crate::docs::DocStringKind;
use crate::docs::DocType;
use crate::eval::ParametersSpec;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializerImpl;
use crate::pagable::starlark_deserialize_context::StarlarkDeserializerImpl;
use crate::register_starlark_any;
use crate::stdlib;
pub use crate::stdlib::LibraryExtension;
use crate::typing::Ty;
use crate::values::AllocFrozenRepresentation;
use crate::values::AllocFrozenValue;
use crate::values::FrozenHeap;
use crate::values::HeapEdge;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenHeap;
use crate::values::OwnedFrozenRef;
use crate::values::ProvidesStaticType;
use crate::values::StarlarkPagable;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::function::NativeFunc;
use crate::values::function::NativeFuncFn;
use crate::values::function::SpecialBuiltinFunction;
use crate::values::layout::heap::name::FrozenHeapName;
use crate::values::namespace::Namespace;
use crate::values::namespace::value::MaybeDocHiddenValue;
use crate::values::types::function::NativeFunction;

/// The global values available during execution: the builtins, and whatever the embedder adds
/// through a [`GlobalsBuilder`]. Always frozen.
///
/// The values live in a frozen heap that the `Globals` owns; they are reached as
/// [`OwnedFrozenRef`]s ([`get_ref`](Globals::get_ref), [`iter`](Globals::iter)), which carry the
/// heap along. The compiler resolves a global name to its value, so a module evaluated with these
/// globals refers to the values directly, and its frozen heap keeps this heap alive.
#[derive(
    Clone,
    Dupe,
    Debug,
    Allocative,
    starlark_derive::StarlarkPagableViaPagable
)]
pub struct Globals(Arc<GlobalsInner>);

#[derive(Debug, Allocative)]
struct GlobalsInner {
    data: OwnedFrozen<GlobalsData<'static>>,
    /// The keys of `data.variables`, sorted.
    variable_names: Vec<String>,
    docstring: Option<String>,
}

/// A value in a [`Globals`], plus whether it should be hidden from generated documentation.
#[derive(Clone, Copy, Dupe, Debug, Allocative, StarlarkPagable)]
pub(crate) struct GlobalValue<'v> {
    pub(crate) value: Value<'v>,
    pub(crate) doc_hidden: bool,
}

/// The values of a [`Globals`], at the brand of the heap that owns them.
#[derive(Debug, Allocative, ProvidesStaticType)]
pub(crate) struct GlobalsData<'v> {
    pub(crate) variables: SymbolMap<GlobalValue<'v>>,
}

/// A value in a [`GlobalsBuilder`]: allocated in the builder's heap or in a heap it references,
/// and stored with the brand erased until `build` pairs it with the sealed heap.
#[derive(Clone, Copy, Dupe, Debug)]
struct BuilderValue {
    value: Value<'static>,
    doc_hidden: bool,
}

impl PagableSerialize for Globals {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        let GlobalsInner {
            data,
            variable_names,
            docstring,
        } = &*self.0;

        // Serialize the heap (via pagable arc — actual heap data may be deferred).
        data.heap_arc().pagable_serialize(serializer)?;

        // Force-register chunk indices for the heap and its transitive deps. The
        // pagable arc may not run heap serialization yet, but we need the
        // chunk indices now so the upcoming starlark serializer can resolve
        // value pointers. Same trick as `OwnedFrozen` and `FrozenModule`.
        let state = StarlarkSerializerImpl::get_or_create_state(serializer);
        state.ensure_chunk_index_registered(data.heap_arc())?;
        let mut ctx = StarlarkSerializerImpl::new_with_root(serializer, state, data.heap_arc());

        data.by_ref(|data| data.variables.starlark_serialize(&mut ctx))
            .map_err(|e: crate::Error| e.into_anyhow())?;
        drop(ctx);

        variable_names.pagable_serialize(serializer)?;
        docstring.pagable_serialize(serializer)?;

        Ok(())
    }
}

impl<'de> PagableDeserialize<'de> for Globals {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let heap = OwnedFrozen::<()>::pagable_deserialize(deserializer)?;

        // The preceding heap deserialization registers its heap state in the
        // Starlark scope, so Starlark fields can resolve value pointers.
        let origin = heap.heap_arc().dupe();
        let data: OwnedFrozen<GlobalsData<'static>> =
            StarlarkDeserializerImpl::recover_root_from_pagable_in(
                deserializer.as_dyn(),
                &origin,
                |ctx| {
                    let variables = <SymbolMap<GlobalValue<'_>>>::starlark_deserialize(ctx)
                        .map_err(|e: crate::Error| e.into_anyhow())?;
                    // SAFETY: The context's brand is `heap`, which the values were resolved
                    // against, so `heap` keeps them alive.
                    pagable::Result::Ok(unsafe {
                        OwnedFrozen::unchecked_new(heap, GlobalsData { variables })
                    })
                },
            )?;

        let variable_names = <Vec<String>>::pagable_deserialize(deserializer)?;
        let docstring = <Option<String>>::pagable_deserialize(deserializer)?;

        Ok(Globals::from_data(data, variable_names, docstring))
    }
}

/// Heap name for a [`Globals`] object, used for heap graph tracking.
#[derive(Debug, Clone, Copy, Hash, pagable::Pagable)]
pub struct GlobalFrozenHeapName {
    /// A name identifying this globals heap.
    pub name: StaticStr,
}

impl std::fmt::Display for GlobalFrozenHeapName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "globals({})", self.name)
    }
}

/// Used to build a [`Globals`] value.
#[derive(Debug)]
pub struct GlobalsBuilder {
    // The heap everything is allocated in
    heap: OwnedFrozenHeap,
    // Normal top-level variables, e.g. True/hash
    variables: SymbolMap<BuilderValue>,
    /// The fields of the namespaces being built, innermost last. The keys are stored like the
    /// values, see [`BuilderValue`].
    namespace_fields: Vec<SmallMap<StringValue<'static>, BuilderValue>>,
    /// The raw docstring for this module
    ///
    /// FIXME(JakobDegen): This should probably be removed. Having a docstring on a `GlobalsBuilder`
    /// doesn't really make sense, because there's no way good way to combine multiple docstrings.
    docstring: Option<String>,
}

impl Globals {
    /// Create an empty [`Globals`], with no functions in scope.
    pub fn new() -> Self {
        GlobalsBuilder::new().build()
    }

    /// Create a [`Globals`] following the
    /// [Starlark standard](https://github.com/bazelbuild/starlark/blob/master/spec.md#built-in-constants-and-functions).
    pub fn standard() -> Self {
        GlobalsBuilder::standard().build()
    }

    /// Create a [`Globals`] combining those functions in the Starlark standard plus
    /// all those defined in [`LibraryExtension`].
    ///
    /// This function is public to use in the `starlark` binary,
    /// but users of starlark should list the extensions they want explicitly.
    #[doc(hidden)]
    pub fn extended_internal() -> Self {
        GlobalsBuilder::extended().build()
    }

    /// Create a [`Globals`] combining those functions in the Starlark standard plus
    /// all those given in the [`LibraryExtension`] arguments.
    pub fn extended_by(extensions: &[LibraryExtension]) -> Self {
        GlobalsBuilder::extended_by(extensions).build()
    }

    /// Pair the values with the heap that owns them. `variable_names` are the keys of
    /// `variables`, sorted, which [`iter`](Globals::iter) relies on.
    ///
    /// # SAFETY
    ///
    /// Every value in `variables` must be allocated in `heap` or in a heap it references.
    unsafe fn from_parts(
        heap: OwnedFrozen<()>,
        variables: SymbolMap<GlobalValue<'static>>,
        variable_names: Vec<String>,
        docstring: Option<String>,
    ) -> Globals {
        // SAFETY: The caller's obligation.
        let data = unsafe { OwnedFrozen::unchecked_new(heap, GlobalsData { variables }) };
        Self::from_data(data, variable_names, docstring)
    }

    fn from_data(
        data: OwnedFrozen<GlobalsData<'static>>,
        variable_names: Vec<String>,
        docstring: Option<String>,
    ) -> Globals {
        Globals(Arc::new(GlobalsInner {
            data,
            variable_names,
            docstring,
        }))
    }

    /// The values, for the compiler to read at the brand of the heap it allocates on.
    pub(crate) fn data(&self) -> &OwnedFrozen<GlobalsData<'static>> {
        &self.0.data
    }

    /// The value bound to `name`, kept alive by this globals.
    pub fn get_ref(&self, name: &str) -> Option<OwnedFrozenRef<'_, Value<'static>>> {
        self.0
            .data
            .maybe_map_ref(|data| Some(data.variables.get_str(name)?.value))
    }

    pub(crate) fn get_owned(&self, name: &str) -> Option<OwnedFrozen<Value<'static>>> {
        Some(self.get_ref(name)?.to_owned())
    }

    /// The value bound to `name`, at the brand of the borrow.
    #[cfg(test)]
    pub(crate) fn get(&self, name: &str) -> Option<Value<'_>> {
        Some(self.get_ref(name)?.value())
    }

    /// Get all the names defined in this environment.
    pub fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.0.variable_names.iter().map(String::as_str)
    }

    /// Iterate over all the items in this environment, in the order of [`names`](Globals::names).
    pub fn iter(&self) -> impl Iterator<Item = (&str, OwnedFrozenRef<'_, Value<'static>>)> {
        self.names().map(|name| {
            (
                name,
                self.get_ref(name)
                    .expect("`variable_names` are the keys of `variables`"),
            )
        })
    }

    /// The heap that owns the values in this globals.
    pub fn heap(&self) -> OwnedFrozenRef<'_, ()> {
        self.0.data.owner()
    }

    /// Print information about the values in this object.
    pub fn describe(&self) -> String {
        self.0.data.by_ref(|data| {
            data.variables
                .iter()
                .map(|(name, val)| val.value.describe(name.as_str()))
                .join("\n")
        })
    }

    /// Get the documentation for the object itself
    pub fn docstring(&self) -> Option<&str> {
        self.0.docstring.as_deref()
    }

    /// Get the documentation for both the object itself, and its members.
    pub fn documentation(&self) -> DocModule {
        self.0.data.by_ref(|data| {
            let (docs, members) = common_documentation(
                &self.0.docstring,
                data.variables
                    .iter()
                    .filter(|(_, v)| !v.doc_hidden)
                    .map(|(n, v)| (n.as_str(), v.value)),
            );
            DocModule {
                docs,
                members: members.collect(),
            }
        })
    }
}

impl GlobalsBuilder {
    /// Create an empty [`GlobalsBuilder`], with no functions in scope.
    pub fn new() -> Self {
        Self {
            heap: OwnedFrozenHeap::new(),
            variables: SymbolMap::new(),
            namespace_fields: Vec::new(),
            docstring: None,
        }
    }

    /// Create a [`GlobalsBuilder`] following the
    /// [Starlark standard](https://github.com/bazelbuild/starlark/blob/master/spec.md#built-in-constants-and-functions).
    pub fn standard() -> Self {
        stdlib::standard_environment()
    }

    /// Create a [`GlobalsBuilder`] combining those functions in the Starlark standard plus
    /// all those defined in [`LibraryExtension`].
    pub(crate) fn extended() -> Self {
        Self::extended_by(LibraryExtension::all())
    }

    /// Create a [`GlobalsBuilder`] combining those functions in the Starlark standard plus
    /// all those defined in [`LibraryExtension`].
    pub fn extended_by(extensions: &[LibraryExtension]) -> Self {
        let mut res = Self::standard();
        for x in extensions {
            x.add(&mut res);
        }
        res
    }

    /// Add a nested namespace to the builder. If `f` adds the definition `foo`,
    /// it will end up on a namespace `name`, accessible as `name.foo`.
    pub fn namespace(&mut self, name: &str, f: impl FnOnce(&mut GlobalsBuilder)) {
        self.namespace_inner(name, false, f)
    }

    /// Same as `namespace`, but this value will not show up in generated documentation.
    pub fn namespace_no_docs(&mut self, name: &str, f: impl FnOnce(&mut GlobalsBuilder)) {
        self.namespace_inner(name, true, f)
    }

    fn namespace_inner(
        &mut self,
        name: &str,
        doc_hidden: bool,
        f: impl FnOnce(&mut GlobalsBuilder),
    ) {
        self.namespace_fields.push(SmallMap::new());
        f(self);
        let fields = self.namespace_fields.pop().unwrap();
        let namespace = self.heap.with(|heap| {
            let fields = fields
                .into_iter()
                .map(|(name, value)| {
                    // SAFETY: `erase` stored both in `self.heap`, whose handle `heap` is.
                    let (name, v) = unsafe {
                        (
                            Self::at_brand(heap, name),
                            Self::at_brand(heap, value.value),
                        )
                    };
                    (
                        name,
                        MaybeDocHiddenValue {
                            value: v,
                            doc_hidden: value.doc_hidden,
                        },
                    )
                })
                .collect();
            // SAFETY: Allocated in `self.heap` just here.
            unsafe { Self::erase(heap.alloc(Namespace::new(fields))) }
        });
        // SAFETY: Allocated in `self.heap` just above.
        unsafe { self.set_inner(name, namespace, doc_hidden) }
    }

    /// Forget the brand of a value, for storage in this builder.
    ///
    /// # Safety
    ///
    /// `v` must be allocated in `self.heap` or in a heap it references.
    unsafe fn erase<'fh, T: IsStaticType>(v: T::Reinfect<'fh>) -> T
    where
        for<'fv> T::Reinfect<'fv>: Sized,
    {
        // SAFETY: `self.heap` keeps the value alive until `build`, and the `Globals` it becomes
        // does afterwards.
        unsafe { OwnedFrozen::<T>::erase_brand(v) }
    }

    /// A stored value, back at the brand of this builder's heap; `_heap` only names the brand.
    ///
    /// # Safety
    ///
    /// `v` must be allocated in the heap `_heap` is the handle of, or in a heap it references.
    unsafe fn at_brand<'fh, T: IsStaticType>(_heap: FrozenHeap<'fh>, v: T) -> T::Reinfect<'fh>
    where
        for<'fv> T::Reinfect<'fv>: Sized,
    {
        // SAFETY: The caller's obligation; `'fh` is closure-introduced, so it names nothing but
        // `_heap`.
        unsafe { OwnedFrozen::<T>::restore_brand(v) }
    }

    /// A fluent API for modifying [`GlobalsBuilder`] and returning the result.
    pub fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    /// A fluent API for modifying [`GlobalsBuilder`] using [`namespace`](GlobalsBuilder::namespace).
    pub fn with_namespace(mut self, name: &str, f: impl Fn(&mut GlobalsBuilder)) -> Self {
        self.namespace(name, f);
        self
    }

    /// Called at the end to build a [`Globals`].
    pub fn build(self) -> Globals {
        self.build_impl(None)
    }

    /// Called at the end to build a [`Globals`] with a named heap.
    pub fn build_named(self, name: GlobalFrozenHeapName) -> Globals {
        self.build_impl(Some(name))
    }

    fn build_impl(self, name: Option<GlobalFrozenHeapName>) -> Globals {
        let mut variable_names: Vec<String> = self
            .variables
            .keys()
            .map(|x| x.as_str().to_owned())
            .collect();
        variable_names.sort();
        let heap = self.heap.seal_impl(name.map(FrozenHeapName::Global), None);
        let variables = self.variables.map_values(|v| GlobalValue {
            value: v.value,
            doc_hidden: v.doc_hidden,
        });
        // SAFETY: `set_inner`'s contract: every stored value is in `self.heap`, which was just
        // sealed into `heap`, or in a heap it references.
        unsafe { Globals::from_parts(heap, variables, variable_names, self.docstring) }
    }

    /// Set a value in the [`GlobalsBuilder`] that is a representation, allocated the way it
    /// chose to be.
    pub fn set_representation<'v, B: for<'fv> AllocFrozenRepresentation<'fv>>(
        &'v mut self,
        name: &str,
        builder: B,
    ) {
        // SAFETY: Allocated in `self.heap` just here.
        let value = self
            .heap
            .with(|heap| unsafe { Self::erase(heap.alloc_representation(builder).to_value()) });
        // SAFETY: Allocated in `self.heap` just above.
        unsafe { self.set_inner(name, value, false) }
    }

    /// Set a value in the [`GlobalsBuilder`].
    pub fn set<'v, V: for<'fv> AllocFrozenValue<'fv>>(&'v mut self, name: &str, value: V) {
        // SAFETY: Allocated in `self.heap` just here.
        let value = self
            .heap
            .with(|heap| unsafe { Self::erase(heap.alloc(value)) });
        // SAFETY: Allocated in `self.heap` just above.
        unsafe { self.set_inner(name, value, false) }
    }

    /// Store a value.
    ///
    /// # Safety
    ///
    /// `value` must be allocated in `self.heap` or in a heap it references.
    unsafe fn set_inner<'v>(&'v mut self, name: &str, value: Value<'static>, doc_hidden: bool) {
        let value = BuilderValue { value, doc_hidden };
        match self.namespace_fields.last_mut() {
            None => {
                // TODO(nga): do not quietly ignore redefinitions.
                self.variables.insert(name, value)
            }
            Some(fields) => {
                // SAFETY: Allocated in `self.heap` just here.
                let name = self
                    .heap
                    .with(|heap| unsafe { Self::erase(heap.alloc_str(name)) });
                fields.insert(name, value)
            }
        };
    }

    /// Set a method. This function is usually called from code
    /// generated by `starlark_derive` and rarely needs to be called manually.
    ///
    /// `sig` builds the signature on the heap of this builder, so that its default values live
    /// there.
    pub fn set_function(
        &mut self,
        name: &str,
        components: NativeCallableComponents,
        sig: impl for<'fh> FnOnce(FrozenHeap<'fh>) -> ParametersSpec<Value<'fh>>,
        as_type: Option<(Ty, DocType)>,
        ty: Option<Ty>,
        special_builtin_function: Option<SpecialBuiltinFunction>,
        f: NativeFuncFn,
    ) {
        let speculative_exec_safe = components.speculative_exec_safe;
        let as_type_ty = as_type.as_ref().map(|x| x.0.dupe());
        let ty = ty.unwrap_or_else(|| components.make_type(as_type_ty.dupe()));
        let value = self.heap.with(|heap| {
            let docs = components.into_docs(as_type, heap);
            let function = heap.alloc(NativeFunction {
                function: NativeFunc(f, sig(heap)),
                name: name.to_owned(),
                speculative_exec_safe,
                as_type: as_type_ty,
                ty,
                docs,
                special_builtin_function,
            });
            // SAFETY: Allocated in `self.heap` just here.
            unsafe { Self::erase(function) }
        });
        // SAFETY: Allocated in `self.heap` just above.
        unsafe { self.set_inner(name, value, false) }
    }

    /// Allocate on the heap where globals are allocated.
    pub fn frozen_heap<R>(&self, f: impl for<'fh> FnOnce(FrozenHeap<'fh>) -> R) -> R {
        self.heap.with(f)
    }

    /// Set per module docstring.
    ///
    /// This function is called by the `starlark_derive` generated code
    /// and rarely needs to be called manually.
    pub fn set_docstring(&mut self, docstring: &str) {
        self.docstring = Some(docstring.to_owned());
    }
}

/// Lazy, named cache for a [`Globals`] value. Created via the
/// [`globals_static!`](crate::globals_static) macro; the globals are built on
/// first access via the supplied initializer.
pub struct GlobalsStatic {
    cell: OnceLock<Globals>,
    name: StaticStr,
    init: fn(&mut GlobalsBuilder),
}

impl GlobalsStatic {
    /// Create a new [`GlobalsStatic`]. Prefer the
    /// [`globals_static!`](crate::globals_static) macro, which fills in `name`
    /// from the call site.
    pub const fn new(name: StaticStr, init: fn(&mut GlobalsBuilder)) -> GlobalsStatic {
        GlobalsStatic {
            cell: OnceLock::new(),
            name,
            init,
        }
    }

    /// Get (or build, on first call) the [`Globals`] value.
    pub fn globals(&'static self) -> &'static Globals {
        self.cell.get_or_init(|| {
            GlobalsBuilder::new()
                .with(self.init)
                .build_named(GlobalFrozenHeapName { name: self.name })
        })
    }

    /// Get a function out of the object. Requires that the initializer set
    /// exactly one value. If populated via a `#[starlark_module]`, that means
    /// a single function in it.
    ///
    /// The static is never dropped, so the function is usable with any heap.
    pub fn function<'v>(&'static self) -> Value<'v> {
        let globals = self.globals();
        assert!(
            globals.0.variable_names.len() == 1,
            "GlobalsBuilder.function must have exactly 1 member, you had {}",
            globals.names().map(|s| format!("`{s}`")).join(", ")
        );

        let function = globals.0.data.by_ref(|data| {
            let function = data.variables.values().next().unwrap().value;
            // SAFETY: `self` is `'static`, so the globals and their heap are never dropped.
            unsafe { OwnedFrozen::<Value<'static>>::erase_brand(function) }
        });
        HeapEdge::immortal().rebrand(function)
    }

    /// Copy all the globals into another builder. The values stay owned by
    /// the static's heap; `out`'s heap takes a reference so the dependency is
    /// recorded for downstream consumers (e.g. pagable serialization).
    pub fn populate(&'static self, out: &mut GlobalsBuilder) {
        let globals = self.globals();
        out.heap.with(|heap| heap.add_reference(globals.heap()));
        globals.0.data.by_ref(|data| {
            for (name, value) in data.variables.iter() {
                // SAFETY: `out.heap` references the heap of `globals`, which keeps the value
                // alive, since just above.
                unsafe {
                    let value_erased = GlobalsBuilder::erase(value.value);
                    out.set_inner(name.as_str(), value_erased, value.doc_hidden)
                }
            }
        });
        out.docstring = globals.0.docstring.clone();
    }
}

/// Define a `static` of type [`GlobalsStatic`] backed by an init function.
/// The heap is named `<module_path>::<NAME>`.
///
/// ```ignore
/// fn build(b: &mut GlobalsBuilder) { ... }
///
/// starlark::globals_static!(MY_GLOBALS = build);
/// ```
#[macro_export]
macro_rules! globals_static {
    ($vis:vis $name:ident = $init:expr) => {
        #[allow(dead_code)]
        $vis static $name: $crate::__derive_refs::GlobalsStatic = {
            $crate::__derive_refs::static_str!(
                __GLOBAL_HEAP_NAME =
                concat!(module_path!(), "::", stringify!($name))
            );
            $crate::__derive_refs::GlobalsStatic::new(__GLOBAL_HEAP_NAME, $init)
        };

        $crate::__derive_refs::inventory::submit! {
            $crate::__derive_refs::StaticHeapEntry {
                file: file!(),
                line: line!(),
                get_heap: || $name.globals().heap(),
            }
        }
    };
}

pub(crate) fn common_documentation<'a, 'v, T: IntoIterator<Item = (&'a str, Value<'v>)>>(
    docstring: &Option<String>,
    members: T,
) -> (
    Option<DocString>,
    impl Iterator<Item = (String, DocItem)> + use<'a, 'v, T>,
) {
    let main_docs = docstring
        .as_ref()
        .and_then(|ds| DocString::from_docstring(DocStringKind::Rust, ds));
    let member_docs = members
        .into_iter()
        .map(|(name, val)| (name.to_owned(), val.documentation()))
        .sorted_by(|(l, _), (r, _)| Ord::cmp(l, r));

    (main_docs, member_docs)
}

register_starlark_any!(Globals);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::values::none::NoneType;

    #[test]
    fn test_send_sync()
    where
        Globals: Send + Sync,
    {
    }

    #[test]
    fn test_doc_hidden() {
        let mut globals = GlobalsBuilder::new();
        globals.namespace_no_docs("ns_hidden", |_| {});
        globals.namespace("ns", |globals| {
            globals.namespace_no_docs("nested_ns_hidden", |_| {});
            globals.set("x", NoneType);
        });
        let docs = globals.build().documentation();

        let (k, v) = docs.members.into_iter().exactly_one().ok().unwrap();
        assert_eq!(&k, "ns");
        let DocItem::Module(docs) = v else {
            unreachable!()
        };
        assert_eq!(&docs.members.into_keys().exactly_one().ok().unwrap(), "x");
    }
}
