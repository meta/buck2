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

//! The values module define a trait `StarlarkValue` that defines the attribute of
//! any value in Starlark and a few macro to help implementing this trait.
//! The `Value` struct defines the actual structure holding a StarlarkValue. It is
//! mostly used to enable mutable and Rc behavior over a StarlarkValue.
//! This modules also defines this traits for the basic immutable values: int,
//! bool and NoneType. Sub-modules implement other common types of all Starlark
//! dialect.
//!
//! __Note__: we use _sequence_, _iterable_ and _indexable_ according to the
//! definition in the [Starlark specification](
//! https://github.com/bazelbuild/starlark/blob/master/spec.md#sequence-types).
//! We also use the term _container_ for denoting any of those type that can
//! hold several values.

use std::cmp::Ordering;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;

use allocative::Allocative;
use erased_serde::Serialize;
use starlark_derive::starlark_internal_vtable;
use starlark_map::StarlarkHashValue;

use crate::any::ProvidesStaticType;
use crate::collections::Hashed;
use crate::collections::StarlarkHasher;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocProperty;
use crate::environment::Methods;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::private::Private;
use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::TypingBinOp;
use crate::values::FreezeDynamic;
use crate::values::Heap;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueTyped;
use crate::values::demand::Demand;
use crate::values::error::ControlError;
use crate::values::function::FUNCTION_TYPE;
use crate::values::layout::vtable::StarlarkValueRawPtr;

/// A trait for values which are more complex - because they are either mutable
/// (e.g. using [`RefCell`](std::cell::RefCell)), or contain references to other values.
///
/// A `ComplexValue` is allocated with
/// [`alloc_complex`](crate::values::Heap::alloc_complex). Types whose only
/// lifetime parameter is the heap brand can derive
/// [`Freeze`](crate::values::Freeze) and let
/// [`starlark_complex_value!`](crate::starlark_complex_value!) write the
/// boilerplate; the blanket [`FreezeDynamic`](crate::values::FreezeDynamic) implementation
/// carries them through the same freeze protocol.
///
/// ## Types containing [`Value`]
///
/// A Starlark type containing values is one type constructor instantiated at
/// different heap brands: unfrozen at a mutable heap's brand, frozen at a
/// frozen heap's. If we are defining the type containing a single value,
/// let's call it `One`, we define it once over its brand and freeze
/// `One<'v>` into `One<'fv>` with
/// [`Freeze`](crate::values::Freeze):
///
/// ```
/// use allocative::Allocative;
/// use derive_more::Display;
/// use starlark::values::Freeze;
/// use starlark::values::FreezeResult;
/// use starlark::values::Freezer;
/// use starlark::values::NoSerialize;
/// use starlark::values::ProvidesStaticType;
/// use starlark::values::StarlarkPagable;
/// use starlark::values::StarlarkValue;
/// use starlark::values::Trace;
/// use starlark::values::Value;
/// use starlark_derive::starlark_value;
///
/// #[derive(
///     Debug,
///     Trace,
///     Display,
///     ProvidesStaticType,
///     NoSerialize,
///     StarlarkPagable,
///     Allocative
/// )]
/// struct One<'v>(Value<'v>);
///
/// #[starlark_value(type = "one", frozen_vtable)]
/// impl<'v> StarlarkValue<'v> for One<'v> {}
///
/// impl<'v> Freeze<'v> for One<'v> {
///     type Frozen<'fv> = One<'fv>;
///     fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
///         Ok(One(freezer.freeze(self.0)?))
///     }
/// }
/// ```
///
/// If the difference between frozen and non-frozen does not follow this pattern — for
/// example your type contains no [`Value`]s and implements this trait for mutability, or
/// it holds a [`Cell`](std::cell::Cell) when non-frozen and a direct value when frozen —
/// write the types and trait instances you need manually.
pub trait ComplexValue<'v>: StarlarkValue<'v> + Trace<'v> + FreezeDynamic<'v> {}

impl<'v, V> ComplexValue<'v> for V where V: StarlarkValue<'v> + Trace<'v> + FreezeDynamic<'v> {}

