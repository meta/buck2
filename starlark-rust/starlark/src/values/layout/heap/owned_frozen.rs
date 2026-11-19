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

//! [`OwnedFrozen`] and [`OwnedFrozenRef`]: a value paired with the sealed frozen heap that keeps
//! it alive. Everything that touches the pairing is in this file; conveniences built on its safe
//! API are in `owned_frozen_ext.rs`.

use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;

use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use starlark_map::Equivalent;

use crate::any::IsStaticType;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::HeapSendable;
use crate::values::OwnedFrozenHeap;
use crate::values::layout::heap::branding::rebrand_ref_unchecked;
use crate::values::layout::heap::branding::rebrand_unchecked;
use crate::values::layout::heap::edge::HeapEdge;
use crate::values::layout::heap::name::FrozenHeapName;
use crate::values::layout::heap::owned_frozen_ext::FnOncish;
use crate::values::layout::heap::owned_frozen_ext::FnOncish2;
use crate::values::layout::heap::profile::by_type::HeapSummary;
use crate::values::layout::heap::sealed::FrozenHeapArc;
use crate::values::layout::heap::send::HeapSyncable;

/// A value in a frozen heap that is automatically kept alive.
///
/// This type is a `T` together with the sealed frozen heap that keeps that `T` alive. The
/// `'static` in a type like `OwnedFrozen<Value<'static>>` stands in for the brand of that heap,
/// which has no name, and no accessor hands the value back at `'static`:
///
/// * [`add_to_heap`](OwnedFrozen::add_to_heap) records the owning heap as a reference of the
///   given heap and hands the value back at that heap's brand. This is what you want nearly all
///   of the time.
/// * [`by_ref`](OwnedFrozen::by_ref) runs a closure on the value at a brand `'fv` private to the
///   closure, so nothing derived from it can escape;
///   [`by_ref_with_reconstructor`](OwnedFrozen::by_ref_with_reconstructor) also provides an
///   `OwnedFrozenReconstructor`, which re-pairs derived values with the owner or mints a
///   [`HeapEdge`].
/// * [`map`](OwnedFrozen::map), [`try_map`](OwnedFrozen::try_map) and
///   [`maybe_map`](OwnedFrozen::maybe_map) produce an `OwnedFrozen<U>` of the same heap.
/// * [`as_ref`](OwnedFrozen::as_ref) borrows the owner as an [`OwnedFrozenRef`], which uses the
///   borrow as the brand and hands the value out directly.
///
/// The `branding` module explains why the accessors are shaped this way.
///
/// `OwnedFrozen<()>` is a bare heap handle: it keeps a heap alive without picking out any value in
/// it, and offers only the heap-identity API (`name`, `refs`, the size accessors). It compares and
/// hashes by heap identity. Sealing an [`OwnedFrozenHeap`] produces one, see
/// [`OwnedFrozenHeap::seal`], and it is the currency of [`Heap::add_reference`] and
/// [`FrozenHeap::add_reference`].
pub struct OwnedFrozen<T> {
    heap_ref: FrozenHeapArc,
    // This is morally storing a `T::Reinfect<'fv>` for `'fv` the lifetime associated with the
    // frozen heap. It would be a little bit more natural to store a `T::Reinfect<'static>` here;
    // `T` is guaranteed by the safety contract on `ProvidesStaticType` to be the same, but if we
    // had a `T::Reinfect<'static>` we wouldn't need to rely on that.
    //
    // The problem is that that would require us to add a `T: IsStaticType` bound to this *type*.
    // That does mostly turn out fine, except that it turns out to be the *exact* pattern that
    // consistently hits the compiler bug in <https://github.com/rust-lang/rust/issues/102211>,
    // making this type ~unusable in async contexts. Once that bug is fixed, it may be worth to
    // revisit.
    v: T,
}

// This module only has the safety-critical impls for this type. Additional conveniences and trait
// impls are found in `owned_frozen_ext.rs` and based on the safe APIs provided here
impl<T> OwnedFrozen<T>
where
    for<'fv> T: IsStaticType<Reinfect<'fv> = T>,
{
    /// Get a reference to the inner value
    pub fn get<'a>(&'a self) -> &'a T {
        &self.v
    }
}

