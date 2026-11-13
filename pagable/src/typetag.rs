/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Pagable-native trait object serialization using inventory for auto-registration.
//!
//! This module provides a trait object serialization system similar to `typetag`,
//! but using PagableSerialize/PagableBoxDeserialize instead of serde traits.
//!
//! It uses the `inventory` crate for automatic registration of concrete types
//! at program startup, so there's no need for explicit registration calls.
//!
//! # Usage
//!
//! Use the `#[pagable_typetag]` attribute on both the trait definition and
//! each impl block:
//!
//! ```ignore
//! #[pagable::pagable_typetag]
//! trait MyTrait: pagable::typetag::PagableTagged + Send + Sync {
//!     fn do_something(&self);
//! }
//!
//! #[derive(pagable::Pagable)]
//! struct MyImpl { value: i32 }
//!
//! #[pagable::pagable_typetag]
//! impl MyTrait for MyImpl {
//!     fn do_something(&self) { println!("{}", self.value); }
//! }
//! ```
//!
//! # How it works
//!
//! The `#[pagable_typetag]` attribute macro on a trait definition generates:
//! - A trait-specific registration struct
//! - A static registry to collect registered implementations
//! - `inventory::collect!` for the registration struct
//! - `PagableSerialize` impl for `dyn Trait`
//! - `PagableBoxDeserialize` impl for `dyn Trait`
//!
//! The `#[pagable_typetag]` attribute macro on an impl block generates:
//! - `PagableTypeTag` impl for the concrete type (using the type name as the tag)
//! - `inventory::submit!` to register the type with its tag

use std::any::TypeId;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::BuildHasherDefault;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;

use crate::Pagable;
use crate::PagableDeserializer;
use crate::PagableSerializer;
use crate::hashers::TypeIdHasher;

mod platform;

/// Static type tag used to register a concrete type for trait-object deserialization.
pub trait PagableTypeTag: Sized {
    fn pagable_type_tag_static() -> &'static str;
}

/// Stable, compiler-independent type name used to compose pagable typetags.
///
/// `std::any::type_name` makes no stability guarantee across compiler
/// releases, so tags built from it could invalidate previously serialized
/// data on a toolchain upgrade. Implementations of this trait compose the
/// name from `module_path!()` and source identifiers instead; the result
/// changes only when the type is renamed or moved.
///
/// `#[derive(Pagable)]` and `#[derive(PagableSerialize)]` provide an
/// implementation automatically. For generic types the derived
/// implementation requires every type parameter to implement
/// `PagableStableName` itself.
pub trait PagableStableName {
    fn pagable_stable_name() -> &'static str;
}

macro_rules! impl_stable_name {
    ($($ty:ty),* $(,)?) => {
        $(
            impl PagableStableName for $ty {
                fn pagable_stable_name() -> &'static str {
                    stringify!($ty)
                }
            }
        )*
    };
}

impl_stable_name!(
    bool, char, f32, f64, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, String,
);

macro_rules! impl_stable_name_wrapper {
    ($($ty:ident),* $(,)?) => {
        $(
            impl<T: PagableStableName + 'static> PagableStableName for $ty<T> {
                fn pagable_stable_name() -> &'static str {
                    memoized_stable_name::<Self>(|| {
                        format!(concat!(stringify!($ty), "<{}>"), T::pagable_stable_name())
                    })
                }
            }
        )*
    };
}

impl_stable_name_wrapper!(Arc, Box, Option, Vec);

/// Build (once per monomorphization per process) and cache the composed
/// stable name for `T`. Each distinct `T` leaks exactly one `String`.
///
/// The cache exists because generic monomorphizations cannot hold the name in
/// a per-instantiation static (statics in generic items are shared across all
/// instantiations), and rebuilding the `String` on every serialization would
/// leak unboundedly.
pub fn memoized_stable_name<T: 'static>(build: impl FnOnce() -> String) -> &'static str {
    // This sits on the serialization path of every generic pagable value, and
    // after the one-time insert per instantiation the map is read-only, so
    // the hit case takes only the shared read lock and concurrent serializers
    // do not contend.
    type Names = HashMap<TypeId, &'static str, BuildHasherDefault<TypeIdHasher>>;
    static NAMES: OnceLock<RwLock<Names>> = OnceLock::new();
    let names = NAMES.get_or_init(|| RwLock::new(Names::default()));
    if let Some(name) = names
        .read()
        .expect("stable name map lock should not be poisoned")
        .get(&TypeId::of::<T>())
    {
        return name;
    }
    // Build without holding the lock: for nested generics `build()` re-enters
    // this function for the type arguments, and the lock is not reentrant.
    // A racing thread may build the same name concurrently; the loser's
    // `String` is dropped, so at most one copy per instantiation is leaked.
    let name = build();
    names
        .write()
        .expect("stable name map lock should not be poisoned")
        .entry(TypeId::of::<T>())
        .or_insert_with(|| String::leak(name))
}