/// How to put a Rust values into [`Value`]s.
///
/// Every Rust value stored in a [`Value`] must implement this trait.
///
/// A type that contains nested Starlark [`Value`]s, or that is not [`Send`] and [`Sync`] because
/// it has interior mutability such as a [`RefCell`](std::cell::RefCell), additionally needs
/// [`Trace`](crate::values::Trace) and [`Freeze`](crate::values::Freeze), and is
/// allocated with [`alloc_complex`](Heap::alloc_complex) — see
/// [`starlark_complex_value!`](crate::starlark_complex_value!), which writes the
/// boilerplate for the common shape.
///
/// A value's behaviour, from `type()` to iteration, is its type's and lives on
/// [`StarlarkType`]; this trait says that a Rust type is a representation of one, and which.
/// Implement it with the [`starlark_value`](crate::values::starlark_value) proc macro, which
/// writes this trait's items and makes the methods in the block the type's `StarlarkType`
/// impl:
///
/// ```
/// use allocative::Allocative;
/// # use starlark::starlark_simple_value;
/// use derive_more::Display;
/// use starlark::values::NoSerialize;
/// use starlark::values::ProvidesStaticType;
/// use starlark::values::StarlarkPagable;
/// use starlark::values::StarlarkValue;
/// use starlark_derive::starlark_value;
///
/// #[derive(
///     Debug,
///     Display,
///     ProvidesStaticType,
///     NoSerialize,
///     StarlarkPagable,
///     Allocative
/// )]
/// #[display("Foo")]
/// struct Foo;
/// # starlark_simple_value!(Foo);
/// #[starlark_value(type = "foo")]
/// impl<'v> StarlarkValue<'v> for Foo {}
/// ```
///
/// Every method of [`StarlarkType`] added to the block enables a further feature in Starlark.
/// In most cases the default implementation returns an "unimplemented" [`Err`].
///
/// # How a value is stored
///
/// A value is allocated on a heap as a header, which points at a vtable generated from this
/// trait, followed by the Rust value itself. [`Value`]'s methods dispatch through the vtable to
/// the methods below. Which heaps a type can be allocated on, and what happens to it when its
/// module is frozen, follows from what it contains:
///
/// * A *simple* value needs neither tracing nor freezing. On a [`Heap`] that means it holds no
///   [`Value`]s at all and is `Send + Sync + 'static` ([`Heap::alloc_simple`]); on a
///   [`FrozenHeap`](crate::values::FrozenHeap) it may hold values at that heap's brand, since
///   nothing there is ever traced or moved ([`FrozenHeap::alloc_simple`](crate::values::FrozenHeap::alloc_simple)). Freezing copies a
///   simple value to the frozen heap as it is.
/// * A *complex* value holds [`Value`]s of an unfrozen heap, or interior mutability. It
///   implements [`Trace`](crate::values::Trace), so the garbage collector can find what it
///   points at, and [`Freeze`](crate::values::Freeze), whose output is the simple
///   value that stands in for it on the frozen heap ([`Heap::alloc_complex`]).
///   [`Heap::alloc_complex_no_freeze`] is for a type that must be traced but has no frozen form;
///   freezing a module that still holds one is an error.
///
/// The behaviour of a Starlark type: what `type()` says, its methods and attributes, and what
/// its values do when compared, iterated, called, indexed, and so on.
///
/// A type with one Rust representation implements this for that representation, and
/// [`#[starlark_value]`](crate::values::starlark_value) writes the impl from the
/// `impl StarlarkValue` block, so most types never spell this trait. A type with several
/// representations, say a mutable and a frozen layout, implements it once, for a view over its
/// representations, and each representation names the view as its
/// [`Type`](StarlarkValue::Type).
///
/// # Note To Implementors
/// The vtable entry for a method added to this trait is generated by
/// `#[starlark_internal_vtable]`, but [`Value`] reaches it through a forwarding method on
/// `AValueDyn` in `crate::values::layout::vtable`, which has to be written by hand. Without it,
/// implementations other than the default are never run.
#[starlark_internal_vtable]
#[allow(non_upper_case_globals, non_snake_case)] // For generated members.
pub trait StarlarkType<'v>: 'v + ProvidesStaticType<'v> + Debug + Display + Sized {
    /// Return a string describing the type of self, as returned by the type()
    /// function.
    ///
    /// This can be only implemented by the [`#[starlark_value]`](crate::values::starlark_value)
    /// proc macro.
    const TYPE: &'static str;

    /// Like [`TYPE`](Self::TYPE), but returns a reusable [`StringValue`] pointer to it. The
    /// string is a static, which is what the `'static` brand means; use
    /// [`at`](StringValue::at) to bring it to the brand of the heap in use. This function
    /// deliberately doesn't take a heap, as it would not be performant to allocate a new value
    /// each time.
    ///
    /// This can be only implemented by the [`#[starlark_value]`](crate::values::starlark_value)
    /// proc macro.
    fn get_type_value_static() -> StringValue<'static> {
        panic!("This function is implemented by #[starlark_value] proc macro")
    }

    /// What a value of this type is read through: `ValueTyped<'v, Self>` for a type with one
    /// representation, the view itself for a type with several.
    ///
    /// This can be only implemented by the [`#[starlark_value]`](crate::values::starlark_value)
    /// proc macro.
    type Get;

    /// `value` as this type's [`Get`](Self::Get), if it is of this type.
    ///
    /// This can be only implemented by the [`#[starlark_value]`](crate::values::starlark_value)
    /// proc macro.
    #[starlark_internal_vtable(skip)]
    fn get(value: Value<'v>) -> Option<Self::Get>;

    /// The type as a user would write it in a type annotation. This often will be the same as
    /// [`Self::TYPE`], but in some instances it might be slightly different than what is
    /// returned by `TYPE`.
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) generates this, as the nominal type
    /// of the representation, unless the block overrides it.
    fn get_type_starlark_repr() -> Ty;

    /// Get the members associated with this type, accessible via `this_type.x`.
    /// These members will have `dir`/`getattr`/`hasattr` properly implemented,
    /// so it is the preferred way to go if possible.
    ///
    /// `#[starlark_module]` creates a [`MethodsStatic`](crate::environment::MethodsStatic)
    /// named after the initializer function. Attributes and methods can be attached
    /// to a value with that generated static:
    ///
    /// ```ignore
    /// #[starlark_module]
    /// fn foo_methods(builder: &mut MethodsBuilder) {
    ///     #[starlark(attribute)]
    ///     fn name<'v>(this: &'v Foo) -> starlark::Result<&'v str> {
    ///         Ok(this.name.as_str())
    ///     }
    /// }
    ///
    /// #[starlark_value(type = "foo")]
    /// impl<'v> StarlarkValue<'v> for Foo {
    ///     fn get_methods() -> Option<&'static Methods> {
    ///         Some(FOO_METHODS_STATICS.methods())
    ///     }
    /// }
    /// ```
    ///
    /// The generated `FOO_METHODS_STATICS` already owns the method table; do not
    /// also register the initializer with [`methods_static!`](crate::methods_static).
    /// When migrating from `StarlarkAttrs`, remove field-level `#[starlark(skip)]`
    /// and `#[starlark(clone)]` attributes. Omit getters for skipped fields, and
    /// return borrowed or preallocated values instead of cloning expensive fields.
    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        None
    }

    /// Return the documentation for this value.
    ///
    /// This should be the doc-item that is expected to be generated when this value appears as a
    /// global in a module. In other words, for normal types this should generally return a
    /// `DocMember::Property`. In that case there is no need to override this method.
    fn documentation(&self) -> DocItem
    where
        Self: Sized,
    {
        let ty = self
            .typechecker_ty()
            .unwrap_or_else(|| Self::get_type_starlark_repr());
        DocItem::Member(DocMember::Property(DocProperty {
            docs: None,
            typ: ty,
        }))
    }

    /// Type of this instance for typechecker.
    /// Note this can be more precise than generic type.
    fn typechecker_ty(&self) -> Option<Ty> {
        // TODO(nga): replace with `Self::get_type_starlark_repr()`
        //   when it gets implemented properly.
        None
    }

    /// Evaluate this value as a type expression. Basically, `eval_type(this)`.
    #[doc(hidden)]
    fn eval_type(&self) -> Option<Ty> {
        None
    }

    /// Return a string representation of self, as returned by the `repr()` function.
    /// Defaults to the `Display` instance - which should be fine for nearly all types.
    /// In many cases the `repr()` representation will also be a Starlark expression
    /// for creating the value.
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// repr("test") == '"test"'
    /// repr([1,2,3]) == '[1, 2, 3]'
    /// repr([1,[2,3]]) == '[1, [2, 3]]'
    /// repr([1]) == '[1]'
    /// repr([]) == '[]'
    /// # "#);
    /// ```
    fn collect_repr(&self, collector: &mut String) {
        // Rust won't return Err when writing to a String, so safe unwrap
        write!(collector, "{self}").unwrap()
    }

    /// Invoked to print `repr` when a cycle is the object stack is detected.
    fn collect_repr_cycle(&self, collector: &mut String) {
        write!(collector, "<{}...>", Self::TYPE).unwrap()
    }

    /// Return a string representation of self, as returned by the `str()` function.
    /// Defaults to `collect_repr`. Override to provide a different string representation
    /// for `str()` vs `repr()` (e.g. bytes decodes as UTF-8 for `str()`).
    fn collect_str(&self, collector: &mut String) {
        self.collect_repr(collector)
    }

    /// String used when printing call stack. `repr(self)` by default.
    fn name_for_call_stack(&self, me: Value<'v>) -> String {
        me.to_repr()
    }

    /// Convert self to a boolean, as returned by the bool() function.
    /// The default implementation returns [`true`].
    fn to_bool(&self) -> bool {
        // Return `true` by default, because this is default when implementing
        // custom types in Python: https://docs.python.org/release/2.5.2/lib/truth.html
        true
    }

    /// Return a hash data for self to be used when self is placed as a key in a `Dict`.
    /// Return an [`Err`] if there is no hash for this value (e.g. list).
    /// Must be stable between frozen and non-frozen values.
    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        if Self::TYPE == FUNCTION_TYPE {
            // The Starlark spec says values of type "function" must be hashable.
            // We could return the address of the function, but that changes
            // with frozen/non-frozen which breaks freeze for Dict.
            // We could create an atomic counter and use that, but it takes memory,
            // effort, complexity etc, and we don't expect many Dict's keyed by
            // function. Returning 0 as the hash is valid, as Eq will sort it out.
            let _ = hasher;
            Ok(())
        } else {
            Err(crate::Error::new_other(ControlError::NotHashableValue(
                Self::TYPE.to_owned(),
            )))
        }
    }

    /// Get the hash value. Calls [`write_hash`](Self::write_hash) by default.
    #[doc(hidden)]
    fn get_hash(&self, _private: Private) -> crate::Result<StarlarkHashValue> {
        let mut hasher = StarlarkHasher::new();
        self.write_hash(&mut hasher)?;
        Ok(hasher.finish_short())
    }

    /// Compare `self` with `other` for equality.
    /// Should only return an error on excessive recursion.
    ///
    /// This function can only be called when it is known that self pointer
    /// is not equal to the other pointer. Thus, an implementation may assume
    /// that the pointers are not equal. Implementation of `equals` for some
    /// builtin types and default implementation rely on this assumption.
    ///
    /// Equality must be symmetric (`a == b` implies `b == a`).
    /// When evaluating `a == b` (or when using equality in dicts and such),
    /// it is not specified whether `a.equals(b)` or `b.equals(a)` is called.
    fn equals(&self, _other: Value<'v>) -> crate::Result<bool> {
        // Type is only equal via a pointer
        Ok(false)
    }

    /// Compare `self` with `other`.
    /// This method returns a result of type [`Ordering`], or an [`Err`]
    /// if the two types differ.
    fn compare(&self, other: Value<'v>) -> crate::Result<Ordering> {
        ValueError::unsupported_with(self, "compare", other)
    }

    /// Directly invoke a function.
    /// The number of `named` and `names` arguments are guaranteed to be equal.
    ///
    /// # Parameters
    ///
    /// * `me` - `self` as the [`Value`] it is stored in, for putting on the call stack or in
    ///   other values.
    fn invoke(
        &self,
        _me: Value<'v>,
        _args: &Arguments<'v, '_>,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        ValueError::unsupported(self, "call()")
    }

    /// Return the result of `a[index]` if `a` is indexable.
    fn at(&self, index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "[]", index)
    }

    /// Return the result of `a[index0, index1]` if `a` is indexable by two parameters.
    fn at2(
        &self,
        _index0: Value<'v>,
        _index1: Value<'v>,
        _heap: Heap<'v>,
        _private: Private,
    ) -> crate::Result<Value<'v>> {
        ValueError::unsupported(self, "[,]")
    }

    /// Extract a slice of the underlying object if the object is indexable. The
    /// result will be object between `start` and `stop` (both of them are
    /// added length() if negative and then clamped between 0 and length()).
    /// `stride` indicates the direction.
    ///
    /// # Parameters
    ///
    /// * start: the start of the slice.
    /// * stop: the end of the slice.
    /// * stride: the direction of slice,
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 'abc'[1:] == 'bc'         # Remove the first element
    /// 'abc'[:-1] == 'ab'        # Remove the last element
    /// 'abc'[1:-1] == 'b'        # Remove the first and the last element
    /// 'abc'[-1:] == 'c'         # Take the last letter
    /// 'abc'[:1] == 'a'          # Take the first letter
    /// 'banana'[1::2] == 'aaa'   # Select one element out of 2, skipping the first
    /// 'banana'[4::-2] == 'nnb'  # Select one element out of 2 in reverse order, starting at index 4
    /// # "#);
    /// ```
    fn slice(
        &self,
        _start: Option<Value<'v>>,
        _stop: Option<Value<'v>>,
        _stride: Option<Value<'v>>,
        _heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        ValueError::unsupported(self, "[::]")
    }

    /// Implement iteration over the value of this container by providing
    /// the values in a `Vec`.
    #[starlark_internal_vtable(skip)]
    fn iterate_collect(&self, _heap: Heap<'v>) -> crate::Result<Vec<Value<'v>>> {
        ValueError::unsupported(self, "(iter)")
    }

    /// Returns an iterator over the value of this container if this value holds
    /// an iterable container.
    ///
    /// **This function is hard to implement correctly.**
    /// For example, returning a list from this function is memory violation,
    /// because the list object acting as an iterator is assumed
    /// to have the iteration lock acquired.
    ///
    /// Consider implementing [`iterate_collect`](Self::iterate_collect) instead
    /// when possible.
    ///
    /// This function calls [`iterate_collect`](Self::iterate_collect) by default.
    ///
    /// Returned iterator value must implement
    /// [`iter_next`](Self::iter_next) and [`iter_stop`](Self::iter_stop).
    /// Default implementations of these functions panic.
    ///
    /// Starlark-rust guarantees that
    /// * `iter_next` and `iter_stop` are only called on the value returned from `iterate`
    /// * `iter_next` is called only before `iter_stop`
    /// * `iter_stop` is called exactly once
    ///
    /// So implementations of iterators may acquire mutation lock in `iterate`,
    /// assume that it is held in `iter_next`, and release it in `iter_stop`.
    /// Obviously, there are no such guarantees if these functions are called directly.
    unsafe fn iterate(&self, _me: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(heap.alloc_tuple(&self.iterate_collect(heap)?))
    }

    /// Returns the size hint for the iterator.
    unsafe fn iter_size_hint(&self, _index: usize) -> (usize, Option<usize>) {
        (0, None)
    }

    /// Yield the next value from the iterator.
    ///
    /// This function is called on the iterator value returned by [`iterate`](Self::iterate).
    /// This function accepts an index, which starts at 0 and is incremented by 1
    /// for each call to `iter_next`. The index can be used to implement
    /// cheap iteration over simple containers like lists:
    /// list [`iterate`](Self::iterate) just returns the list itself,
    /// and the passed index is used to access the list elements.
    ///
    /// Default implementation panics.
    ///
    /// This function is only called before [`iter_stop`](Self::iter_stop).
    unsafe fn iter_next(&self, _index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
        panic!(
            "iter_next called on non-iterable value of type {}",
            Self::TYPE
        )
    }

    /// Indicate that the iteration is finished.
    ///
    /// This function is typically used to release mutation lock.
    /// The function must be implemented for iterators even if it does nothing.
    ///
    /// This function is called exactly once for the iterator.
    unsafe fn iter_stop(&self) {
        panic!(
            "iter_stop called on non-iterable value of type {}",
            Self::TYPE
        )
    }

    /// Returns the length of the value, if this value is a sequence.
    fn length(&self) -> crate::Result<i32> {
        ValueError::unsupported(self, "len()")
    }

    /// Attribute type, for the typechecker.
    ///
    /// If [`get_attr`](StarlarkType::get_attr) is implemented,
    /// `#[starlark_value]` proc macro will generate this to return `Some(Any)`.
    fn attr_ty(_name: &str) -> Option<Ty> {
        Some(Ty::any())
    }

    /// Get an attribute for the current value as would be returned by dotted
    /// expression (i.e. `a.attribute`).
    ///
    /// The three methods [`get_attr`](StarlarkType::get_attr),
    /// [`has_attr`](StarlarkType::has_attr) and [`dir_attr`](StarlarkType::dir_attr)
    /// must be consistent - if you implement one, you should probably implement all three.
    ///
    /// This operations must have no side effects, because it can be called speculatively.
    fn get_attr(&self, _attribute: &str, _heap: Heap<'v>) -> Option<Value<'v>> {
        None
    }

    /// A version of `get_attr` which takes `BorrowHashed<str>` instead of `&str`,
    /// thus implementation may reuse the hash of the string if this is called
    /// repeatedly with the same string.
    ///
    /// This function is optional, but if it is implemented, it must be consistent with
    /// [`get_attr`](Self::get_attr).
    fn get_attr_hashed(&self, attribute: Hashed<&str>, heap: Heap<'v>) -> Option<Value<'v>> {
        self.get_attr(attribute.key(), heap)
    }

    /// Return true if an attribute of name `attribute` exists for the current
    /// value.
    ///
    /// The three methods [`get_attr`](StarlarkType::get_attr),
    /// [`has_attr`](StarlarkType::has_attr) and [`dir_attr`](StarlarkType::dir_attr)
    /// must be consistent.
    ///
    /// Default implementation of this function delegates to [`get_attr`](Self::get_attr).
    fn has_attr(&self, attribute: &str, heap: Heap<'v>) -> bool {
        self.get_attr(attribute, heap).is_some()
    }

    /// Return a vector of string listing all attribute of the current value.
    ///
    /// The three methods [`get_attr`](StarlarkType::get_attr),
    /// [`has_attr`](StarlarkType::has_attr) and [`dir_attr`](StarlarkType::dir_attr)
    /// must be consistent - if you implement one, you should probably implement all three.
    fn dir_attr(&self) -> Vec<String> {
        Vec::new()
    }

    /// Tell whether `other` is in the current value, if it is a container.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// ('a' in 'abc') == True
    /// ('b' in 'abc') == True
    /// ('z' in 'abc') == False
    /// # "#);
    /// ```
    fn is_in(&self, other: Value<'v>) -> crate::Result<bool> {
        ValueError::unsupported_owned(other.get_type(), "in", Some(Self::TYPE))
    }

    /// Apply the `+` unary operator to the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// +1 == 1
    /// # "#);
    /// ```
    fn plus(&self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported(self, "+")
    }

    /// Apply the `-` unary operator to the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// -(1) == -1
    /// # "#);
    /// ```
    fn minus(&self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported(self, "-")
    }

    /// Add with the arguments the other way around.
    /// Normal `add` should return `None` in order for it to be evaluated.
    fn radd(&self, _lhs: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        None
    }

    /// Add `rhs` to the current value. Should return [`None`]
    /// to fall through to `radd`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 1 + 2 == 3
    /// [1, 2, 3] + [2, 3] == [1, 2, 3, 2, 3]
    /// 'abc' + 'def' == 'abcdef'
    /// (1, 2, 3) + (2, 3) == (1, 2, 3, 2, 3)
    /// # "#);
    /// ```
    fn add(&self, _rhs: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        None
    }

    /// Subtract `other` from the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 1 - 2 == -1
    /// # "#);
    /// ```
    fn sub(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "-", other)
    }

    /// Called on `rhs` of `lhs * rhs` when `lhs.mul` returns `None`.
    fn rmul(&self, lhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        let _ignore = (lhs, heap);
        None
    }

    /// Multiply the current value with `other`.
    ///
    /// When this function returns `None`, starlark-rust calls `rhs.rmul(lhs)`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 2 * 3 == 6
    /// [1, 2, 3] * 3 == [1, 2, 3, 1, 2, 3, 1, 2, 3]
    /// 'abc' * 3 == 'abcabcabc'
    /// (1, 2, 3) * 3 == (1, 2, 3, 1, 2, 3, 1, 2, 3)
    /// # "#);
    /// ```
    fn mul(&self, _rhs: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        None
    }

    /// Divide the current value by `other`. Always results in a float value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 4 / 2.0 == 2.0
    /// 7 / 2 == 3.5
    /// # "#);
    /// ```
    fn div(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "/", other)
    }

    /// Apply the percent operator between the current value and `other`. Usually used on
    /// strings, as per
    /// [the Starlark spec](https://github.com/bazelbuild/starlark/blob/master/spec.md#string-interpolation).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 5 % 3 == 2
    /// -5 % -3 == -2
    /// 5 % -3 == -1
    /// -5 % 3 == 1
    /// 5.5 % 3.0 == 2.5
    /// -5.5 % 3.0 == 0.5
    /// 5.5 % -3.0 == -0.5
    /// -5.5 % -3.0 == -2.5
    /// "a %s c" % 3 == "a 3 c"
    /// "Hello %s, your score is %d" % ("Bob", 75) == "Hello Bob, your score is 75"
    /// "%d %o %x" % (65, 65, 65) == "65 101 41"
    /// "%d" % 12345678901234567890 == "12345678901234567890"
    /// "Hello %s, welcome" % "Bob" == "Hello Bob, welcome"
    /// "%s" % (1,) == "1"
    /// "%s" % ((1,),) == "(1,)"
    /// "%s" % [1] == "[1]"
    /// "test" % () == "test"
    /// # "#);
    /// ```
    fn percent(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "%", other)
    }

    /// Floor division between the current value and `other`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 7 // 2 == 3
    /// -7 // -2 == 3
    /// 7 // -2 == -4
    /// -7 // 2 == -4
    /// 7.0 // 2.0 == 3.0
    /// -7.0 // -2.0 == 3.0
    /// 7.0 // -2.0 == -4.0
    /// -7.0 // 2.0 == -4.0
    /// 3.0 // 2.0 == 1.0
    /// # "#);
    /// ```
    fn floor_div(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "//", other)
    }

    /// Bitwise `&` operator.
    fn bit_and(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "&", other)
    }

    /// Bitwise `|` operator.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # starlark::assert::all_true(r#"
    /// 0xb00f | 0x0ee0 == 0xbeef
    /// 4 | 7 == 7
    /// {1: 2} | {3: 4} == {1: 2, 3: 4}
    /// {1: 2} | {1: 3} == {1: 3}
    /// # "#);
    /// ```
    fn bit_or(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "|", other)
    }

    /// Bitwise `^` operator.
    fn bit_xor(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "^", other)
    }

    /// Bitwise `~` operator.
    fn bit_not(&self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported(self, "~")
    }

    /// Bitwise `<<` operator.
    fn left_shift(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, "<<", other)
    }

    /// Bitwise `>>` operator.
    fn right_shift(&self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with(self, ">>", other)
    }

    /// Typecheck `this op rhs`.
    fn bin_op_ty(_op: TypingBinOp, _rhs: &TyBasic) -> Option<Ty> {
        None
    }

    /// Typecheck `lhs op this`.
    fn rbin_op_ty(_lhs: &TyBasic, _op: TypingBinOp) -> Option<Ty> {
        None
    }

    /// Called when exporting a value under a specific name,
    fn export_as(
        &self,
        _variable_name: &str,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<()> {
        // Most data types ignore how they are exported
        // but rules/providers like to use it as a helpful hint for users
        Ok(())
    }

    /// Set the value at `index` with the new value.
    ///
    /// ```rust
    /// # starlark::assert::is_true(r#"
    /// v = [1, 2, 3]
    /// v[1] = 1
    /// v[2] = [2,3]
    /// v == [1, 1, [2, 3]]
    /// # "#);
    /// ```
    fn set_at(&self, _index: Value<'v>, _new_value: Value<'v>) -> crate::Result<()> {
        Err(crate::Error::new_other(
            ValueError::CannotMutateImmutableValue,
        ))
    }

    /// Set the attribute named `attribute` of the current value to
    /// `value` (e.g. `a.attribute = value`).
    fn set_attr(&self, attribute: &str, _new_value: Value<'v>) -> crate::Result<()> {
        ValueError::unsupported(self, &format!(".{attribute}="))
    }
}