impl<T: IsStaticType> OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
{
    /// Create a new `OwnedFrozen` from the given heap and a value associated with that heap.
    ///
    /// # SAFETY
    ///
    /// The `'fv` provided must be kept alive by the passed `owner`.
    pub(crate) unsafe fn unchecked_new<'fv>(owner: OwnedFrozen<()>, v: T::Reinfect<'fv>) -> Self
    where
        // See comments on `Send` and `Sync` impls below
        for<'fv2> T::Reinfect<'fv2>: HeapSendable<'fv2> + HeapSyncable<'fv2>,
    {
        // SAFETY: The caller guarantees that `owner` keeps `'fv` alive.
        unsafe { Self::from_erased(owner, Self::erase_brand(v)) }
    }

    /// Pair `v`, which had its brand forgotten by [`erase_brand`](OwnedFrozen::erase_brand), with
    /// the heap that keeps it alive.
    ///
    /// # SAFETY
    ///
    /// `owner` must keep the brand that `v` was erased from alive.
    pub(crate) unsafe fn from_erased(owner: OwnedFrozen<()>, v: T) -> Self
    where
        // See comments on `Send` and `Sync` impls below
        for<'fv2> T::Reinfect<'fv2>: HeapSendable<'fv2> + HeapSyncable<'fv2>,
    {
        Self {
            heap_ref: owner.heap_ref,
            v,
        }
    }

    /// The owner and the brand-erased value, for storing the value beside the owner in
    /// another shape. The owner keeps the value alive exactly as it did here.
    ///
    /// # SAFETY
    ///
    /// The value must stay paired with the returned owner, or one that keeps its heap
    /// alive, and be given a brand again only through that owner. Dropping the owner
    /// while keeping the value leaves the value pointing into a freed heap.
    pub(crate) unsafe fn into_parts(self) -> (OwnedFrozen<()>, T) {
        (OwnedFrozen::for_heap(self.heap_ref), self.v)
    }

    /// Forget the brand of `v`.
    ///
    /// # SAFETY
    ///
    /// The result must be stored alongside an owner that keeps `'fv` alive.
    pub(crate) unsafe fn erase_brand<'fv>(v: T::Reinfect<'fv>) -> T {
        // SAFETY: The caller's obligation is the erased storage case of `rebrand_unchecked`'s.
        unsafe { rebrand_unchecked::<T, T::Reinfect<'fv>, T>(v) }
    }

    /// Give `v`, which had its brand forgotten by [`erase_brand`](OwnedFrozen::erase_brand), a
    /// brand again.
    ///
    /// # SAFETY
    ///
    /// The heap identified by `'fv` must keep `v` alive.
    pub(crate) unsafe fn restore_brand<'fv>(v: T) -> T::Reinfect<'fv> {
        // SAFETY: The caller's obligation is `rebrand_unchecked`'s.
        unsafe { rebrand_unchecked::<T, T, T::Reinfect<'fv>>(v) }
    }

    /// [`restore_brand`](OwnedFrozen::restore_brand), behind a reference.
    ///
    /// # SAFETY
    ///
    /// As for [`restore_brand`](OwnedFrozen::restore_brand).
    pub(crate) unsafe fn restore_brand_ref<'a, 'fv>(v: &'a T) -> &'a T::Reinfect<'fv> {
        // SAFETY: The caller's obligation is `rebrand_unchecked`'s.
        unsafe { rebrand_ref_unchecked::<T, T, T::Reinfect<'fv>>(v) }
    }

    /// Build a value in a fresh frozen heap and return it kept alive by that heap.
    ///
    /// The heap is private to `f`, which can only get data out of it by returning it at the
    /// heap's brand, so the result is paired with its owner by construction. This, and
    /// [`OwnedFrozenHeap::seal_with`] for a heap with more than one root, are how an `OwnedFrozen`
    /// comes to be: a value is never paired with a heap after the fact.
    ///
    /// The `name` identifies the heap; see [`OwnedFrozen::name`].
    pub fn build<F>(name: FrozenHeapName, f: F) -> Self
    where
        // See comments on `Send` and `Sync` impls below
        for<'fv2> T::Reinfect<'fv2>: HeapSendable<'fv2> + HeapSyncable<'fv2>,
        for<'fh> F: FnOnce(FrozenHeap<'fh>) -> T::Reinfect<'fh>,
    {
        match OwnedFrozenHeap::new().seal_with(name, |heap| Ok::<_, Infallible>(f(heap))) {
            Ok(v) => v,
        }
    }

    /// Use this value within the given heap: records this value's heap as a reference of `heap`,
    /// which keeps it alive from then on, and hands the value back at `heap`'s brand.
    ///
    /// ```
    /// use starlark::environment::FrozenModule;
    /// use starlark::environment::Module;
    /// use starlark::values::FrozenHeapName;
    /// use starlark::values::OwnedFrozen;
    /// use starlark::values::Value;
    ///
    /// fn copy<'v>(from: &FrozenModule, to: &Module<'v>) {
    ///     let x: OwnedFrozen<Value<'static>> = from.get("value").unwrap();
    ///     let v: Value<'v> = x.add_to_heap(to.heap());
    ///     to.set("value", v);
    /// }
    ///
    /// let from = Module::with_temp_heap(|from| {
    ///     from.set("value", from.heap().alloc("a string").to_value());
    ///     from.freeze_named(FrozenHeapName::user("from")).unwrap()
    /// });
    /// Module::with_temp_heap(|to| {
    ///     copy(&from, &to);
    ///     assert_eq!(to.get("value").unwrap().unpack_str(), Some("a string"));
    /// });
    /// ```
    ///
    /// When `to` is frozen, the reference is carried into its sealed heap, so the resulting
    /// `FrozenModule` keeps `from`'s heap alive too. The `branding` module explains the brand.
    pub fn add_to_heap<'v>(self, heap: Heap<'v>) -> T::Reinfect<'v> {
        heap.add_reference(self.owner());

        // SAFETY: The heap we just added the reference to keeps this alive for `'v`
        unsafe { Self::restore_brand(self.v) }
    }

    /// Access the underlying value and a reconstructor in a closure
    pub fn by_ref_with_reconstructor<'s, F, R>(&'s self, f: F) -> R
    where
        // Note: This `'a` is intentionally not `'s`. The danger that poses is that `'fv` is
        // supposed to be a brand and hence and arbitrary lifetime, but using `'s` would allow the
        // user to prove `'fv: 's` which makes the lifetime no longer arbitrary. In the extreme
        // case, if the caller supplies `'s = 'static`, they can prove `'fv = 'static`, meaning the
        // `'fv` is no longer unique at all.
        //
        // It's not clear that this is actually a problem, since if `'s = 'static` this thing lives
        // forever and the poison is mostly gone anyway, but it's still very hard to reason about.
        for<'a, 'fv> F: FnOnce(&'a T::Reinfect<'fv>, OwnedFrozenReconstructor<'fv>) -> R,
    {
        // SAFETY: `self.heap_ref` keeps the value alive for the borrow that `'fv` is.
        f(
            unsafe { Self::restore_brand_ref(&self.v) },
            OwnedFrozenReconstructor {
                heap_ref: &self.heap_ref,
                _invariant: PhantomData,
            },
        )
    }

    /// Borrow a part of the underlying value as an [`OwnedFrozenRef`], using the borrow as the
    /// brand.
    ///
    /// This is how a value is picked out of an owner whose type is not `Copy`, where
    /// [`as_ref`](OwnedFrozen::as_ref) is unavailable.
    pub fn maybe_map_ref<'s, U, F>(&'s self, f: F) -> Option<OwnedFrozenRef<'s, U>>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'a, 'fv> F: FnOncish<&'a T::Reinfect<'fv>, Option<U::Reinfect<'fv>>>,
    {
        // SAFETY: `self.heap_ref` keeps the value alive for the borrow that `'fv` is.
        let v = f(unsafe { Self::restore_brand_ref(&self.v) })?;
        // SAFETY: `f` is generic over the brand, so apart from statics, which live in no heap,
        // it can only return values derived from its input, which our heap keeps alive
        Some(unsafe { OwnedFrozenRef::unchecked_new(self.owner(), v) })
    }

    /// Map the underlying value and access a reconstructor
    pub fn try_by_value_with_reconstructor<U, E, R, F>(self, f: F) -> (Result<OwnedFrozen<U>, E>, R)
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish2<
                T::Reinfect<'fv>,
                OwnedFrozenReconstructor<'fv>,
                (Result<U::Reinfect<'fv>, E>, R),
            >,
    {
        let OwnedFrozen { heap_ref, v } = self;
        // `'fv` is the borrow of `heap_ref`, which has to end before `heap_ref` can move into the
        // result, so the result's brand is erased first, as `OwnedFrozenHeap::seal_with` does.
        let (v, extra) = {
            let reconstructor = OwnedFrozenReconstructor {
                heap_ref: &heap_ref,
                _invariant: PhantomData,
            };
            // SAFETY: `heap_ref` keeps the value alive for the borrow that `'fv` is.
            let (v, extra) = f(unsafe { Self::restore_brand(v) }, reconstructor);
            // SAFETY: `'fv` is the brand of `heap_ref`, which the result is paired with below.
            (
                v.map(|v| unsafe { OwnedFrozen::<U>::erase_brand(v) }),
                extra,
            )
        };
        // SAFETY: `heap_ref` is the heap that `'fv` named.
        (
            v.map(|v| unsafe { OwnedFrozen::from_erased(OwnedFrozen::for_heap(heap_ref), v) }),
            extra,
        )
    }
}