/// Object-safe serialization trait for tagged types.
///
/// This trait is dyn-compatible and used by trait objects to serialize
/// themselves with a type tag.
///
/// Notably this trait does **not** inherit from `PagableSerialize`. If it did,
/// Rust would auto-synthesize `impl PagableSerialize for dyn Trait` via the
/// supertrait relation (vtable-dispatched to the concrete type's body-only
/// `pagable_serialize`) and the `#[pagable_typetag]` macro couldn't emit its
/// own `impl PagableSerialize for dyn Trait` that writes `tag + body` (E0371:
/// "the object type automatically implements the trait"). Keeping the
/// relation out lets the macro own the `PagableSerialize` impl for the dyn
/// type so `Arc<dyn Trait>` serialization automatically includes the tag.
pub trait PagableTagged: Send + Sync {
    /// Get the type tag through a trait object, for serialization.
    fn pagable_type_tag(&self) -> &'static str;

    /// Write the body of this value (no tag). Mirrors `PagableSerialize::pagable_serialize`.
    ///
    /// Why this exists (instead of having `serialize_tagged` call
    /// `<Self as PagableSerialize>::pagable_serialize` directly):
    ///
    /// - That call needs `PagableTagged: PagableSerialize`, but we can't have
    ///   that supertrait (see comment on the trait above).
    /// - `where Self: PagableSerialize` → `serialize_tagged` references `Self` in its
    ///   where clause, the whole trait loses `dyn`-compatibility (E0038).
    ///
    /// So `pagable_serialize_body` has to be a genuine vtable-dispatched method.
    /// The blanket implementation below forwards it to `PagableSerialize`.
    fn pagable_serialize_body(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()>;

    /// Write `tag + body`. The `#[pagable_typetag]` macro generates an
    /// `impl PagableSerialize for dyn Trait` that forwards to this method.
    fn serialize_tagged(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        let tag = self.pagable_type_tag();
        serde::Serialize::serialize(&tag, serializer.serde())?;
        self.pagable_serialize_body(serializer)
    }

    /// Write a tagged Arc view that refers to its canonical concrete Arc.
    #[doc(hidden)]
    fn serialize_tagged_arc_payload(
        self: Arc<Self>,
        serializer: &mut dyn PagableSerializer,
    ) -> crate::Result<()>;
}

impl<T> PagableTagged for T
where
    T: PagableTypeTag + Pagable,
{
    fn pagable_type_tag(&self) -> &'static str {
        T::pagable_type_tag_static()
    }

    fn pagable_serialize_body(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        self.pagable_serialize(serializer)
    }

    fn serialize_tagged_arc_payload(
        self: Arc<Self>,
        serializer: &mut dyn PagableSerializer,
    ) -> crate::Result<()> {
        let tag = self.pagable_type_tag();
        serde::Serialize::serialize(&tag, serializer.serde())?;
        serializer.serialize_arc(&self)
    }
}

/// Marker trait — `PagableTagged` registration for generic wrapper types.
///
/// For a generic `Wrapper<T>`, `#[pagable_tagged(MyDynTrait)]` expands to a
/// `PagableTypeTag` impl gated on `Self: PagableRegisteredFor<dyn
/// MyDynTrait>`:
///
/// ```ignore
/// #[pagable_tagged(MyDynTrait)]
/// struct Wrapper<T: MyInnerTrait>(pub T);
///
/// // Expands to (the blanket impl then provides PagableTagged):
/// impl<T: MyInnerTrait> PagableTypeTag for Wrapper<T>
/// where
///     Self: PagableRegisteredFor<dyn MyDynTrait>,
/// { /* ... */ }
/// ```
///
/// So `Wrapper<X>: PagableTypeTag` and therefore `PagableTagged` exist only when
/// `impl PagableRegisteredFor<dyn MyDynTrait> for Wrapper<X>` does.
///
/// **Don't** implement this trait by hand. Call
/// `register_typetag!(Wrapper<X> as dyn MyDynTrait)` for each pair you use —
/// it emits the `PagableRegisteredFor` impl alongside the inventory
/// registration for pagable.
pub trait PagableRegisteredFor<T: ?Sized> {}

/// Registration entry for a concrete type implementing a trait object.
///
/// Used by both `#[pagable_typetag]` and `register_typetag!`.
pub struct TypetagRegistration<T: ?Sized + 'static> {
    pub tag: fn() -> &'static str,
    pub deserialize: fn(&mut dyn PagableDeserializer<'_>) -> crate::Result<Box<T>>,
    pub deserialize_arc_payload: fn(&mut dyn PagableDeserializer<'_>) -> crate::Result<Arc<T>>,
}

// Manual impls: derives would wrongly require `T: Clone`/`T: Copy`, but the
// fields are only fn pointers regardless of `T`.
impl<T: ?Sized> Clone for TypetagRegistration<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for TypetagRegistration<T> {}

// Registrations are shared through statics — `inventory` records and the
// per-trait generic accumulators — so they must be `Send + Sync` for every
// `T`. The fn-pointer fields satisfy that unconditionally; this
// definition-site check rejects a future field that loses the auto traits,
// instead of erroring at every macro expansion site.
const _: () = {
    #[expect(
        dead_code,
        reason = "compile-time proof; the body only needs to typecheck"
    )]
    fn registration_is_send_sync<T: ?Sized + 'static>(
        reg: TypetagRegistration<T>,
    ) -> impl Send + Sync {
        reg
    }
};

