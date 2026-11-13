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

use std::sync::OnceLock;

use dupe::Dupe;
use pagable::StaticStr;
use starlark_map::Hashed;

use crate::__derive_refs::components::NativeCallableComponents;
use crate::collections::symbol::map::SymbolMap;
use crate::collections::symbol::symbol::Symbol;
use crate::docs::DocType;
use crate::environment::common_documentation;
use crate::eval::ParametersSpec;
use crate::typing::Ty;
use crate::values::AllocFrozenRepresentation;
use crate::values::AllocFrozenValue;
use crate::values::FrozenHeap;
use crate::values::FrozenValueTyped;
use crate::values::Heap;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenHeap;
use crate::values::OwnedFrozenRef;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::function::NativeAttribute;
use crate::values::function::NativeMeth;
use crate::values::function::NativeMethFn;
use crate::values::function::NativeMethod;
use crate::values::layout::heap::name::FrozenHeapName;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::types::unbound::UnboundValue;

/// Methods of an object.
///
/// A methods table is reached as `&'static Methods`, through
/// [`StarlarkValue::get_methods`](crate::values::StarlarkValue::get_methods). That is what keeps
/// its members alive, and the readers that hand out members require it: the members are stored
/// at the `'static` brand, which such a reference makes honest, see
/// [`HeapEdge::immortal`](crate::values::HeapEdge::immortal).
#[derive(Clone, Debug)]
pub struct Methods {
    /// This field holds the objects referenced in `members`.
    heap: OwnedFrozen<()>,
    members: SymbolMap<UnboundValue<'static>>,
    docstring: Option<String>,
}

/// Heap name for a [`Methods`] object, used for heap graph tracking.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, pagable::Pagable)]
pub struct MethodFrozenHeapName {
    /// A name identifying this methods heap (e.g. type name like "dict",
    /// or a module path like "starlark::values::types::dict::methods::dict_methods").
    pub name: StaticStr,
}

impl std::fmt::Display for MethodFrozenHeapName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "methods({})", self.name)
    }
}

/// Used to build a [`Methods`] value.
#[derive(Debug)]
pub struct MethodsBuilder {
    /// The heap everything is allocated in.
    heap: OwnedFrozenHeap,
    /// Members, either `NativeMethod` or `NativeAttribute`. Allocated in `heap` (or a heap it
    /// references) and stored with the brand erased; `Methods` documents how they are read.
    members: SymbolMap<UnboundValue<'static>>,
    /// The raw docstring for the main object.
    ///
    /// FIXME(JakobDegen): This should probably be removed. Not only can these docstrings not be
    /// combined with each other, but having the main documentation for the object on the methods
    /// instead of on the object type directly is extraordinarily confusing.
    docstring: Option<String>,
    /// Heap name for identification in heap graph tracking.
    heap_name: Option<MethodFrozenHeapName>,
}

impl Methods {
    pub(crate) fn get<'v>(&'static self, name: &str) -> Option<Value<'v>> {
        Some(self.members.get_str(name)?.at().to_value())
    }

    /// Gets the type of the member
    ///
    /// In the case of an attribute, this is the type the attribute evaluates to, while in the case
    /// of a method, this is the `TyCallable`
    pub(crate) fn get_ty(&'static self, name: &str) -> Option<Ty> {
        match self.members.get_str(name)? {
            UnboundValue::Attr(attr) => Some(attr.typ.dupe()),
            UnboundValue::Method(method) => Some(method.ty.dupe()),
        }
    }

    /// The heap that owns the values in these methods.
    pub fn heap(&self) -> OwnedFrozenRef<'_, ()> {
        self.heap.owner()
    }

    #[inline]
    pub(crate) fn get_hashed(
        &'static self,
        name: Hashed<&str>,
    ) -> Option<&'static UnboundValue<'static>> {
        self.members.get_hashed_str(name)
    }

    #[inline]
    pub(crate) fn get_frozen_symbol(
        &'static self,
        name: &Symbol,
    ) -> Option<&'static UnboundValue<'static>> {
        self.members.get(name)
    }

    pub(crate) fn names(&self) -> Vec<String> {
        self.members.keys().map(|x| x.as_str().to_owned()).collect()
    }

    pub(crate) fn members<'v>(&'static self) -> impl Iterator<Item = (&'static str, Value<'v>)> {
        self.members
            .iter()
            .map(|(k, v)| (k.as_str(), v.at().to_value()))
    }

    /// Fetch the documentation.
    pub fn documentation(&'static self, ty: Ty) -> DocType {
        let (docs, members) = common_documentation(
            &self.docstring,
            self.members
                .iter()
                .map(|(n, v)| (n.as_str(), v.at().to_value())),
        );

        DocType {
            docs,
            members: members
                .filter_map(|(n, item)| {
                    // This is only `None` if the item is a module, but types shouldn't really have
                    // modules in them anyway, so that seems ok
                    Some((n, item.try_as_member_with_collapsed_object().ok()?))
                })
                .collect(),
            ty,
            constructor: None,
        }
    }
}