/// SAFETY: `OwnedFrozen<Foo<'static>>` is meant to be `Send` exactly when `Foo<'fv>` is
/// `HeapSendable<'fv>`, sendable up to the frozen values it holds (see the `send` module), and
/// `Sync` likewise. The auto traits would say just that, `Value<'static>` being `Send + Sync`, and
/// so would an impl conditional on `for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv>`.
/// Neither survives an `await`: for a value held across one, rustc replaces the lifetimes with
/// fresh ones and then has to prove `Value<'0>: Send` for every `'0`, which it cannot
/// (<https://github.com/rust-lang/rust/issues/102211>; `owned_frozen_ext.rs` has the regression
/// test, and the auto-trait version fails it). So the impls are unconditional and every
/// constructor requires the bounds instead, which is equivalent up to the values that can be
/// constructed.
unsafe impl<T> Send for OwnedFrozen<T> {}
unsafe impl<T> Sync for OwnedFrozen<T> {}

/// The heap-identity API: facts about the owning heap, independent of the value.
///
/// ```
/// use starlark::values::FrozenHeapName;
/// use starlark::values::OwnedFrozen;
/// use starlark::values::Value;
///
/// let v: OwnedFrozen<Value<'static>> =
///     OwnedFrozen::build(FrozenHeapName::user("example"), |heap| {
///         heap.alloc("contents").to_value()
///     });
/// assert_eq!(v.name().unwrap().to_string(), "example");
/// assert!(v.allocated_bytes() > 0);
/// assert_eq!(v.refs().count(), 0);
/// ```
impl<T> OwnedFrozen<T> {
    /// The owning heap, as a borrowed handle.
    pub fn owner(&self) -> OwnedFrozenRef<'_, ()> {
        OwnedFrozenRef::for_heap(&self.heap_ref)
    }

    pub(crate) fn heap_arc(&self) -> &FrozenHeapArc {
        &self.heap_ref
    }

    /// The name of the owning heap.
    ///
    /// Names are assigned when sealing frozen heaps, see [`OwnedFrozenHeap::seal`]; in
    /// practice, this is done when freezing modules, see
    /// [`Module::freeze_named`](crate::environment::Module::freeze_named).
    ///
    /// The name is made available here and not at a higher point like the module level so that it
    /// can be inspected even when traversing the dependency graph of frozen heaps via
    /// [`refs`](OwnedFrozen::refs).
    pub fn name(&self) -> Option<&FrozenHeapName> {
        self.heap_ref.name()
    }

    /// The frozen heaps that the owning heap depends on.
    pub fn refs(&self) -> impl Iterator<Item = OwnedFrozenRef<'_, ()>> {
        self.heap_ref.refs_slice().iter().map(OwnedFrozen::owner)
    }

    /// Number of bytes allocated on the owning heap, not including any memory allocated outside of
    /// the starlark heap.
    pub fn allocated_bytes(&self) -> usize {
        self.heap_ref.allocated_bytes()
    }

    /// Peak number of bytes allocated on the live heap that produced the owning heap, if it was
    /// produced by freezing one.
    pub fn peak_allocated_bytes(&self) -> Option<usize> {
        self.heap_ref.peak_allocated_bytes()
    }

    /// Number of bytes allocated by the owning heap but not filled. These bytes will _never_ be
    /// filled, as no further allocations can be made on a sealed heap.
    pub fn available_bytes(&self) -> usize {
        self.heap_ref.available_bytes()
    }

    /// A summary of how much memory is allocated by the owning heap. Doesn't include the heaps it
    /// keeps alive by reference.
    pub fn allocated_summary(&self) -> HeapSummary {
        self.heap_ref.allocated_summary()
    }
}