/// A registry built from `TypetagRegistration` entries collected via `inventory`.
pub struct TypetagRegistry<T: ?Sized + 'static> {
    // Stored by value: generic registrations are drained out of a runtime
    // accumulator, so there is no `'static` allocation to borrow.
    map: HashMap<&'static str, TypetagRegistration<T>>,
}

impl<T: ?Sized + 'static> TypetagRegistry<T> {
    fn insert_registration(
        map: &mut HashMap<&'static str, TypetagRegistration<T>>,
        reg: &TypetagRegistration<T>,
    ) {
        let tag = (reg.tag)();
        let previous = map.insert(tag, *reg);
        assert!(
            previous.is_none(),
            "duplicate pagable typetag registration for {tag}"
        );
    }

    pub fn from_inventory(iter: impl Iterator<Item = &'static TypetagRegistration<T>>) -> Self {
        let mut map = HashMap::new();
        for reg in iter {
            Self::insert_registration(&mut map, reg);
        }
        TypetagRegistry { map }
    }

    /// Deserialize `tag + inline concrete body` into a boxed trait value. e.g. `Box<dyn Trait>`
    pub fn deserialize_tagged(
        &self,
        deserializer: &mut dyn PagableDeserializer<'_>,
    ) -> crate::Result<Box<T>> {
        let tag: String = serde::Deserialize::deserialize(deserializer.serde())?;
        let registration = self
            .map
            .get(tag.as_str())
            .ok_or_else(|| crate::__internal::anyhow::anyhow!("Unknown type tag: {}", tag))?;
        (registration.deserialize)(deserializer)
    }

    /// Deserialize `tag + canonical concrete Arc reference` into an Arc trait view.
    /// The registration coerces the referenced `Arc<Concrete>` without allocating
    /// another copy, preserving identity with other views of that concrete Arc.
    pub fn deserialize_tagged_arc_payload(
        &self,
        deserializer: &mut dyn PagableDeserializer<'_>,
    ) -> crate::Result<Arc<T>> {
        let tag: String = serde::Deserialize::deserialize(deserializer.serde())?;
        let registration = self
            .map
            .get(tag.as_str())
            .ok_or_else(|| crate::__internal::anyhow::anyhow!("Unknown type tag: {}", tag))?;
        (registration.deserialize_arc_payload)(deserializer)
    }
}

/// Accumulator for generic typetag registrations discovered at runtime.
///
/// Concrete `#[pagable_typetag]` implementations can register themselves with
/// `inventory::submit!`, but generic impls cannot: there is no single concrete
/// type to submit until Rust monomorphizes a used instantiation such as
/// `GenericVehicle<Cargo>`. To bridge that gap, the proc macro emits a
/// program constructor for each generic impl instantiation that pushes the
/// monomorphized registration into the trait's accumulator when the image
/// containing it is loaded.
///
/// Registries are built lazily at first use — strictly after program
/// constructors have run — so the registry builder just drains this trait's
/// accumulator and merges the generic registrations with the ordinary
/// inventory registrations.
pub struct GenericTypetagAccumulator<T: ?Sized + 'static> {
    // Pushes happen in program constructors and the drain happens later
    // inside `OnceLock::get_or_init`, so contention is not expected. Keep the
    // mutex because this type and its methods are safe public API over a
    // static accumulator; without interior locking, safe callers could race
    // `push` and `drain`.
    entries: Mutex<Vec<TypetagRegistration<T>>>,
}

impl<T: ?Sized + 'static> GenericTypetagAccumulator<T> {
    pub const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    pub fn push(&self, reg: TypetagRegistration<T>) {
        self.entries
            .lock()
            .expect("generic typetag accumulator mutex should not be poisoned")
            .push(reg);
    }

    pub fn drain(&self) -> Vec<TypetagRegistration<T>> {
        std::mem::take(
            &mut *self
                .entries
                .lock()
                .expect("generic typetag accumulator mutex should not be poisoned"),
        )
    }
}

impl<T: ?Sized + 'static> TypetagRegistry<T> {
    pub fn from_inventory_and_generic(
        iter: impl Iterator<Item = &'static TypetagRegistration<T>>,
        generic_entries: Vec<TypetagRegistration<T>>,
    ) -> Self {
        let mut map = HashMap::new();
        for reg in iter {
            Self::insert_registration(&mut map, reg);
        }
        // A monomorphization can be instantiated in several codegen units,
        // each of which emits its own registration constructor, so repeated
        // generic tags are expected and equivalent (a stable name uniquely
        // identifies the type). Only a generic tag colliding with an
        // inventory registration indicates a real conflict.
        let mut generic_tags = HashSet::new();
        for reg in &generic_entries {
            let tag = (reg.tag)();
            if generic_tags.insert(tag) {
                Self::insert_registration(&mut map, reg);
            }
        }
        TypetagRegistry { map }
    }
}

