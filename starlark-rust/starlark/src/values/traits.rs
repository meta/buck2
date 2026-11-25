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

use crate::__derive_refs::behaviour::bit_and_unsupported;
use crate::__derive_refs::behaviour::bit_or_unsupported;
use crate::__derive_refs::behaviour::bit_xor_unsupported;
use crate::__derive_refs::behaviour::compare_unsupported;
use crate::__derive_refs::behaviour::div_unsupported;
use crate::__derive_refs::behaviour::floor_div_unsupported;
use crate::__derive_refs::behaviour::left_shift_unsupported;
use crate::__derive_refs::behaviour::percent_unsupported;
use crate::__derive_refs::behaviour::right_shift_unsupported;
use crate::__derive_refs::behaviour::sub_unsupported;
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
use crate::typing::OperatorSignature;
use crate::typing::Ty;
use crate::typing::TypingBinOp;
use crate::typing::TypingUnOp;
use crate::values::FreezeDynamic;
use crate::values::Heap;
use crate::values::StarlarkTypeExpr;
use crate::values::StarlarkTypeExprStable;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueTyped;
use crate::values::error::ControlError;
use crate::values::function::FUNCTION_TYPE;
use crate::values::layout::vtable::StarlarkValueRawPtr;
use crate::values::predicate::AnyRepresentation;
use crate::values::predicate::Membership;
use crate::values::predicate::Memberships;
use crate::values::typing::type_compiled::compiled::TypeCompiledDyn;

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
/// A type is a type expression: [`StarlarkTypeExpr`] is a supertrait, and its
/// [`Get`](StarlarkTypeExpr::Get) and [`get`](StarlarkTypeExpr::get) are how a value is read
/// as the type. `#[starlark_value]` writes that impl next to this one; a block that reads
/// its values in a way of its own writes `type Get` and `fn get` in the block.
///
/// # The receiver
///
/// A method that acts on a value takes it as its first parameter, which this trait declares as
/// the type's [`Get`](StarlarkTypeExpr::Get). In a block the parameter is named `this`, and
/// its type is the author's choice among the conversions from the type that cannot fail, that is, the
/// [`UnpackValue`](crate::values::UnpackValue)s whose `Type` is the type and whose `Error` is
/// uninhabited:
///
/// | Receiver | What the body has |
/// |---|---|
/// | `this: &Self` | The representation, of a type with one. It lives as long as the heap. |
/// | `this: Self` | The view, of a type with several representations. |
/// | `this: ValueOf<'v, Self>` | The value, to return or to keep. |
/// | `this: ValueTyped<'v, Self>` | The value of a type with one representation, as a handle that derefs to the representation. |
/// | `this: Self::Get` | What this trait declares, for a body that calls another method of the trait on the same value. |
///
/// Nothing is checked to bind it: a method is only called for a value of its type. A block may
/// also still write `&self`, which is the representation or, for a type with several, the
/// view.
///
/// The methods are associated functions. Rust code that has a [`Value`] calls the method of
/// the same name on it, which dispatches here; a method cannot be called on a representation
/// that is not on a heap.
///
/// # Note To Implementors
/// The vtable entry for a method added to this trait is generated by
/// `#[starlark_internal_vtable]`, but [`Value`] reaches it through a forwarding method on
/// `AValueDyn` in `crate::values::layout::vtable`, which has to be written by hand. Without it,
/// implementations other than the default are never run.
#[starlark_internal_vtable]
#[allow(non_upper_case_globals, non_snake_case)] // For generated members.
pub trait StarlarkType<'v>:
    'v
    + StarlarkTypeExpr<'v, Get: Copy + Into<Value<'v>>>
    + StarlarkTypeExprStable<'v>
    + ProvidesStaticType<'v>
    + Debug
    + Display
    + Sized
{
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
    fn documentation(this: Self::Get) -> DocItem
    where
        Self: Sized,
    {
        let ty = Self::typechecker_ty(this).unwrap_or_else(|| Self::get_type_starlark_repr());
        DocItem::Member(DocMember::Property(DocProperty {
            docs: None,
            typ: ty,
        }))
    }

    /// Type of this instance for typechecker.
    /// Note this can be more precise than generic type.
    fn typechecker_ty(_this: Self::Get) -> Option<Ty> {
        // TODO(nga): replace with `Self::get_type_starlark_repr()`
        //   when it gets implemented properly.
        None
    }

    /// Evaluate this value as a type expression. Basically, `eval_type(this)`.
    #[doc(hidden)]
    fn eval_type(_this: Self::Get) -> Option<Ty> {
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
    fn collect_repr(this: Self::Get, collector: &mut String) {
        // Rust won't return Err when writing to a String, so safe unwrap
        // The representation's `Display`. `Value`'s own `Display` is this behind the guard
        // against cycles, which the caller of this method already holds.
        write!(collector, "{}", this.into().get_ref().as_display()).unwrap()
    }

    /// Invoked to print `repr` when a cycle is the object stack is detected.
    fn collect_repr_cycle(_this: Self::Get, collector: &mut String) {
        write!(collector, "<{}...>", Self::TYPE).unwrap()
    }

    /// Return a string representation of self, as returned by the `str()` function.
    /// Defaults to `collect_repr`. Override to provide a different string representation
    /// for `str()` vs `repr()` (e.g. bytes decodes as UTF-8 for `str()`).
    fn collect_str(this: Self::Get, collector: &mut String) {
        Self::collect_repr(this, collector)
    }

    /// String used when printing call stack. `repr(self)` by default.
    fn name_for_call_stack(this: Self::Get) -> String {
        this.into().to_repr()
    }

    /// Convert self to a boolean, as returned by the bool() function.
    /// The default implementation returns [`true`].
    fn to_bool(_this: Self::Get) -> bool {
        // Return `true` by default, because this is default when implementing
        // custom types in Python: https://docs.python.org/release/2.5.2/lib/truth.html
        true
    }

    /// Return a hash data for self to be used when self is placed as a key in a `Dict`.
    /// Return an [`Err`] if there is no hash for this value (e.g. list).
    /// Must be stable between frozen and non-frozen values.
    ///
    /// # In a `#[starlark_value]` block
    ///
    /// A type whose values all hash writes the method without a result:
    ///
    /// ```ignore
    /// fn write_hash(this: &Self, hasher: &mut StarlarkHasher) {
    ///     this.name.hash(hasher)
    /// }
    /// ```
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) derives this method from it, and
    /// implements [`StarlarkTypeExprHashable`](crate::values::StarlarkTypeExprHashable) for the
    /// type. A type with values that do not hash, such as a container that hashes what it
    /// contains, writes this method as it is declared here.
    ///
    /// A value the type holds is hashed through its handle: with
    /// [`ValueOf::write_hash`](crate::values::ValueOf::write_hash) where the type of the part
    /// always hashes, which needs no result here either, and with
    /// [`ValueOf::try_write_hash`](crate::values::ValueOf::try_write_hash) or
    /// [`Value::write_hash`] where it may not.
    fn write_hash(_this: Self::Get, hasher: &mut StarlarkHasher) -> crate::Result<()> {
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
    fn get_hash(this: Self::Get, _private: Private) -> crate::Result<StarlarkHashValue> {
        let mut hasher = StarlarkHasher::new();
        Self::write_hash(this, &mut hasher)?;
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
    ///
    /// # In a `#[starlark_value]` block
    ///
    /// The method is written with the operand it compares with, and the plain result if it
    /// cannot fail:
    ///
    /// ```ignore
    /// fn equals(this: &Self, other: &Self) -> bool {
    ///     this.name == other.name
    /// }
    /// ```
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) derives this method from it.
    ///
    /// * The operand is any [`UnpackValue`](crate::values::UnpackValue): `&Self` for a type
    ///   with one representation; for a type with several, `Self`, the view, or a reference
    ///   the type is read through, such as `&str`; a union
    ///   such as `Either<&Self, &str>`.
    /// * The result is `bool`, or `starlark::Result<bool>` for a body that compares the values
    ///   it contains.
    /// * A value that is not of the operand's type is not equal, as with the default. A value
    ///   that is of the operand's type but does not convert (an `int` that does not fit an
    ///   `i32` operand) is the conversion's error.
    ///
    /// A method is taken to be written as declared here, and left as it is, when its operand's
    /// type is spelled `Value` and its result's `Result`.
    fn equals(_this: Self::Get, _other: Value<'v>) -> crate::Result<bool> {
        // Type is only equal via a pointer
        Ok(false)
    }

    /// Compare `self` with `other`.
    /// This method returns a result of type [`Ordering`], or an [`Err`]
    /// if the two types differ.
    ///
    /// # In a `#[starlark_value]` block
    ///
    /// The method is written as [`equals`](Self::equals) is, with the operand it compares with:
    ///
    /// ```ignore
    /// fn compare(this: &Self, other: &Self) -> Ordering {
    ///     this.name.cmp(&other.name)
    /// }
    /// ```
    ///
    /// The result is [`Ordering`] or `starlark::Result<Ordering>`. A value that is not of the
    /// operand's type gets the default's error, that the two types do not compare. The
    /// operand's type is also what the typechecker takes `<` and its like to accept.
    fn compare(_this: Self::Get, other: Value<'v>) -> crate::Result<Ordering> {
        compare_unsupported::<Self>(other)
    }

    /// Directly invoke a function.
    /// The number of `named` and `names` arguments are guaranteed to be equal.
    ///
    /// A body that puts the function itself on the call stack or into another value takes it
    /// as a receiver that is the value: `this: ValueOf<'v, Self>`, or `ValueTyped<'v, Self>`.
    fn invoke(
        _this: Self::Get,
        _args: &Arguments<'v, '_>,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        ValueError::unsupported_for::<_, Self>("call()")
    }

    /// Return the result of `a[index]` if `a` is indexable.
    ///
    /// # In a `#[starlark_value]` block
    ///
    /// The method is written with the index it takes and what it returns, as an operator is
    /// with its operand and result ([`add`](Self::add)), and may leave out the heap the same
    /// way:
    ///
    /// ```ignore
    /// fn at(this: &Self, index: i32) -> starlark::Result<Value<'v>> {
    ///     this.items.get(index as usize).copied().ok_or_else(|| out_of_bounds(index))
    /// }
    /// ```
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) derives this method from it, and
    /// from its two types what the typechecker knows of `a[index]`
    /// ([`at_signature`](Self::at_signature)).
    ///
    /// A value that is not of the index's type is the error of a conversion that expected one
    /// ("Expected `int`, but got ..") rather than what this default answers: the type is
    /// indexed, and was given the wrong kind of index.
    fn at(_this: Self::Get, index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_with_for::<_, Self>("[]", index)
    }

    /// Return the result of `a[index0, index1]` if `a` is indexable by two parameters.
    fn at2(
        _this: Self::Get,
        _index0: Value<'v>,
        _index1: Value<'v>,
        _heap: Heap<'v>,
        _private: Private,
    ) -> crate::Result<Value<'v>> {
        ValueError::unsupported_for::<_, Self>("[,]")
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
        _this: Self::Get,
        _start: Option<Value<'v>>,
        _stop: Option<Value<'v>>,
        _stride: Option<Value<'v>>,
        _heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        ValueError::unsupported_for::<_, Self>("[::]")
    }

    /// Implement iteration over the value of this container by providing
    /// the values in a `Vec`.
    #[starlark_internal_vtable(skip)]
    fn iterate_collect(_this: Self::Get, _heap: Heap<'v>) -> crate::Result<Vec<Value<'v>>> {
        ValueError::unsupported_for::<_, Self>("(iter)")
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
    unsafe fn iterate(this: Self::Get, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(heap
            .alloc_tuple(&Self::iterate_collect(this, heap)?)
            .to_value())
    }

    /// Returns the size hint for the iterator.
    unsafe fn iter_size_hint(_this: Self::Get, _index: usize) -> (usize, Option<usize>) {
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
    unsafe fn iter_next(_this: Self::Get, _index: usize, _heap: Heap<'v>) -> Option<Value<'v>> {
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
    unsafe fn iter_stop(_this: Self::Get) {
        panic!(
            "iter_stop called on non-iterable value of type {}",
            Self::TYPE
        )
    }

    /// Returns the length of the value, if this value is a sequence.
    ///
    /// In a `#[starlark_value]` block, a length that cannot fail is written
    /// `fn length(this: &Self) -> i32`.
    fn length(_this: Self::Get) -> crate::Result<i32> {
        ValueError::unsupported_for::<_, Self>("len()")
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
    fn get_attr(_this: Self::Get, _attribute: &str, _heap: Heap<'v>) -> Option<Value<'v>> {
        None
    }

    /// A version of `get_attr` which takes `BorrowHashed<str>` instead of `&str`,
    /// thus implementation may reuse the hash of the string if this is called
    /// repeatedly with the same string.
    ///
    /// This function is optional, but if it is implemented, it must be consistent with
    /// [`get_attr`](Self::get_attr).
    fn get_attr_hashed(
        this: Self::Get,
        attribute: Hashed<&str>,
        heap: Heap<'v>,
    ) -> Option<Value<'v>> {
        Self::get_attr(this, attribute.key(), heap)
    }

    /// Return true if an attribute of name `attribute` exists for the current
    /// value.
    ///
    /// The three methods [`get_attr`](StarlarkType::get_attr),
    /// [`has_attr`](StarlarkType::has_attr) and [`dir_attr`](StarlarkType::dir_attr)
    /// must be consistent.
    ///
    /// Default implementation of this function delegates to [`get_attr`](Self::get_attr).
    fn has_attr(this: Self::Get, attribute: &str, heap: Heap<'v>) -> bool {
        Self::get_attr(this, attribute, heap).is_some()
    }

    /// Return a vector of string listing all attribute of the current value.
    ///
    /// The three methods [`get_attr`](StarlarkType::get_attr),
    /// [`has_attr`](StarlarkType::has_attr) and [`dir_attr`](StarlarkType::dir_attr)
    /// must be consistent - if you implement one, you should probably implement all three.
    fn dir_attr(_this: Self::Get) -> Vec<String> {
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
    ///
    /// # In a `#[starlark_value]` block
    ///
    /// The method is written with the operand it takes, as [`equals`](Self::equals) is:
    ///
    /// ```ignore
    /// fn is_in(this: &Self, other: &str) -> bool {
    ///     this.name.contains(other)
    /// }
    /// ```
    ///
    /// The result is `bool` or `starlark::Result<bool>`. The operand's type is what the
    /// typechecker takes `other in a` to accept. A value that is not of it is the error of a
    /// conversion that expected one, as for [`at`](Self::at), rather than what this default
    /// answers, which says that the type has no `in`.
    fn is_in(_this: Self::Get, other: Value<'v>) -> crate::Result<bool> {
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
    ///
    /// In a `#[starlark_value]` block it is written with the type of its result, as
    /// [`add`](Self::add) is with its types.
    fn plus(_this: Self::Get, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_for::<_, Self>("+")
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
    ///
    /// In a `#[starlark_value]` block it is written with the type of its result, as
    /// [`add`](Self::add) is with its types.
    fn minus(_this: Self::Get, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_for::<_, Self>("-")
    }

    /// Add with the arguments the other way around.
    /// Normal `add` should return `None` in order for it to be evaluated.
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn radd(
        _this: Self::Get,
        _lhs: Value<'v>,
        _heap: Heap<'v>,
    ) -> Option<crate::Result<Value<'v>>> {
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
    ///
    /// # In a `#[starlark_value]` block
    ///
    /// An operator is written with the operand it takes and what it makes:
    ///
    /// ```ignore
    /// fn add(this: &Self, other: &Self) -> Seconds {
    ///     Seconds(this.0 + other.0)
    /// }
    /// ```
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) derives this method from it, and
    /// from its two types what the typechecker knows of the operator
    /// ([`bin_op_signatures`](Self::bin_op_signatures)). This goes for every binary operator,
    /// and for the unary ones with the result alone.
    ///
    /// * The operand is any [`UnpackValue`](crate::values::UnpackValue), as for
    ///   [`equals`](Self::equals).
    /// * The result is an [`AllocValue`](crate::values::AllocValue), which the derived method
    ///   puts on the heap, or a `starlark::Result` of one. The type is named, not
    ///   `impl AllocValue`: the typechecker reads it.
    /// * A body that allocates takes the heap as its last parameter, `heap: Heap<'v>`, and
    ///   returns the handle it got ([`ValueOf`](crate::values::ValueOf)); a body that does not
    ///   use the heap leaves the parameter out.
    /// * A value that is not of the operand's type gets what the default of the method gives.
    ///   For `add`, `radd`, `mul` and `rmul` that is `None`, so that the other operand gets its
    ///   turn; for the other operators it is the error that the operator is not supported. A
    ///   value that is of the operand's type but does not convert is the conversion's error.
    ///
    /// A method is taken to be written as declared here, and left as it is, when it takes the
    /// heap, its operand's type is spelled `Value` and its result's `Option`; for the operators
    /// that return a `starlark::Result<Value>`, when the result is spelled so.
    fn add(_this: Self::Get, _rhs: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
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
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn sub(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        sub_unsupported::<Self>(other)
    }

    /// Called on `rhs` of `lhs * rhs` when `lhs.mul` returns `None`.
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn rmul(_this: Self::Get, lhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
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
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn mul(_this: Self::Get, _rhs: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
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
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn div(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        div_unsupported::<Self>(other)
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
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn percent(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        percent_unsupported::<Self>(other)
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
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn floor_div(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        floor_div_unsupported::<Self>(other)
    }

    /// Bitwise `&` operator.
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn bit_and(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        bit_and_unsupported::<Self>(other)
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
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn bit_or(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        bit_or_unsupported::<Self>(other)
    }

    /// Bitwise `^` operator.
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn bit_xor(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        bit_xor_unsupported::<Self>(other)
    }

    /// Bitwise `~` operator.
    ///
    /// In a `#[starlark_value]` block it is written with the type of its result, as
    /// [`add`](Self::add) is with its types.
    fn bit_not(_this: Self::Get, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        ValueError::unsupported_for::<_, Self>("~")
    }

    /// Bitwise `<<` operator.
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn left_shift(_this: Self::Get, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        left_shift_unsupported::<Self>(other)
    }

    /// Bitwise `>>` operator.
    ///
    /// In a `#[starlark_value]` block it is written with its types, as [`add`](Self::add) is.
    fn right_shift(
        _this: Self::Get,
        other: Value<'v>,
        _heap: Heap<'v>,
    ) -> crate::Result<Value<'v>> {
        right_shift_unsupported::<Self>(other)
    }

    /// The typing of `this op rhs`: the signatures of the operator, each with the type of
    /// `rhs` it takes and the type of the result. None if the type does not have the operator.
    /// The typechecker matches the operand against them; see [`OperatorSignature`].
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) generates this from the operator
    /// methods of the block: for one written with types (see [`add`](Self::add)), the types of
    /// its operand and result; for one written as declared here,
    /// [`OperatorSignature::any`]. `<` is [`compare`](Self::compare) with its operand, and
    /// `in` is [`is_in`](Self::is_in) with its.
    fn bin_op_signatures(_op: TypingBinOp) -> Vec<OperatorSignature> {
        Vec::new()
    }

    /// The typing of `lhs op this` for the operators with a reflected method (`radd`,
    /// `rmul`), as [`bin_op_signatures`](Self::bin_op_signatures); the operand is `lhs`.
    fn rbin_op_signatures(_op: TypingBinOp) -> Vec<OperatorSignature> {
        Vec::new()
    }

    /// The type of the result of the unary operator, if the type has it.
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) generates this from the unary
    /// methods of the block: for one written with the type of its result, that type; for one
    /// written as declared here, the type itself.
    fn un_op_result(_op: TypingUnOp) -> Option<Ty> {
        None
    }

    /// The typing of `this[index]`: the type of index the type takes and the type of what is
    /// there, or `None` if the type is not indexed. The typechecker matches the index against
    /// it as it matches an operand against the signature of an operator.
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) generates this from the
    /// [`at`](Self::at) of the block: the types of its index and result if it is written with
    /// them, [`OperatorSignature::any`] if it is written as declared here.
    fn at_signature() -> Option<OperatorSignature> {
        None
    }

    /// Called when exporting a value under a specific name,
    fn export_as(
        _this: Self::Get,
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
    fn set_at(_this: Self::Get, _index: Value<'v>, _new_value: Value<'v>) -> crate::Result<()> {
        Err(crate::Error::new_other(
            ValueError::CannotMutateImmutableValue,
        ))
    }

    /// Set the attribute named `attribute` of the current value to
    /// `value` (e.g. `a.attribute = value`).
    fn set_attr(_this: Self::Get, attribute: &str, _new_value: Value<'v>) -> crate::Result<()> {
        ValueError::unsupported_for::<_, Self>(&format!(".{attribute}="))
    }
}

/// [`FrozenHeap::alloc_simple`]: crate::values::FrozenHeap::alloc_simple
pub trait StarlarkValue<'v>:
    'v + ProvidesStaticType<'v> + Allocative + Debug + Display + Serialize + Sized
{
    /// The Starlark type this is a representation of: `Self` for a type with one
    /// representation, otherwise the view the type implements [`StarlarkType`] for.
    ///
    /// [`Memberships`] says which [predicate types](crate::values::predicate) the type is a
    /// member of, and how a value of this representation is used as one.
    type Type: StarlarkType<'v> + Memberships<'v, Self>;

    /// This value as its type's [`Get`](StarlarkTypeExpr::Get): what [`get`](StarlarkTypeExpr::get)
    /// gives for a value of this representation, for a caller that already knows the
    /// representation. That is the handle itself for a type with one representation, and the
    /// type's view of this representation for a type with several.
    ///
    /// [`#[starlark_value]`](crate::values::starlark_value) writes this, except for a type
    /// that writes its own `Get` and for a representation whose type is not a view that
    /// `#[derive(StarlarkRepresentations)]` wrote; those write it by hand.
    fn get_typed(this: ValueTyped<'v, Self>) -> <Self::Type as StarlarkTypeExpr<'v>>::Get;

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

    /// Whether `value` is of the type this value denotes, for the representations of `type`.
    ///
    /// This lives on the representation rather than on [`StarlarkType`]: it runs on every runtime
    /// type check, and a representation of `type` is monomorphised over its matcher, so here the
    /// matcher is called statically where the view of `type` could only call it dynamically.
    #[doc(hidden)]
    fn type_matches_value(&self, _value: Value<'v>, _private: Private) -> bool {
        unreachable!("`type_matches_value` should only be called on special types")
    }

    /// This value as a compiled type, for the representations of `type`; `None` for a value of
    /// any other type.
    ///
    /// This lives on the representation rather than on [`StarlarkType`]: a representation of
    /// `type` is generic over its matcher, and only it can erase the matcher. The view of
    /// `type` is made from a `Value` and does not know which matcher it has.
    #[doc(hidden)]
    fn as_type_compiled_dyn(&'v self, _private: Private) -> Option<&'v dyn TypeCompiledDyn> {
        None
    }
}