impl OwnedFrozen<()> {
    /// A handle to the heap behind `heap_ref`.
    pub(crate) fn for_heap(heap_ref: FrozenHeapArc) -> Self {
        Self { heap_ref, v: () }
    }
}

/// The empty heap, which keeps nothing alive.
impl Default for OwnedFrozen<()> {
    fn default() -> Self {
        Self::for_heap(FrozenHeapArc::default())
    }
}

/// Heap identity: two handles are equal iff they refer to the same heap allocation. Consistent
/// within a process, but non-deterministic across executions and between distinct but observably
/// identical heaps.
impl PartialEq for OwnedFrozen<()> {
    fn eq(&self, other: &Self) -> bool {
        self.heap_ref == other.heap_ref
    }
}

impl Eq for OwnedFrozen<()> {}

impl Hash for OwnedFrozen<()> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.heap_ref.hash(state)
    }
}

impl PagableSerialize for OwnedFrozen<()> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.heap_ref.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for OwnedFrozen<()> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        Ok(Self::for_heap(FrozenHeapArc::pagable_deserialize(
            deserializer,
        )?))
    }
}

/// Marker providing the ability to reconstruct `OwnedFrozen` values.
///
/// This type is provided as an argument to a number of the closures in `OwnedFrozen` APIs. It
/// allows constructing more `OwnedFrozen`s referring to the same heap:
///
/// ```rust,ignore
/// let v: OwnedFrozen<(Value<'static>, Value<'static>)> = ...;
/// let v: (OwnedFrozen<Value<'static>>, OwnedFrozen<Value<'static>>) = v
///     .by_ref_with_reconstructor(|vs, reconstructor| {
///         let v0 = reconstructor.reconstruct(vs.0);
///         let v1 = reconstructor.reconstruct(vs.1);
///         (v0, v1)
///     });
/// ```
///
/// Usually this is not needed and combinations of `map`, `clone` suffice instead.
#[derive(Copy, Clone, Dupe)]
pub struct OwnedFrozenReconstructor<'fv> {
    heap_ref: &'fv FrozenHeapArc,
    // Ensure this is invariant in `'fv`; other than that, it's fine for it to be `Send + Sync`,
    // though not very useful
    _invariant: PhantomData<fn(&'fv ()) -> &'fv ()>,
}