/// Register a concrete generic instantiation of a wrapper for pagable
/// typetag dispatch. Each call emits the `PagableRegisteredFor<dyn Trait>`
/// impl plus an inventory entry keyed by the concrete type's canonical tag. See
/// [`PagableRegisteredFor`] for the full picture.
///
/// Prerequisite: `Trait` must have `#[pagable_typetag]` applied.
///
/// Examples:
/// ```ignore
/// register_typetag!(Foo<MyInner> as dyn MyTrait);
/// register_typetag!(Foo<A, B> as dyn MyTrait);
/// register_typetag!(Foo<T, T> as dyn MyTrait);
/// ```
#[macro_export]
macro_rules! register_typetag {
    ($concrete:ty as dyn $trait:path) => {
        impl $crate::typetag::PagableRegisteredFor<dyn $trait> for $concrete {}

        $crate::__internal::inventory::submit! {
            <dyn $trait>::__pagable_wrap_registration(
                $crate::typetag::TypetagRegistration {
                    tag: <$concrete as $crate::typetag::PagableTypeTag>::pagable_type_tag_static,
                    deserialize: |deserializer| {
                        let value: $concrete =
                            $crate::PagableDeserialize::pagable_deserialize(deserializer)?;
                        Ok(Box::new(value) as Box<dyn $trait>)
                    },
                    deserialize_arc_payload: |deserializer| {
                        let value: std::sync::Arc<$concrete> =
                            $crate::PagableDeserialize::pagable_deserialize(deserializer)?;
                        let value: std::sync::Arc<dyn $trait> = value;
                        Ok(value)
                    },
                }
            )
        }
    };
}

// Under Miri the tests of generic registrations are compiled out, which leaves the methods of
// their fixtures unused.
#[cfg(test)]
#[cfg_attr(miri, expect(dead_code))]
mod tests {
    use std::any::Any;
    use std::fmt::Debug;
    use std::sync::Arc;

    use pagable::PagableRegisteredFor;
    use pagable::PagableTagged;

    use crate as pagable;
    use crate::Pagable;

    #[crate::pagable_typetag]
    pub trait Named: PagableTagged + Send + Sync + Debug {
        fn name(&self) -> &str;
    }

    #[derive(Pagable, Debug, Eq, PartialEq)]
    pub struct Key {
        pub name: Arc<String>,
    }

    #[crate::pagable_typetag]
    impl Named for Key {
        fn name(&self) -> &str {
            &self.name
        }
    }

    #[crate::pagable_typetag]
    pub trait SecondaryNamed: PagableTagged + Send + Sync + Debug {
        fn name(&self) -> &str;
    }

    impl SecondaryNamed for Key {
        fn name(&self) -> &str {
            &self.name
        }
    }

    crate::register_typetag!(Key as dyn SecondaryNamed);

    #[derive(Pagable, Debug, Eq, PartialEq)]
    #[crate::pagable_typetag(Named)]
    pub struct Bar {
        pub name: Arc<String>,
    }

    pub trait NamedDyn: PagableTagged + Send + Sync + Debug {
        fn name(&self) -> &str;
    }

    impl NamedDyn for Bar {
        fn name(&self) -> &str {
            &self.name
        }
    }

    impl<T: NamedDyn> Named for T {
        fn name(&self) -> &str {
            NamedDyn::name(self)
        }
    }