/// [`FrozenHeap::alloc_simple`]: crate::values::FrozenHeap::alloc_simple
pub trait StarlarkValue<'v>:
    'v + ProvidesStaticType<'v> + Allocative + Debug + Display + Serialize + Sized
{
    /// The Starlark type this is a representation of: `Self` for a type with one
    /// representation, otherwise the view the type implements [`StarlarkType`] for.
    type Type: StarlarkType<'v>;

    /// Call `f` on this value as the receiver of its type's methods: the payload itself for a
    /// type with one representation, the view a type with several builds here.
    fn with_type<R>(this: ValueTyped<'v, Self>, f: impl FnOnce(&Self::Type) -> R) -> R;

    /// [`with_type`](Self::with_type) for a caller that already holds the payload, which is
    /// every vtable entry; a type with one representation hands it straight to `f` instead of
    /// deriving it from the value again.
    #[doc(hidden)]
    #[inline]
    fn with_type_at<R>(
        payload: &'v Self,
        this: ValueTyped<'v, Self>,
        f: impl FnOnce(&Self::Type) -> R,
    ) -> R {
        let _ = payload;
        Self::with_type(this, f)
    }

    /// [`StarlarkType::TYPE`] of [`Type`](Self::Type), for code generic over representations.
    const TYPE: &'static str = <Self::Type as StarlarkType<'v>>::TYPE;

    /// [`StarlarkType::get_type_value_static`] of [`Type`](Self::Type).
    fn get_type_value_static() -> StringValue<'static> {
        <Self::Type as StarlarkType<'v>>::get_type_value_static()
    }

    /// [`StarlarkType::get_type_starlark_repr`] of [`Type`](Self::Type).
    fn get_type_starlark_repr() -> Ty {
        <Self::Type as StarlarkType<'v>>::get_type_starlark_repr()
    }

    /// [`StarlarkType::get_methods`] of [`Type`](Self::Type).
    fn get_methods() -> Option<&'static Methods> {
        <Self::Type as StarlarkType<'v>>::get_methods()
    }

    /// [`StarlarkType::attr_ty`] of [`Type`](Self::Type).
    fn attr_ty(name: &str) -> Option<Ty> {
        <Self::Type as StarlarkType<'v>>::attr_ty(name)
    }

    /// [`StarlarkType::bin_op_ty`] of [`Type`](Self::Type).
    fn bin_op_ty(op: TypingBinOp, rhs: &TyBasic) -> Option<Ty> {
        <Self::Type as StarlarkType<'v>>::bin_op_ty(op, rhs)
    }

    /// [`StarlarkType::rbin_op_ty`] of [`Type`](Self::Type).
    fn rbin_op_ty(lhs: &TyBasic, op: TypingBinOp) -> Option<Ty> {
        <Self::Type as StarlarkType<'v>>::rbin_op_ty(lhs, op)
    }

    /// Please do not implement this method or `get_type`,
    /// but use [`#[starlark_value]`](crate::values::starlark_value) proc macro.
    #[doc(hidden)]
    fn please_use_starlark_type_macro() {
        panic!("This function is implemented by #[starlark_value] proc macro")
    }

    /// Type is special in Starlark, it is implemented differently than user defined types.
    /// For example, some special types like `bool` cannon be heap allocated.
    ///
    /// This function must not be implemented outside of starlark crate.
    #[doc(hidden)]
    fn is_special(_private: Private) -> bool {
        false
    }

    /// Dynamically provide values based on type.
    ///
    /// Value can be fetched using [`Value::request_value`].
    ///
    /// The API is based on
    /// [std::any::Provider](https://doc.rust-lang.org/std/any/trait.Provider.html).
    ///
    /// This lives on the representation rather than on [`StarlarkType`]: what it provides are
    /// references into the representation's memory, for the heap's lifetime, which the receiver
    /// of a `StarlarkType` method, built for the call, cannot give out.
    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        let _ = demand;
    }

    /// Whether `value` is of the type this value denotes, for the representations of `type`.
    ///
    /// This lives on the representation rather than on [`StarlarkType`]: it runs on every runtime
    /// type check, and a representation of `type` is monomorphised over its matcher, so here the
    /// matcher is called statically where the view of `type` could only call it dynamically.
    #[doc(hidden)]
    fn type_matches_value(&self, _value: Value<'v>, _private: Private) -> bool {
        unreachable!("`type_matches_value` should only be called on special types")
    }
}