impl<'fv> OwnedFrozenReconstructor<'fv> {
    /// Packages a value of this heap into an `OwnedFrozen` of its own.
    pub fn reconstruct<T: IsStaticType>(&self, v: T::Reinfect<'fv>) -> OwnedFrozen<T>
    where
        for<'fv2> T::Reinfect<'fv2>: HeapSendable<'fv2> + HeapSyncable<'fv2> + Sized,
    {
        // SAFETY: The heap ref keeps the value alive for `'fv`
        unsafe { OwnedFrozen::unchecked_new(OwnedFrozen::for_heap(self.heap_ref.dupe()), v) }
    }

    /// Make this heap a dependency of the given heap, witnessed by the returned edge.
    ///
    /// This is the escape hatch out of the closure-based `OwnedFrozen` APIs: `'fv`-branded
    /// values can be rebranded for the given heap and returned from the closure.
    pub fn edge<'v>(&self, heap: Heap<'v>) -> HeapEdge<'v, 'fv> {
        heap.add_reference(OwnedFrozenRef::for_heap(self.heap_ref));

        // SAFETY: The reference we just added keeps our heap alive for `'v`, and `'fv` is a
        // closure-introduced brand
        unsafe { HeapEdge::unchecked_new() }
    }

    /// Like [`edge`](OwnedFrozenReconstructor::edge), but for a frozen heap
    pub fn frozen_edge<'v>(&self, heap: FrozenHeap<'v>) -> HeapEdge<'v, 'fv> {
        heap.add_reference(OwnedFrozenRef::for_heap(self.heap_ref));

        // SAFETY: The reference we just added keeps our heap alive for `'v`, and `'fv` is a
        // closure-introduced brand
        unsafe { HeapEdge::unchecked_new() }
    }
}