    #[test]
    fn test_typetag_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Named> = Arc::new(Key {
            name: Arc::new("test".to_owned()),
        });

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);

        let restored: Box<dyn Named> = <dyn Named>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.name(), "test");
        Ok(())
    }

    #[test]
    fn test_typetag_roundtrip_with_secondary_registration() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn SecondaryNamed> = Arc::new(Key {
            name: Arc::new("test".to_owned()),
        });

        assert_eq!(
            value.pagable_type_tag(),
            "pagable::typetag::tests::Key",
            "concrete typetag forms should compose the same stable names as generic forms"
        );

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn SecondaryNamed> =
            <dyn SecondaryNamed>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.name(), "test");
        Ok(())
    }

    #[test]
    fn test_typetag_arc_roundtrip_preserves_concrete_identity() -> crate::Result<()> {
        use crate::PagableDeserialize;
        use crate::PagableSerialize;
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;

        let concrete = Arc::new(Key {
            name: Arc::new("test".to_owned()),
        });
        let dyn_view: Arc<dyn Named> = concrete.clone();

        let mut serializer = TestingSerializer::new();
        dyn_view.pagable_serialize(&mut serializer)?;
        concrete.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored_dyn = Arc::<dyn Named>::pagable_deserialize(&mut deserializer)?;
        let restored_concrete = Arc::<Key>::pagable_deserialize(&mut deserializer)?;
        assert_eq!(
            Arc::as_ptr(&restored_dyn) as *const (),
            Arc::as_ptr(&restored_concrete) as *const (),
        );
        Ok(())
    }

    // --- Generic wrapper tests (like TyCustomFunction<F>) ---
    // Uses register_typetag! + TypetagRegistry instead of #[pagable_typetag]

    /// Trait for the dyn object (like TyCustomDyn)
    #[crate::pagable_typetag]
    pub trait Animal: PagableTagged + Send + Sync + Debug {
        fn species(&self) -> &str;
    }

    /// Generic wrapper (like TyCustomFunction<F>).
    /// The Registered bound on T enforces that only registered inner types can be used.
    #[derive(Debug, Pagable)]
    #[crate::pagable_tagged(Animal)]
    pub struct Wrapper<T: Pagable + Send + Sync + Debug + 'static>(pub T);

    impl<T: Pagable + Send + Sync + Debug + 'static> Animal for Wrapper<T>
    where
        Self: PagableRegisteredFor<dyn Animal>,
        Self: pagable::PagableStableName,
    {
        fn species(&self) -> &str {
            "wrapped"
        }
    }

    /// Concrete inner type (like ZipType) — registered
    #[derive(Debug, Pagable, Eq, PartialEq)]
    pub struct Cat;

    // Register Wrapper<Cat> for deserialization as dyn Animal.
    // This generates: impl PagableRegisteredFor<dyn Animal> for Wrapper<Cat> {}
    crate::register_typetag!(Wrapper<Cat> as dyn Animal);

    #[test]
    fn test_register_typetag_generic_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Animal> = Arc::new(Wrapper(Cat));

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn Animal> = <dyn Animal>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.species(), "wrapped");
        Ok(())
    }

    // --- 2-generic wrapper tests ---

    #[derive(Debug, Pagable)]
    #[crate::pagable_tagged(Animal)]
    pub struct Pair<
        A: Pagable + Send + Sync + Debug + 'static,
        B: Pagable + Send + Sync + Debug + 'static,
    >(pub A, pub B);

    impl<A: Pagable + Send + Sync + Debug + 'static, B: Pagable + Send + Sync + Debug + 'static>
        Animal for Pair<A, B>
    where
        Self: PagableRegisteredFor<dyn Animal>,
        Self: pagable::PagableStableName,
    {
        fn species(&self) -> &str {
            "paired"
        }
    }

    #[derive(Debug, Pagable, Eq, PartialEq)]
    pub struct Dog;

    // Each register_typetag! emits one impl on the wrapper itself.
    crate::register_typetag!(Pair<Cat, Dog> as dyn Animal);
    crate::register_typetag!(Pair<Cat, Cat> as dyn Animal);

    #[test]
    fn test_register_typetag_pair_distinct_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Animal> = Arc::new(Pair(Cat, Dog));

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn Animal> = <dyn Animal>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.species(), "paired");
        Ok(())
    }

    #[test]
    fn test_register_typetag_pair_same_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Animal> = Arc::new(Pair(Cat, Cat));

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn Animal> = <dyn Animal>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.species(), "paired");
        Ok(())
    }

    #[test]
    fn test_typetag_roundtrip_indirect_impl() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Named> = Arc::new(Bar {
            name: Arc::new("test".to_owned()),
        });

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);

        let restored: Box<dyn Named> = <dyn Named>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.name(), "test");
        Ok(())
    }

    // `#[derive(Pagable)]` on a struct holding `Arc<dyn Trait>` or
    // `Box<dyn Trait>` round-trips.
    #[derive(Debug, Pagable)]
    pub struct AnimalHolder {
        pub animal: Arc<dyn Animal>,
    }

    #[derive(Debug, Pagable)]
    pub struct AnimalHolderBox {
        pub animal: Box<dyn Animal>,
    }

    #[test]
    fn test_pagable_derive_arc_dyn_trait_field_roundtrip() -> crate::Result<()> {
        use crate::PagableDeserialize;
        use crate::PagableSerialize;
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;

        // Arc<dyn Animal> round-trip.
        let arc_value = AnimalHolder {
            animal: Arc::new(Wrapper(Cat)),
        };
        let mut serializer = TestingSerializer::new();
        arc_value.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored = AnimalHolder::pagable_deserialize(&mut deserializer)?;
        assert_eq!(restored.animal.species(), "wrapped");

        // Box<dyn Animal> round-trip.
        let box_value = AnimalHolderBox {
            animal: Box::new(Wrapper(Cat)),
        };
        let mut serializer = TestingSerializer::new();
        box_value.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored = AnimalHolderBox::pagable_deserialize(&mut deserializer)?;
        assert_eq!(restored.animal.species(), "wrapped");

        Ok(())
    }

    // --- Generic #[pagable_typetag] tests (link-section chaining) ---

    #[crate::pagable_typetag]
    pub trait Vehicle: PagableTagged + Send + Sync + Debug {
        fn wheels(&self) -> u32;
        fn as_any(&self) -> &dyn Any;
    }

    #[derive(Debug, Pagable)]
    pub struct GenericVehicle<T: Pagable + Send + Sync + Debug + 'static> {
        pub payload: T,
        pub wheel_count: u32,
    }

    #[derive(Debug, Pagable)]
    pub struct ConstGenericVehicle<const N: usize> {
        pub wheel_count: u32,
    }

    #[derive(Debug, Pagable)]
    pub struct TypeAndConstGenericVehicle<
        T: Pagable + Send + Sync + Debug + 'static,
        const N: usize,
    > {
        pub payload: T,
        pub wheel_count: u32,
    }

    #[crate::pagable_typetag]
    impl<T: Pagable + Send + Sync + Debug + 'static> Vehicle for GenericVehicle<T> {
        fn wheels(&self) -> u32 {
            self.wheel_count
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[crate::pagable_typetag]
    impl<const N: usize> Vehicle for ConstGenericVehicle<N> {
        fn wheels(&self) -> u32 {
            self.wheel_count
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[crate::pagable_typetag]
    impl<T: Pagable + Send + Sync + Debug + 'static, const N: usize> Vehicle
        for TypeAndConstGenericVehicle<T, N>
    {
        fn wheels(&self) -> u32 {
            self.wheel_count
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[derive(Debug, Pagable, Eq, PartialEq)]
    pub struct Cargo {
        pub weight: u32,
    }

    #[derive(Debug, Pagable, Eq, PartialEq)]
    pub struct Passenger {
        pub name: String,
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_generic_pagable_typetag_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Vehicle> = Arc::new(GenericVehicle {
            payload: Cargo { weight: 1000 },
            wheel_count: 4,
        });

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn Vehicle> = <dyn Vehicle>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.wheels(), 4);
        let restored = restored
            .as_any()
            .downcast_ref::<GenericVehicle<Cargo>>()
            .expect("restored vehicle should be GenericVehicle<Cargo>");
        assert_eq!(restored.payload.weight, 1000);

        let value2: Arc<dyn Vehicle> = Arc::new(GenericVehicle {
            payload: Passenger {
                name: "Alice".to_owned(),
            },
            wheel_count: 2,
        });

        let mut serializer = TestingSerializer::new();
        value2.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored2: Box<dyn Vehicle> = <dyn Vehicle>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored2.wheels(), 2);
        let restored2 = restored2
            .as_any()
            .downcast_ref::<GenericVehicle<Passenger>>()
            .expect("restored vehicle should be GenericVehicle<Passenger>");
        assert_eq!(restored2.payload.name, "Alice");

        Ok(())
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_const_generic_pagable_typetag_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Vehicle> = Arc::new(ConstGenericVehicle::<7> { wheel_count: 8 });

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn Vehicle> = <dyn Vehicle>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.wheels(), 8);
        restored
            .as_any()
            .downcast_ref::<ConstGenericVehicle<7>>()
            .expect("restored vehicle should be ConstGenericVehicle<7>");

        Ok(())
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_type_and_const_generic_pagable_typetag_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Arc<dyn Vehicle> = Arc::new(TypeAndConstGenericVehicle::<Cargo, 3> {
            payload: Cargo { weight: 250 },
            wheel_count: 6,
        });

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn Vehicle> = <dyn Vehicle>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.wheels(), 6);
        let restored = restored
            .as_any()
            .downcast_ref::<TypeAndConstGenericVehicle<Cargo, 3>>()
            .expect("restored vehicle should be TypeAndConstGenericVehicle<Cargo, 3>");
        assert_eq!(restored.payload.weight, 250);

        Ok(())
    }

    // `Arc<dyn Trait>` fields route through `serialize_tagged_arc_payload` /
    // `deserialize_arc_payload`, which generic impls provide manually instead
    // of via the `PagableTypeTag` blanket impl.
    #[derive(Debug, Pagable)]
    pub struct VehicleHolder {
        pub vehicle: Arc<dyn Vehicle>,
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_generic_pagable_typetag_arc_field_roundtrip() -> crate::Result<()> {
        use crate::PagableDeserialize;
        use crate::PagableSerialize;
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;

        let value = VehicleHolder {
            vehicle: Arc::new(GenericVehicle {
                payload: Cargo { weight: 500 },
                wheel_count: 6,
            }),
        };

        let mut serializer = TestingSerializer::new();
        value.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored = VehicleHolder::pagable_deserialize(&mut deserializer)?;

        assert_eq!(restored.vehicle.wheels(), 6);
        let restored_vehicle = restored
            .vehicle
            .as_any()
            .downcast_ref::<GenericVehicle<Cargo>>()
            .expect("restored vehicle should be GenericVehicle<Cargo>");
        assert_eq!(restored_vehicle.payload.weight, 500);

        Ok(())
    }

    // --- Generic struct-form tests: `#[pagable_typetag(Trait)]` on a generic
    // struct whose dyn-trait impl comes from a blanket impl (like starlark's
    // `impl<T: TyCustomImpl> TyCustomDyn for T`). The trait lives in a
    // different module to exercise accumulator resolution through the trait.

    mod blanket_dyn {
        use std::any::Any;
        use std::fmt::Debug;

        use pagable::PagableTagged;

        use crate as pagable;

        pub(crate) trait ShapeImpl: PagableTagged + Debug + Send + Sync + 'static {
            fn sides(&self) -> u32;
        }

        #[crate::pagable_typetag]
        pub(crate) trait ShapeDyn: PagableTagged + Debug + Send + Sync + 'static {
            fn sides_dyn(&self) -> u32;
            fn as_any(&self) -> &dyn Any;
        }

        impl<T: ShapeImpl> ShapeDyn for T {
            fn sides_dyn(&self) -> u32 {
                self.sides()
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }
    }

    mod blanket_wrapper {
        use std::fmt::Debug;

        use crate as pagable;
        use crate::Pagable;

        #[derive(Debug, Pagable)]
        #[crate::pagable_typetag(super::blanket_dyn::ShapeDyn)]
        pub(crate) struct Polygon<T: Pagable + Debug + Send + Sync + 'static> {
            pub payload: T,
            pub sides: u32,
        }

        impl<T: Pagable + Debug + Send + Sync + 'static> super::blanket_dyn::ShapeImpl for Polygon<T>
        where
            Self: pagable::PagableStableName,
        {
            fn sides(&self) -> u32 {
                self.sides
            }
        }
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_generic_struct_form_typetag_roundtrip() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value: Box<dyn blanket_dyn::ShapeDyn> = Box::new(blanket_wrapper::Polygon {
            payload: Cargo { weight: 9 },
            sides: 5,
        });
        assert_eq!(
            value.pagable_type_tag(),
            "pagable::typetag::tests::blanket_wrapper::Polygon<pagable::typetag::tests::Cargo>",
            "struct-form registration should use the same stable name scheme"
        );

        let mut serializer = TestingSerializer::new();
        value.serialize_tagged(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Box<dyn blanket_dyn::ShapeDyn> =
            <dyn blanket_dyn::ShapeDyn>::deserialize_box(&mut deserializer)?;

        assert_eq!(restored.sides_dyn(), 5);
        let restored = restored
            .as_any()
            .downcast_ref::<blanket_wrapper::Polygon<Cargo>>()
            .expect("restored shape should be Polygon<Cargo>");
        assert_eq!(restored.payload.weight, 9);

        Ok(())
    }

    // Pin the exact tag strings: they are persisted, so they must stay
    // source-derived (module path + idents) and never depend on
    // `std::any::type_name` formatting, which can change between compilers.
    #[test]
    fn test_typetag_tags_are_stable_names() {
        let generic = GenericVehicle {
            payload: Cargo { weight: 1 },
            wheel_count: 4,
        };
        assert_eq!(
            PagableTagged::pagable_type_tag(&generic),
            "pagable::typetag::tests::GenericVehicle<pagable::typetag::tests::Cargo>",
            "generic typetag should compose module paths of the wrapper and its arguments"
        );

        let const_generic = ConstGenericVehicle::<7> { wheel_count: 8 };
        assert_eq!(
            PagableTagged::pagable_type_tag(&const_generic),
            "pagable::typetag::tests::ConstGenericVehicle<7>",
            "const generic arguments should be Display-formatted"
        );

        let wrapper = Wrapper(Cat);
        assert_eq!(
            PagableTagged::pagable_type_tag(&wrapper),
            "pagable::typetag::tests::Wrapper<pagable::typetag::tests::Cat>",
            "pagable_tagged wrappers should use the same stable name scheme"
        );

        // Nested generics: composing the outer name re-enters
        // `memoized_stable_name` for the inner type, which must not deadlock
        // on the memoization lock.
        assert_eq!(
            <Vec<Vec<i32>> as crate::PagableStableName>::pagable_stable_name(),
            "Vec<Vec<i32>>",
            "nested generic names should compose recursively"
        );
    }

    mod same_type_name_one {
        use std::fmt::Debug;

        use crate as pagable;
        use crate::Pagable;

        #[derive(Debug, Pagable)]
        pub struct Wrapper<T: Pagable + Send + Sync + Debug + 'static> {
            pub payload: T,
            pub wheel_count: u32,
        }
    }

    mod same_type_name_two {
        use std::fmt::Debug;

        use crate as pagable;
        use crate::Pagable;

        #[derive(Debug, Pagable)]
        pub struct Wrapper<T: Pagable + Send + Sync + Debug + 'static> {
            pub payload: T,
            pub wheel_count: u32,
        }
    }

    #[crate::pagable_typetag]
    impl<T: Pagable + Send + Sync + Debug + 'static> Vehicle for same_type_name_one::Wrapper<T> {
        fn wheels(&self) -> u32 {
            self.wheel_count
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[crate::pagable_typetag]
    impl<T: Pagable + Send + Sync + Debug + 'static> Vehicle for same_type_name_two::Wrapper<T> {
        fn wheels(&self) -> u32 {
            self.wheel_count
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_same_type_name_generic_registrations_do_not_collide() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value_one: Arc<dyn Vehicle> = Arc::new(same_type_name_one::Wrapper {
            payload: Cargo { weight: 100 },
            wheel_count: 2,
        });
        let value_two: Arc<dyn Vehicle> = Arc::new(same_type_name_two::Wrapper {
            payload: Passenger {
                name: "Bob".to_owned(),
            },
            wheel_count: 3,
        });

        let mut serializer = TestingSerializer::new();
        value_one.serialize_tagged(&mut serializer)?;
        let bytes_one = serializer.finish();

        let mut serializer = TestingSerializer::new();
        value_two.serialize_tagged(&mut serializer)?;
        let bytes_two = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes_one);
        let restored_one: Box<dyn Vehicle> = <dyn Vehicle>::deserialize_box(&mut deserializer)?;
        assert_eq!(restored_one.wheels(), 2);
        let restored_one = restored_one
            .as_any()
            .downcast_ref::<same_type_name_one::Wrapper<Cargo>>()
            .expect("restored vehicle should be same_type_name_one::Wrapper<Cargo>");
        assert_eq!(restored_one.payload.weight, 100);

        let mut deserializer = TestingDeserializer::new(&bytes_two);
        let restored_two: Box<dyn Vehicle> = <dyn Vehicle>::deserialize_box(&mut deserializer)?;
        assert_eq!(restored_two.wheels(), 3);
        let restored_two = restored_two
            .as_any()
            .downcast_ref::<same_type_name_two::Wrapper<Passenger>>()
            .expect("restored vehicle should be same_type_name_two::Wrapper<Passenger>");
        assert_eq!(restored_two.payload.name, "Bob");

        Ok(())
    }

    mod collision_one {
        use std::any::Any;
        use std::fmt::Debug;

        use pagable::PagableTagged;

        use crate as pagable;
        use crate::Pagable;

        #[crate::pagable_typetag]
        pub trait Collision: PagableTagged + Send + Sync + Debug {
            fn label(&self) -> &'static str;
            fn as_any(&self) -> &dyn Any;
        }

        #[derive(Debug, Pagable)]
        pub struct Wrapper<T: Pagable + Send + Sync + Debug + 'static> {
            pub payload: T,
        }

        #[crate::pagable_typetag]
        impl<T: Pagable + Send + Sync + Debug + 'static> Collision for Wrapper<T> {
            fn label(&self) -> &'static str {
                "one"
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        #[derive(Debug, Pagable, Eq, PartialEq)]
        pub struct PayloadOne {
            pub value: u32,
        }
    }

    mod collision_two {
        use std::any::Any;
        use std::fmt::Debug;

        use pagable::PagableTagged;

        use crate as pagable;
        use crate::Pagable;

        #[crate::pagable_typetag]
        pub trait Collision: PagableTagged + Send + Sync + Debug {
            fn label(&self) -> &'static str;
            fn as_any(&self) -> &dyn Any;
        }

        #[derive(Debug, Pagable)]
        pub struct Wrapper<T: Pagable + Send + Sync + Debug + 'static> {
            pub payload: T,
        }

        #[crate::pagable_typetag]
        impl<T: Pagable + Send + Sync + Debug + 'static> Collision for Wrapper<T> {
            fn label(&self) -> &'static str {
                "two"
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        #[derive(Debug, Pagable, Eq, PartialEq)]
        pub struct PayloadTwo {
            pub value: u32,
        }
    }

    // Generic registrations come from a linker section, which Miri does not assemble.
    #[cfg(not(miri))]
    #[test]
    fn test_same_short_trait_name_generic_registrations_do_not_collide() -> crate::Result<()> {
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableBoxDeserialize;

        let value_one: Arc<dyn collision_one::Collision> = Arc::new(collision_one::Wrapper {
            payload: collision_one::PayloadOne { value: 1 },
        });
        let value_two: Arc<dyn collision_two::Collision> = Arc::new(collision_two::Wrapper {
            payload: collision_two::PayloadTwo { value: 2 },
        });

        let mut serializer = TestingSerializer::new();
        value_one.serialize_tagged(&mut serializer)?;
        let bytes_one = serializer.finish();

        let mut serializer = TestingSerializer::new();
        value_two.serialize_tagged(&mut serializer)?;
        let bytes_two = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes_one);
        let restored_one: Box<dyn collision_one::Collision> =
            <dyn collision_one::Collision>::deserialize_box(&mut deserializer)?;
        assert_eq!(restored_one.label(), "one");
        let restored_one = restored_one
            .as_any()
            .downcast_ref::<collision_one::Wrapper<collision_one::PayloadOne>>()
            .expect("restored value should be collision_one::Wrapper<PayloadOne>");
        assert_eq!(restored_one.payload.value, 1);

        let mut deserializer = TestingDeserializer::new(&bytes_two);
        let restored_two: Box<dyn collision_two::Collision> =
            <dyn collision_two::Collision>::deserialize_box(&mut deserializer)?;
        assert_eq!(restored_two.label(), "two");
        let restored_two = restored_two
            .as_any()
            .downcast_ref::<collision_two::Wrapper<collision_two::PayloadTwo>>()
            .expect("restored value should be collision_two::Wrapper<PayloadTwo>");
        assert_eq!(restored_two.payload.value, 2);

        Ok(())
    }
}