impl Methods {
    /// Create an empty [`Methods`], with no functions in scope.
    pub fn new() -> Self {
        MethodsBuilder::new().build()
    }
}

impl MethodsBuilder {
    /// Create an empty [`MethodsBuilder`], with no functions in scope.
    pub fn new() -> Self {
        MethodsBuilder {
            heap: OwnedFrozenHeap::new(),
            members: SymbolMap::new(),
            docstring: None,
            heap_name: None,
        }
    }

    /// Called at the end to build a [`Methods`].
    pub fn build(self) -> Methods {
        let heap = self
            .heap
            .seal_impl(self.heap_name.map(FrozenHeapName::Method), None);
        Methods {
            heap,
            members: self.members,
            docstring: self.docstring,
        }
    }

    /// A fluent API for modifying [`MethodsBuilder`] and returning the result.
    pub fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }

    /// Set the raw docstring for this object.
    pub fn set_docstring(&mut self, docstring: &str) {
        self.docstring = Some(docstring.to_owned());
    }

    /// Set a constant value in the [`MethodsBuilder`] that will be suitable for use with
    /// [`StarlarkValue::get_methods`](crate::values::StarlarkValue::get_methods).
    pub fn set_attribute<'v, V: for<'fv> AllocFrozenValue<'fv>>(
        &'v mut self,
        name: &str,
        value: V,
        docstring: Option<String>,
    ) {
        let typ =
            <<V as AllocFrozenValue<'static>>::Type as StarlarkTypeExpr<'static>>::starlark_type_repr();
        self.set_attribute_with(name, typ, docstring, |heap| heap.alloc(value).to_value())
    }

    /// Set a constant value that is a representation, allocated the way it chose to be.
    pub fn set_attribute_representation<'v, B>(
        &'v mut self,
        name: &str,
        builder: B,
        docstring: Option<String>,
    ) where
        B: for<'fv> AllocFrozenRepresentation<'fv>,
    {
        let typ =
            <<B as AllocFrozenRepresentation<'static>>::Repr as StarlarkValue<'static>>::get_type_starlark_repr();
        self.set_attribute_with(name, typ, docstring, |heap| {
            heap.alloc_representation(builder).to_value()
        })
    }

    fn set_attribute_with(
        &mut self,
        name: &str,
        typ: Ty,
        docstring: Option<String>,
        alloc: impl for<'fv> FnOnce(FrozenHeap<'fv>) -> Value<'fv>,
    ) {
        // We want to build an attribute, that ignores its self argument, and does no subsequent allocation.
        let attr = self.heap.with(|heap| {
            let value = alloc(heap);
            let attr = heap.alloc_simple_typed(NativeAttribute {
                speculative_exec_safe: true,
                docstring,
                typ,
                data: Some(value),
                // SAFETY: Set to `Some` immediately above
                callable: |value, _, _| Ok(unsafe { value.unwrap_unchecked() }),
            });
            let attr = FrozenValueTyped::from_typed(attr).expect("allocated in a frozen heap");
            // SAFETY: Allocated in `self.heap` just above.
            unsafe { erase_member(UnboundValue::Attr(attr)) }
        });
        self.members.insert(name, attr);
    }

    /// Set an attribute. Only used by `starlark_module` macro
    #[doc(hidden)]
    pub fn set_attribute_fn(
        &mut self,
        name: &str,
        speculative_exec_safe: bool,
        docstring: Option<String>,
        typ: Ty,
        // The first argument is always `None`
        f: for<'v> fn(Option<Value<'v>>, Value<'v>, Heap<'v>) -> crate::Result<Value<'v>>,
    ) {
        let attr = self.heap.with(|heap| {
            let attr = heap.alloc_simple_typed(NativeAttribute {
                speculative_exec_safe,
                docstring,
                typ,
                data: None,
                callable: f,
            });
            let attr = FrozenValueTyped::from_typed(attr).expect("allocated in a frozen heap");
            // SAFETY: Allocated in `self.heap` just above.
            unsafe { erase_member(UnboundValue::Attr(attr)) }
        });
        self.members.insert(name, attr);
    }

    /// Set a method. Only used by `starlark_module` macro
    ///
    /// `sig` builds the signature on the heap of this builder, so that its default values live
    /// there.
    #[doc(hidden)]
    pub fn set_method(
        &mut self,
        name: &str,
        components: NativeCallableComponents,
        sig: impl for<'fh> FnOnce(FrozenHeap<'fh>) -> ParametersSpec<Value<'fh>>,
        f: NativeMethFn,
    ) {
        let ty = components.make_type(None);

        let method = self.heap.with(|heap| {
            let method = heap.alloc_simple_typed(NativeMethod {
                function: NativeMeth(f, sig(heap)),
                name: name.to_owned(),
                speculative_exec_safe: components.speculative_exec_safe,
                docs: components.into_docs(None, heap),
                ty,
            });
            let method = FrozenValueTyped::from_typed(method).expect("allocated in a frozen heap");
            // SAFETY: Allocated in `self.heap` just above.
            unsafe { erase_member(UnboundValue::Method(method)) }
        });
        self.members.insert(name, method);
    }
}