/// A value in a frozen heap, kept alive by a borrowed heap.
///
/// This is the borrowed counterpart of [`OwnedFrozen`]: instead of owning the heap, it borrows
/// one for `'f`, and `'f` doubles as the brand under which the value is handed out. That
/// makes access much lighter-weight than `OwnedFrozen`'s closure-based APIs — [`value`] hands out
/// the branded value directly — and the type is `Copy` when `T` is.
///
/// Because `'f` is an ordinary lifetime rather than a closure-introduced one, it is a weaker
/// brand: two `OwnedFrozenRef`s for different heaps may share the same `'f`. The soundness of
/// this type does not depend on brand uniqueness — only on the heap ref outliving `'f` — but
/// APIs that accept `'f`-branded values back cannot exist on this type; use the brand-generic
/// [`try_map`] and friends instead.
///
/// Create one with [`OwnedFrozen::as_ref`].
///
/// [`value`]: OwnedFrozenRef::value
/// [`try_map`]: OwnedFrozenRef::try_map
pub struct OwnedFrozenRef<'f, T> {
    heap_ref: &'f FrozenHeapArc,
    // Morally a `T::Reinfect<'f>`, stored brand-erased for the same reasons as `OwnedFrozen::v`
    v: T,
}

// This type has the same relationship to its safety-critical impls as `OwnedFrozen`: everything
// here upholds the invariant that `v` is kept alive by the heap behind `heap_ref`; conveniences
// live in `owned_frozen_ext.rs`.
impl<'f, T: IsStaticType> OwnedFrozenRef<'f, T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
{
    /// Create a new `OwnedFrozenRef` from the given heap and a value associated with that heap.
    ///
    /// # SAFETY
    ///
    /// The value must be kept alive by the heap behind `owner`.
    pub(crate) unsafe fn unchecked_new(owner: OwnedFrozenRef<'f, ()>, v: T::Reinfect<'f>) -> Self
    where
        // See comments on the `Send` and `Sync` impls for `OwnedFrozen`
        for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv>,
    {
        Self {
            heap_ref: owner.heap_ref,
            // SAFETY: Caller promised
            v: unsafe { OwnedFrozen::<T>::erase_brand(v) },
        }
    }

    /// Reattaches a value whose brand was erased by [`OwnedFrozen::into_parts`] to its owner.
    ///
    /// # SAFETY
    ///
    /// `owner` must be the heap `v` was erased from, or one that keeps that heap alive.
    pub(crate) unsafe fn from_erased(owner: OwnedFrozenRef<'f, ()>, v: T) -> Self
    where
        for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv>,
    {
        Self {
            heap_ref: owner.heap_ref,
            v,
        }
    }

    /// Get the value, branded with `'f`
    pub fn value(&self) -> T::Reinfect<'f>
    where
        T: Copy,
    {
        // SAFETY: The heap ref keeps the value alive for `'f`
        unsafe { OwnedFrozen::<T>::restore_brand(self.v) }
    }

    /// Get access to this value within the provided heap
    ///
    /// See the `branding` module for more details.
    pub fn add_to_heap<'v>(self, heap: Heap<'v>) -> T::Reinfect<'v> {
        heap.add_reference(self.owner());

        // SAFETY: The heap we just added the reference to keeps this alive for `'v`
        unsafe { OwnedFrozen::<T>::restore_brand(self.v) }
    }

    /// Like [`add_to_heap`](OwnedFrozenRef::add_to_heap), but for a frozen heap
    pub fn add_to_frozen_heap<'v>(self, heap: FrozenHeap<'v>) -> T::Reinfect<'v> {
        heap.add_reference(self.owner());

        // SAFETY: The heap we just added the reference to keeps this alive for `'v`
        unsafe { OwnedFrozen::<T>::restore_brand(self.v) }
    }

    /// Convert to an [`OwnedFrozen`] of the same value
    pub fn to_owned(&self) -> OwnedFrozen<T>
    where
        T: Copy,
        for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv>,
    {
        // SAFETY: The heap ref keeps the value alive
        unsafe {
            OwnedFrozen::unchecked_new(OwnedFrozen::for_heap(self.heap_ref.dupe()), self.value())
        }
    }

    /// Fallibly transform the contained value
    pub fn try_map<U, E, F>(self, f: F) -> Result<OwnedFrozenRef<'f, U>, E>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish<T::Reinfect<'fv>, Result<U::Reinfect<'fv>, E>>,
    {
        let owner = self.owner();
        // SAFETY: The heap ref keeps the value alive for `'f`
        let v = f(unsafe { OwnedFrozen::<T>::restore_brand(self.v) })?;
        // SAFETY: `f` is generic over the brand, so apart from statics, which live in no heap,
        // it can only return values derived from its input, which our heap keeps alive
        Ok(unsafe { OwnedFrozenRef::unchecked_new(owner, v) })
    }
}

impl<'f, T: Copy> Copy for OwnedFrozenRef<'f, T> {}

impl<'f, T: Copy> Clone for OwnedFrozenRef<'f, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'f, T: Copy> Dupe for OwnedFrozenRef<'f, T> {}

/// SAFETY: As for `OwnedFrozen`; the `heap_ref` field is fine to share because `FrozenHeapArc`
/// is `Sync`.
unsafe impl<'f, T> Send for OwnedFrozenRef<'f, T> {}
unsafe impl<'f, T> Sync for OwnedFrozenRef<'f, T> {}

/// The heap-identity API, see the [`OwnedFrozen`] counterparts.
impl<'f, T> OwnedFrozenRef<'f, T> {
    /// The owning heap, as a borrowed handle.
    pub fn owner(&self) -> OwnedFrozenRef<'f, ()> {
        OwnedFrozenRef::for_heap(self.heap_ref)
    }

    pub(crate) fn heap_arc(&self) -> &'f FrozenHeapArc {
        self.heap_ref
    }

    /// See [`OwnedFrozen::name`].
    pub fn name(&self) -> Option<&'f FrozenHeapName> {
        self.heap_ref.name()
    }

    /// See [`OwnedFrozen::refs`].
    pub fn refs(&self) -> impl Iterator<Item = OwnedFrozenRef<'f, ()>> + use<'f, T> {
        let heap_ref: &'f FrozenHeapArc = self.heap_ref;
        heap_ref.refs_slice().iter().map(OwnedFrozen::owner)
    }

    /// See [`OwnedFrozen::allocated_bytes`].
    pub fn allocated_bytes(&self) -> usize {
        self.heap_ref.allocated_bytes()
    }

    /// See [`OwnedFrozen::peak_allocated_bytes`].
    pub fn peak_allocated_bytes(&self) -> Option<usize> {
        self.heap_ref.peak_allocated_bytes()
    }

    /// See [`OwnedFrozen::available_bytes`].
    pub fn available_bytes(&self) -> usize {
        self.heap_ref.available_bytes()
    }

    /// See [`OwnedFrozen::allocated_summary`].
    pub fn allocated_summary(&self) -> HeapSummary {
        self.heap_ref.allocated_summary()
    }
}