/// Lazy, named cache for a [`Methods`] value. Created via the
/// [`methods_static!`](crate::methods_static) macro; the methods are built on
/// first access via the supplied initializer.
///
/// ```ignore
/// fn my_methods(builder: &mut MethodsBuilder) { ... }
///
/// starlark::methods_static!(MY_METHODS = my_methods);
///
/// impl StarlarkValue<'_> for Foo {
///     fn get_methods() -> Option<&'static Methods> {
///         Some(MY_METHODS.methods())
///     }
/// }
/// ```
pub struct MethodsStatic {
    cell: OnceLock<Methods>,
    name: StaticStr,
    init: fn(&mut MethodsBuilder),
}

impl MethodsStatic {
    /// Create a new [`MethodsStatic`]. Prefer the
    /// [`methods_static!`](crate::methods_static) macro, which fills in `name`
    /// from the call site.
    pub const fn new(name: StaticStr, init: fn(&mut MethodsBuilder)) -> MethodsStatic {
        MethodsStatic {
            cell: OnceLock::new(),
            name,
            init,
        }
    }

    /// Get (or build, on first call) the [`Methods`] value.
    pub fn methods(&'static self) -> &'static Methods {
        self.cell.get_or_init(|| {
            let mut builder = MethodsBuilder::new();
            builder.heap_name = Some(MethodFrozenHeapName { name: self.name });
            (self.init)(&mut builder);
            builder.build()
        })
    }

    /// Copy all the methods into another builder. The methods' values stay
    /// owned by the static's heap; `out`'s heap takes a reference so the
    /// dependency is recorded for downstream consumers (e.g. pagable
    /// serialization).
    pub fn populate(&'static self, out: &mut MethodsBuilder) {
        let methods = self.methods();
        for (name, value) in methods.members.iter() {
            out.members.insert(name.as_str(), *value);
        }
        out.heap
            .with(|heap| heap.add_reference(methods.heap.owner()));
        out.docstring = methods.docstring.clone();
    }
}

/// Forget the brand of a member, for storage in a [`MethodsBuilder`].
///
/// # SAFETY
///
/// `member` must be allocated in the builder's heap.
unsafe fn erase_member<'fh>(member: UnboundValue<'fh>) -> UnboundValue<'static> {
    // SAFETY: The builder's heap ends up as the `Methods`' heap, which keeps the member alive.
    unsafe { OwnedFrozen::<UnboundValue<'static>>::erase_brand(member) }
}

/// Define a `static` of type [`MethodsStatic`] backed by an init function. The
/// heap is named `<module_path>::<NAME>`.
///
/// ```ignore
/// fn my_methods(builder: &mut MethodsBuilder) { ... }
///
/// starlark::methods_static!(MY_METHODS = my_methods);
/// ```
///
/// Or with an inline closure:
///
/// ```ignore
/// starlark::methods_static!(RES = |b| {
///     b.set_attribute("foo", 42, None);
/// });
/// ```
#[macro_export]
macro_rules! methods_static {
    ($vis:vis $name:ident = $init:expr) => {
        $vis static $name: $crate::__derive_refs::MethodsStatic = {
            $crate::__derive_refs::static_str!(
                __METHOD_HEAP_NAME =
                concat!(module_path!(), "::", stringify!($name))
            );
            $crate::__derive_refs::MethodsStatic::new(__METHOD_HEAP_NAME, $init)
        };

        $crate::__derive_refs::inventory::submit! {
            $crate::__derive_refs::StaticHeapEntry {
                file: file!(),
                line: line!(),
                get_heap: || $name.methods().heap(),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use derive_more::Display;
    use starlark_derive::NoSerialize;
    use starlark_derive::ProvidesStaticType;
    use starlark_derive::StarlarkPagable;
    use starlark_derive::starlark_value;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::Methods;
    use crate::starlark_simple_value;
    use crate::values::StarlarkValue;

    #[test]
    fn test_set_attribute() {
        #[derive(
            Debug,
            Display,
            ProvidesStaticType,
            NoSerialize,
            Allocative,
            StarlarkPagable
        )]
        #[display("Magic")]
        struct Magic;
        starlark_simple_value!(Magic);

        starlark::methods_static!(
            RES = |x| {
                x.set_attribute("my_type", "magic", None);
                x.set_attribute("my_value", 42, None);
            }
        );

        #[starlark_value(type = "magic")]
        impl<'v> StarlarkValue<'v> for Magic {
            fn get_methods() -> Option<&'static Methods> {
                Some(RES.methods())
            }
        }

        let mut a = Assert::new();
        a.globals_add(|x| x.set_representation("magic", Magic));
        a.pass(
            r#"
assert_eq(magic.my_type, "magic")
assert_eq(magic.my_value, 42)"#,
        );
    }
}