impl<'f> OwnedFrozenRef<'f, ()> {
    /// A handle to the heap behind `heap_ref`.
    pub(crate) fn for_heap(heap_ref: &'f FrozenHeapArc) -> Self {
        Self { heap_ref, v: () }
    }
}

impl Debug for OwnedFrozenRef<'_, ()> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.heap_ref, f)
    }
}

/// Heap identity, as for [`OwnedFrozen<()>`].
impl PartialEq for OwnedFrozenRef<'_, ()> {
    fn eq(&self, other: &Self) -> bool {
        *self.heap_ref == *other.heap_ref
    }
}

impl Eq for OwnedFrozenRef<'_, ()> {}

impl Hash for OwnedFrozenRef<'_, ()> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.heap_ref.hash(state)
    }
}

/// Heap identity, as for [`OwnedFrozen<()>`]: looks a borrowed heap up in a set of owned ones.
impl Equivalent<OwnedFrozen<()>> for OwnedFrozenRef<'_, ()> {
    fn equivalent(&self, key: &OwnedFrozen<()>) -> bool {
        *self.heap_ref == key.heap_ref
    }
}

impl<T: IsStaticType> OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
{
    /// Borrow this value as an [`OwnedFrozenRef`], using the borrow as the brand
    pub fn as_ref(&self) -> OwnedFrozenRef<'_, T>
    where
        T: Copy,
    {
        OwnedFrozenRef {
            heap_ref: &self.heap_ref,
            v: self.v,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use dupe::Dupe;

    use super::OwnedFrozen;
    use super::OwnedFrozenRef;
    use crate::values::FrozenHeapName;
    use crate::values::OwnedFrozenHeap;

    #[test]
    fn test_send_sync()
    where
        OwnedFrozen<()>: Send + Sync,
    {
    }

    fn sealed_heap(name: &str) -> OwnedFrozen<()> {
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("contents");
        });
        heap.seal(FrozenHeapName::user(name))
    }

    #[test]
    fn test_heap_identity_api() {
        let dep = sealed_heap("dep");
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("contents");
            heap.add_reference(dep.owner());
        });
        let owned = heap.seal(FrozenHeapName::user("heap"));

        assert_eq!(owned.name().unwrap().to_string(), "heap");
        assert!(owned.allocated_bytes() > 0);
        assert_eq!(owned.peak_allocated_bytes(), None);
        assert!(owned.allocated_summary().total_allocated_bytes() > 0);
        let refs: Vec<OwnedFrozenRef<()>> = owned.refs().collect();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name().unwrap().to_string(), "dep");
        assert!(refs[0] == dep.owner());

        let borrowed = owned.as_ref();
        assert_eq!(borrowed.name().unwrap().to_string(), "heap");
        assert_eq!(borrowed.allocated_bytes(), owned.allocated_bytes());
        assert_eq!(borrowed.available_bytes(), owned.available_bytes());
        assert_eq!(borrowed.peak_allocated_bytes(), None);
        assert_eq!(
            borrowed.allocated_summary().total_allocated_bytes(),
            owned.allocated_summary().total_allocated_bytes()
        );
        assert_eq!(borrowed.refs().count(), 1);
        assert!(borrowed.refs().next().unwrap() == refs[0]);
    }

    #[test]
    fn test_heap_identity_eq_hash() {
        let a = sealed_heap("a");
        let same_content = sealed_heap("a");
        let empty = OwnedFrozen::<()>::default();

        assert!(a == a.dupe());
        assert!(a != same_content);
        assert!(a != empty);
        assert!(empty == OwnedFrozen::<()>::default());
        let set: HashSet<OwnedFrozen<()>> = [a.dupe(), a.dupe(), same_content.dupe(), empty.dupe()]
            .into_iter()
            .collect();
        assert_eq!(set.len(), 3);

        assert!(a.as_ref() == a.as_ref());
        assert!(a.as_ref() != same_content.as_ref());
        assert!(a.as_ref() != empty.as_ref());
        let set: HashSet<OwnedFrozenRef<()>> = [
            a.as_ref(),
            a.as_ref(),
            same_content.as_ref(),
            empty.as_ref(),
        ]
        .into_iter()
        .collect();
        assert_eq!(set.len(), 3);
    }
}
