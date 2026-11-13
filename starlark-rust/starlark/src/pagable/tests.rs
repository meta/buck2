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

//! Round-trip serialize/deserialize tests for Starlark values.

mod cycle_readiness;

use std::sync::Arc;

use allocative::Allocative;
use derive_more::Display;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use pagable::StorageContext;
use pagable::storage::traits::ArcSerCache;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::StarlarkPagableUnsupported;
use starlark_derive::starlark_value;
use starlark_syntax::codemap::CodeMap;
use starlark_syntax::codemap::FileSpan;
use starlark_syntax::codemap::NativeCodeMap;
use strong_hash::StrongHash;

use crate as starlark;
use crate::const_frozen_string;
use crate::environment::GlobalFrozenHeapName;
use crate::environment::GlobalsBuilder;
use crate::environment::MethodFrozenHeapName;
use crate::pagable::error::PagableError;
use crate::pagable::heap_ref_id::HeapRefId;
use crate::pagable::starlark_deserialization_state_retained_bytes;
use crate::pagable::starlark_deserialize_context::StarlarkDeserScope;
use crate::pagable::starlark_serialization_state_retained_bytes;
use crate::pagable::starlark_serialize_context::StarlarkSerState;
use crate::singleton_heap_name;
use crate::starlark_simple_value;
use crate::values::AllocFrozenValue;
use crate::values::Deferred;
use crate::values::FrozenHeap;
use crate::values::FrozenHeapName;
use crate::values::FrozenValueTyped;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenHeap;
use crate::values::OwnedFrozenRef;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::any::StarlarkAny;
use crate::values::any::StarlarkAnyRegistered;
use crate::values::any_complex::StarlarkAnyComplex;
use crate::values::dict::globals::register_dict;
use crate::values::layout::avalue::AValueSimpleBound;
use crate::values::layout::heap::sealed::HeapAllocationOrigin;
use crate::values::layout::typed::AtomicFrozenValueTypedOption;
use crate::values::list::AllocList;
use crate::values::list::globals::register_list;
use crate::values::none::NoneType;
use crate::values::tuple::AllocTuple;
use crate::values::tuple::value::VALUE_EMPTY_TUPLE;
use crate::values::types::any_array::AnyArrayRegistered;

pagable::static_str!(TEST_EVAL_HEAP_NAME = "test_eval");
pagable::static_str!(TESTING_GLOBALS_HEAP_NAME = "testing");
pagable::static_str!(CROSS_THREAD_GLOBALS_HEAP_NAME = "cross_thread");
pagable::static_str!(BENCH_EVAL_HEAP_NAME = "bench_eval");
pagable::static_str!(TEST_BCINSTRS_HEAP_NAME = "test_bcinstrs");
pagable::static_str!(TEST_BCINSTRS_DET_GLOBALS_HEAP_NAME = "test_bcinstrs_det_globals");
pagable::static_str!(METHOD_TEST_HEAP_NAME = "method_test");

/// A frozen heap under construction whose allocations come back brand-erased.
///
/// These tests hand values straight to the pagable plumbing underneath the branded API and build
/// heaps that reference each other freely, so they store values as `Value<'static>` the way the
/// owning carriers do internally (`OwnedFrozen::erase_brand`). Production code keeps the brand
/// that [`FrozenHeap`] hands out. Every test keeps the sealed heap alive for as long as it uses
/// the values erased from it, and only allocates erased values into the heap they came from or
/// into a heap that references it.
struct ErasingHeap(OwnedFrozenHeap);

/// See [`ErasingHeap`].
fn erase(v: Value<'_>) -> Value<'static> {
    // SAFETY: The test keeps the owning heap alive, see `ErasingHeap`.
    unsafe { OwnedFrozen::<Value<'static>>::erase_brand(v) }
}

impl ErasingHeap {
    fn new() -> Self {
        Self(OwnedFrozenHeap::new())
    }

    fn with<R>(&self, f: impl for<'fh> FnOnce(FrozenHeap<'fh>) -> R) -> R {
        self.0.with(f)
    }

    fn alloc<T: for<'fh> AllocFrozenValue<'fh>>(&self, x: T) -> Value<'static> {
        self.with(|heap| erase(heap.alloc(x).to_value()))
    }

    fn alloc_simple<T: for<'fh> AValueSimpleBound<'fh>>(&self, x: T) -> Value<'static> {
        self.with(|heap| erase(heap.alloc_simple(x)))
    }

    fn alloc_str(&self, s: &str) -> Value<'static> {
        self.with(|heap| erase(heap.alloc_str(s).to_value()))
    }

    /// Bring an erased value back to the heap's brand, for allocating values that hold it.
    fn restore_one<'fh>(_heap: FrozenHeap<'fh>, v: Value<'static>) -> Value<'fh> {
        // SAFETY: The value was erased from this heap or from a heap it references, see
        // `ErasingHeap`.
        unsafe { OwnedFrozen::<Value<'static>>::restore_brand(v) }
    }

    /// Bring erased values back to the heap's brand, for allocating containers of them.
    fn restore<'fh>(heap: FrozenHeap<'fh>, elems: &[Value<'static>]) -> Vec<Value<'fh>> {
        elems.iter().map(|v| Self::restore_one(heap, *v)).collect()
    }

    /// Allocate a [`RefData`] pointing at `target`.
    fn alloc_ref_data(&self, label: usize, target: Value<'static>) -> Value<'static> {
        self.with(|heap| {
            erase(heap.alloc_simple(RefData {
                label,
                target: Self::restore_one(heap, target),
            }))
        })
    }

    fn alloc_list(&self, elems: &[Value<'static>]) -> Value<'static> {
        self.with(|heap| erase(heap.alloc(AllocList(Self::restore(heap, elems))).to_value()))
    }

    fn alloc_tuple(&self, elems: &[Value<'static>]) -> Value<'static> {
        self.with(|heap| {
            erase(
                heap.alloc(AllocTuple(Self::restore(heap, elems)))
                    .to_value(),
            )
        })
    }

    fn alloc_any_value<T: StarlarkAnyRegistered>(&self, x: T) -> Value<'static> {
        self.alloc_simple(StarlarkAny::new(x))
    }

    fn alloc_any_array_value<
        T: AnyArrayRegistered + for<'fv> crate::pagable::StarlarkPagable<'fv> + Send + Sync + Clone,
    >(
        &self,
        xs: &[T],
    ) -> Value<'static> {
        self.with(|heap| erase(heap.alloc_any_array_value(xs).to_value()))
    }

    fn add_reference(&self, heap: OwnedFrozenRef<'_, ()>) {
        self.with(|h| h.add_reference(heap))
    }

    fn into_ref_named(self, name: FrozenHeapName) -> OwnedFrozen<()> {
        self.0.seal(name)
    }
}

/// Private test heap name for pagable tests.
#[derive(Clone, derive_more::Display, Debug, Hash, StrongHash, Pagable)]
#[pagable::pagable_typetag(crate::values::UserHeapName)]
#[display("TestHeapName({})", _0)]
struct TestHeapName(String);

impl TestHeapName {
    fn heap_name(name: &str) -> FrozenHeapName {
        FrozenHeapName::User(Box::new(Self(name.to_owned())))
    }

    fn name_of(name: &FrozenHeapName) -> &str {
        let FrozenHeapName::User(name) = name else {
            panic!("expected a user heap name");
        };
        &name.as_any().downcast_ref::<Self>().unwrap().0
    }
}

/// A simple test type with primitive fields.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("SimpleData({}, {})", self.flag, self.count)]
struct SimpleData {
    flag: bool,
    count: usize,
}

starlark_simple_value!(SimpleData);

#[starlark_value(type = "SimpleData")]
impl<'v> StarlarkValue<'v> for SimpleData {}

/// A value that cannot be paged, as a heap holding one must say so without panicking.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagableUnsupported
)]
#[display("Unpagable")]
struct Unpagable;

starlark_simple_value!(Unpagable);

#[starlark_value(type = "Unpagable")]
impl<'v> StarlarkValue<'v> for Unpagable {}

#[test]
fn unsupported_value_fails_page_out_instead_of_panicking() {
    let owned: OwnedFrozen<Value> =
        OwnedFrozen::build(TestHeapName::heap_name("test_unpagable"), |heap| {
            heap.alloc_simple(Unpagable)
        });
    let mut ser = pagable::testing::TestingSerializer::new();
    let error = owned
        .pagable_serialize(&mut ser)
        .expect_err("a heap holding an unpagable value fails to page out");
    assert!(
        format!("{error:#}").contains("`Unpagable` cannot be paged out"),
        "{error:#}"
    );
}

/// Round-trip an `OwnedFrozen` through the testing serializer. Returns the restored value.
fn round_trip_owned(
    heap_ref: OwnedFrozen<()>,
    root_fv: Value<'static>,
) -> crate::Result<OwnedFrozen<Value<'static>>> {
    // SAFETY: `heap_ref` owns the arena hosting `root_fv`.
    let owned: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(heap_ref, root_fv) };
    let mut ser = pagable::testing::TestingSerializer::new();
    owned
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let bytes = ser.finish();

    let mut de = pagable::testing::TestingDeserializer::new(&bytes);
    <OwnedFrozen<Value>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)
}

fn round_trip_heap_ref(heap_ref: OwnedFrozenRef<'_, ()>) -> crate::Result<OwnedFrozen<()>> {
    let mut ser = pagable::testing::TestingSerializer::new();
    heap_ref
        .heap_arc()
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let bytes = ser.finish();

    let mut de = pagable::testing::TestingDeserializer::new(&bytes);
    let restored =
        OwnedFrozen::<()>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    Ok(restored)
}

fn round_trip_heap_name(name: &FrozenHeapName) -> crate::Result<FrozenHeapName> {
    let mut ser = pagable::testing::TestingSerializer::new();
    name.pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let bytes = ser.finish();

    let mut de = pagable::testing::TestingDeserializer::new(&bytes);
    FrozenHeapName::pagable_deserialize(&mut de).map_err(crate::Error::new_other)
}

#[test]
fn test_frozen_heap_name_round_trip() -> crate::Result<()> {
    let names = [
        FrozenHeapName::Global(GlobalFrozenHeapName {
            name: TEST_EVAL_HEAP_NAME,
        }),
        FrozenHeapName::Method(MethodFrozenHeapName {
            name: METHOD_TEST_HEAP_NAME,
        }),
        FrozenHeapName::Singleton(singleton_heap_name!()),
        TestHeapName::heap_name("user_test"),
    ];

    for name in names {
        let expected_id = HeapRefId::from_heap_name(&name);
        let expected_display = name.to_string();
        let restored = round_trip_heap_name(&name)?;
        assert_eq!(HeapRefId::from_heap_name(&restored), expected_id);
        assert_eq!(restored.to_string(), expected_display);

        if let FrozenHeapName::User(user) = restored {
            assert_eq!(
                user.as_any().downcast_ref::<TestHeapName>().unwrap().0,
                "user_test"
            );
        }
    }

    Ok(())
}

#[test]
fn test_frozen_heap_ref_round_trip_preserves_name() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    heap.alloc("value");
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("preserved_name"));
    let expected_id = heap_ref.heap_arc().heap_ref_id().unwrap();

    let restored = round_trip_heap_ref(heap_ref.owner())?;
    let restored_name = restored.name().expect("heap name should round-trip");
    assert_eq!(
        restored.heap_arc().heap_ref_id().unwrap(),
        expected_id,
        "the restored heap is the same heap, so it has the same identity"
    );
    assert_eq!(restored_name.to_string(), "TestHeapName(preserved_name)");
    Ok(())
}

/// The bare heap handle round-trips for all three kinds of heap, each under its own wire tag:
/// empty, paged, and registered static.
#[test]
fn test_owned_frozen_unit_round_trip() -> crate::Result<()> {
    fn round_trip(heap: &OwnedFrozen<()>, expected_tag: u8) -> crate::Result<OwnedFrozen<()>> {
        let mut ser = pagable::testing::TestingSerializer::new();
        heap.pagable_serialize(&mut ser)
            .map_err(crate::Error::new_other)?;
        let bytes = ser.finish();
        assert_eq!(bytes[0], expected_tag);
        let mut de = pagable::testing::TestingDeserializer::new(&bytes);
        <OwnedFrozen<()>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)
    }

    let restored = round_trip(&OwnedFrozen::<()>::default(), 0)?;
    assert!(restored == OwnedFrozen::<()>::default());

    let heap = ErasingHeap::new();
    heap.alloc("value");
    let heap = heap.into_ref_named(TestHeapName::heap_name("owned_frozen_unit_round_trip"));
    let restored = round_trip(&heap, 1)?;
    assert_eq!(
        restored.name().map(ToString::to_string),
        heap.name().map(ToString::to_string)
    );

    let static_heap = PAGABLE_TEST_STATIC_GLOBALS.globals().heap().to_owned();
    let restored = round_trip(&static_heap, 2)?;
    assert!(
        restored == static_heap,
        "a registered static heap resolves to the live allocation"
    );
    Ok(())
}

#[test]
fn test_module_eval_round_trip() -> crate::Result<()> {
    use std::any::TypeId;

    use pagable::PagableDeserialize;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;
    use pagable::storage::traits::PageOutError;

    use crate::environment::FrozenModule;
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;

    let code = r#"
x = 1 + 2
y = "hello" + " world"
a = [1]
a.append(a)
"#;

    let ast = AstModule::parse("test_module.star", code.to_owned(), &Dialect::Extended)?;
    let globals = GlobalsBuilder::new().build_named(GlobalFrozenHeapName {
        name: TEST_EVAL_HEAP_NAME,
    });
    let frozen_module = Module::with_temp_heap(|module| {
        {
            let mut eval = Evaluator::new(&module);
            eval.eval_module(ast, &globals).unwrap();
        }
        module.freeze_named(TestHeapName::heap_name("test_module_eval"))
    })?;

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let handle = PagableStorageHandle::new(storage.clone());
    let storage_ctx = storage.storage_context();

    let mut ser = SerializerForPaging::new(storage_ctx);
    frozen_module
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();

    let top_key = {
        let finished = ArcSerCache::new();
        storage
            .page_out_item(data, arcs, &finished, storage_ctx)
            .map_err(|e| match e {
                PageOutError::Failed(e) => crate::Error::new_other(e),
                PageOutError::AlreadyFailed => {
                    crate::Error::new_other(anyhow::anyhow!("page out reported AlreadyFailed"))
                }
            })?
    };

    let top_data = storage
        .fetch_arc_or_data_blocking(&TypeId::of::<()>(), &top_key)
        .map_err(crate::Error::new_other)?
        .right()
        .expect("top-level key should return data, not a cached arc");

    let mut de = handle.root_deserializer(top_key, &top_data);
    let restored = FrozenModule::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;

    let x = restored.get("x")?.as_ref().value().unpack_i32().unwrap();
    assert_eq!(x, 3);

    let y = restored.get("y")?;
    assert_eq!(y.as_ref().value().unpack_str().unwrap(), "hello world");

    let a = restored.get("a")?;
    assert_eq!(a.as_ref().value().length().unwrap(), 2);

    Ok(())
}
/// A test type with rust heap-allocated fields (Vec, Box, String).
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("HeapData({:?}, {:?}, {:?})", self.items, self.label, self.boxed)]
struct HeapData {
    items: Vec<u32>,
    label: String,
    boxed: Box<i64>,
}

starlark_simple_value!(HeapData);

#[starlark_value(type = "HeapData")]
impl<'v> StarlarkValue<'v> for HeapData {}

#[test]
fn test_simple_value_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(SimpleData {
        flag: true,
        count: 42,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_simple_value"));

    let restored = round_trip_owned(heap_ref, root)?;
    let data: &SimpleData = restored
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .unwrap();
    assert_eq!(data.flag, true);
    assert_eq!(data.count, 42);

    Ok(())
}

#[test]
fn test_heap_allocated_value_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(HeapData {
        items: vec![10, 20, 30],
        label: "hello".to_owned(),
        boxed: Box::new(-99),
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_heap_data_round_trip"));

    let restored = round_trip_owned(heap_ref, root)?;
    let data: &HeapData = restored
        .as_ref()
        .value()
        .downcast_ref::<HeapData>()
        .unwrap();
    assert_eq!(data.items, vec![10, 20, 30]);
    assert_eq!(data.label, "hello");
    assert_eq!(*data.boxed, -99);

    Ok(())
}

/// A branded test type: it is its own frozen form, so `frozen_vtable` registers it.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("BrandedData({}, {})", self.label, self.inner)]
struct BrandedData<'v> {
    label: String,
    inner: Value<'v>,
}

#[starlark_value(type = "BrandedData", frozen_vtable)]
impl<'v> StarlarkValue<'v> for BrandedData<'v> {}

#[test]
fn test_frozen_vtable_flag_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.with(|heap| {
        let inner = heap.alloc("inner").to_value();
        erase(heap.alloc_simple(BrandedData {
            label: "branded".to_owned(),
            inner,
        }))
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_vtable_flag"));

    let restored = round_trip_owned(heap_ref, root)?;
    let data: &BrandedData = restored
        .as_ref()
        .value()
        .downcast_ref::<BrandedData>()
        .unwrap();
    assert_eq!(data.label, "branded");
    assert_eq!(data.inner.unpack_str(), Some("inner"));

    Ok(())
}

/// A test type with a value field that references another value in the same heap.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("RefData({})", self.label)]
struct RefData<'v> {
    label: usize,
    target: Value<'v>,
}

#[starlark_value(type = "RefData", frozen_vtable)]
impl<'v> StarlarkValue<'v> for RefData<'v> {}

#[test]
fn test_frozen_value_ref_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let target_fv = heap.alloc_simple(SimpleData {
        flag: true,
        count: 99,
    });
    let root = heap.alloc_ref_data(7, target_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    assert_eq!(ref_data.label, 7);

    let resolved: &SimpleData = ref_data.target.downcast_ref::<SimpleData>().unwrap();
    assert_eq!(resolved.flag, true);
    assert_eq!(resolved.count, 99);

    Ok(())
}

/// A test type with Drop (due to Vec) that holds a value reference.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("DropRefData({:?})", self.items)]
struct DropRefData<'v> {
    items: Vec<u32>,
    target: Value<'v>,
}

#[starlark_value(type = "DropRefData", frozen_vtable)]
impl<'v> StarlarkValue<'v> for DropRefData<'v> {}

#[test]
fn test_frozen_value_drop_to_undrop_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let target_fv = heap.alloc_simple(SimpleData {
        flag: false,
        count: 123,
    });
    let root = heap.with(|h| {
        erase(h.alloc_simple(DropRefData {
            items: vec![1, 2, 3],
            target: ErasingHeap::restore_one(h, target_fv),
        }))
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let drop_ref_data: &DropRefData = restored
        .as_ref()
        .value()
        .downcast_ref::<DropRefData>()
        .unwrap();
    assert_eq!(drop_ref_data.items, vec![1, 2, 3]);

    let resolved: &SimpleData = drop_ref_data.target.downcast_ref::<SimpleData>().unwrap();
    assert_eq!(resolved.flag, false);
    assert_eq!(resolved.count, 123);

    Ok(())
}

#[test]
fn test_frozen_value_undrop_to_drop_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let target_fv = heap.alloc_simple(HeapData {
        items: vec![10, 20],
        label: "target".to_owned(),
        boxed: Box::new(42),
    });
    let root = heap.alloc_ref_data(5, target_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    assert_eq!(ref_data.label, 5);

    let resolved: &HeapData = ref_data.target.downcast_ref::<HeapData>().unwrap();
    assert_eq!(resolved.items, vec![10, 20]);
    assert_eq!(resolved.label, "target");
    assert_eq!(*resolved.boxed, 42);

    Ok(())
}

#[test]
fn test_frozen_list_round_trip() -> crate::Result<()> {
    use crate::values::list::value::ListGen;
    use crate::values::types::list::value::FrozenListData;

    // Create a heap with SimpleData values and a frozen list referencing them.
    let heap = ErasingHeap::new();
    let a = heap.alloc_simple(SimpleData {
        flag: true,
        count: 10,
    });
    let b = heap.alloc_simple(SimpleData {
        flag: false,
        count: 20,
    });
    let root = heap.alloc_list(&[a, b]);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let list_value: &ListGen<FrozenListData<'_>> = restored
        .as_ref()
        .value()
        .downcast_ref::<ListGen<FrozenListData<'_>>>()
        .unwrap();
    let content = list_value.0.content();
    assert_eq!(content.len(), 2);

    let elem_a: &SimpleData = content[0].downcast_ref::<SimpleData>().unwrap();
    assert_eq!(elem_a.flag, true);
    assert_eq!(elem_a.count, 10);

    let elem_b: &SimpleData = content[1].downcast_ref::<SimpleData>().unwrap();
    assert_eq!(elem_b.flag, false);
    assert_eq!(elem_b.count, 20);

    Ok(())
}

#[test]
fn test_frozen_tuple_round_trip() -> crate::Result<()> {
    use crate::values::types::tuple::value::Tuple;

    // Create a heap with SimpleData values and a frozen tuple referencing them.
    let heap = ErasingHeap::new();
    let a = heap.alloc_simple(SimpleData {
        flag: true,
        count: 100,
    });
    let b = heap.alloc_simple(SimpleData {
        flag: false,
        count: 200,
    });
    let root = heap.alloc_tuple(&[a, b]);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let tuple_value: &Tuple = restored.as_ref().value().downcast_ref::<Tuple>().unwrap();
    let content = tuple_value.content();
    assert_eq!(content.len(), 2);

    let elem_a: &SimpleData = content[0].downcast_ref::<SimpleData>().unwrap();
    assert_eq!(elem_a.flag, true);
    assert_eq!(elem_a.count, 100);

    let elem_b: &SimpleData = content[1].downcast_ref::<SimpleData>().unwrap();
    assert_eq!(elem_b.flag, false);
    assert_eq!(elem_b.count, 200);

    Ok(())
}

#[test]
fn test_frozen_str_value_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let str_fv = heap.alloc_str("hello world");
    let root = heap.alloc_ref_data(42, str_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    assert_eq!(ref_data.label, 42);
    assert!(ref_data.target.is_str());
    assert_eq!(ref_data.target.unpack_str().unwrap(), "hello world");

    Ok(())
}

#[test]
fn test_frozen_value_inline_int_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let int_fv = Value::testing_new_int(42);
    let root = heap.alloc_ref_data(1, int_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test"));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    assert_eq!(ref_data.label, 1);
    assert_eq!(ref_data.target.unpack_i32(), Some(42));

    Ok(())
}

#[test]
fn test_cross_heap_frozen_value_round_trip() -> crate::Result<()> {
    let dep_heap = ErasingHeap::new();
    let dep_fv = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 77,
    });
    let dep_heap_ref = dep_heap.into_ref_named(TestHeapName::heap_name("dep"));

    let main_heap = ErasingHeap::new();
    main_heap.add_reference(dep_heap_ref.owner());
    let root = main_heap.alloc_ref_data(99, dep_fv);
    let main_heap_ref = main_heap.into_ref_named(TestHeapName::heap_name("main"));

    let restored = round_trip_owned(main_heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    assert_eq!(ref_data.label, 99);

    let resolved: &SimpleData = ref_data.target.downcast_ref::<SimpleData>().unwrap();
    assert_eq!(resolved.flag, true);
    assert_eq!(resolved.count, 77);

    Ok(())
}

#[test]
fn test_heap_ref_dedup_round_trip() -> crate::Result<()> {
    use crate::values::list::value::ListGen;
    use crate::values::types::list::value::FrozenListData;

    // Diamond: A refs [B, C], both B and C ref D. After round-trip the
    // restored B and C should share the same D arc.

    let heap_d = ErasingHeap::new();
    let d_fv = heap_d.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    let heap_d_ref = heap_d.into_ref_named(TestHeapName::heap_name("d"));

    let heap_b = ErasingHeap::new();
    heap_b.add_reference(heap_d_ref.owner());
    let b_root = heap_b.alloc_ref_data(2, d_fv);
    let heap_b_ref = heap_b.into_ref_named(TestHeapName::heap_name("b"));

    let heap_c = ErasingHeap::new();
    heap_c.add_reference(heap_d_ref.owner());
    let c_root = heap_c.alloc_ref_data(3, d_fv);
    let heap_c_ref = heap_c.into_ref_named(TestHeapName::heap_name("c"));

    // Link the two roots through a list in a top heap, so that a single owner reaches both
    // heaps' bindings transitively.
    let heap_a = ErasingHeap::new();
    heap_a.add_reference(heap_b_ref.owner());
    heap_a.add_reference(heap_c_ref.owner());
    let root = heap_a.alloc_list(&[b_root, c_root]);
    let heap_a_ref = heap_a.into_ref_named(TestHeapName::heap_name("a"));

    let restored = round_trip_owned(heap_a_ref, root)?;

    // Walk via the list root.
    let list_value: &ListGen<FrozenListData<'_>> = restored
        .as_ref()
        .value()
        .downcast_ref::<ListGen<FrozenListData<'_>>>()
        .unwrap();
    let content = list_value.0.content();
    assert_eq!(content.len(), 2);

    let b_data: &RefData = content[0].downcast_ref::<RefData>().unwrap();
    let c_data: &RefData = content[1].downcast_ref::<RefData>().unwrap();
    assert_eq!(b_data.label, 2);
    assert_eq!(c_data.label, 3);

    // Both should resolve to the same shared SimpleData in heap_d. Same target
    // ptr proves they share the same materialized header (in the dedup'd D).
    assert_eq!(
        b_data.target.ptr_value().ptr_value_untagged(),
        c_data.target.ptr_value().ptr_value_untagged(),
    );
    let d_data: &SimpleData = b_data.target.downcast_ref::<SimpleData>().unwrap();
    assert_eq!(d_data.flag, true);
    assert_eq!(d_data.count, 1);

    Ok(())
}

/// Three heaps with chunks interleaved by address; cross-heap
/// values in heap B target known interior values in A and C.
/// Asserts `state.lookup_ptr` returns the right `(heap_id, value_index)`
/// for every registered header, regardless of cross-heap address mixing.
#[test]
fn test_ser_state_lookup_resolves_cross_heap_ptrs() -> crate::Result<()> {
    use std::mem;

    use crate::pagable::heap_ref_id::HeapRefId;
    use crate::pagable::starlark_serialize_context::StarlarkSerState;
    use crate::values::layout::heap::repr::AValueHeader;

    // Enough values to push each heap into multiple chunks (geometric
    // chunk sizes 512, 1024, ... → ~500 SimpleData spans 3-5 chunks).
    const VALUES_PER_HEAP: usize = 500;

    // Build a leaf heap of `SimpleData`; returns the 7th allocation
    // as a known cross-heap target (deep enough to land past chunk 0).
    fn build_leaf_heap(name: &str) -> (OwnedFrozen<()>, HeapRefId, Vec<usize>, Value<'static>) {
        let heap = ErasingHeap::new();
        let mut allocated = Vec::with_capacity(VALUES_PER_HEAP);
        for v in 0..VALUES_PER_HEAP {
            allocated.push(heap.alloc_simple(SimpleData {
                flag: (v & 1) == 0,
                count: v,
            }));
        }
        let target = allocated[7];
        let heap_ref = heap.into_ref_named(TestHeapName::heap_name(name));
        let heap_id = heap_ref.heap_arc().heap_ref_id().unwrap();
        let ptrs_in_serialization_order: Vec<usize> = heap_ref
            .heap_arc()
            .collect_undrop_headers_ordered()
            .iter()
            .map(|hp| hp.payload_ptr().ptr as usize)
            .collect();
        (heap_ref, heap_id, ptrs_in_serialization_order, target)
    }

    let (heap_a_ref, heap_a_id, heap_a_ptrs, target_in_a) = build_leaf_heap("ser_state_lookup_A");
    let (heap_c_ref, heap_c_id, heap_c_ptrs, target_in_c) = build_leaf_heap("ser_state_lookup_C");

    let heap_b = ErasingHeap::new();
    heap_b.add_reference(heap_a_ref.owner());
    heap_b.add_reference(heap_c_ref.owner());
    heap_b.alloc_ref_data(0, target_in_a);
    heap_b.alloc_ref_data(1, target_in_c);
    for v in 0..VALUES_PER_HEAP {
        heap_b.alloc_simple(SimpleData {
            flag: (v & 1) == 0,
            count: v,
        });
    }
    let heap_b_ref = heap_b.into_ref_named(TestHeapName::heap_name("ser_state_lookup_B"));
    let heap_b_id = heap_b_ref.heap_arc().heap_ref_id().unwrap();
    // RefData + SimpleData are both non-drop, so heap B has only a
    // non-drop bump.
    let heap_b_ptrs: Vec<usize> = heap_b_ref
        .heap_arc()
        .collect_undrop_headers_ordered()
        .iter()
        .map(|hp| hp.payload_ptr().ptr as usize)
        .collect();

    let b_undrop_headers = heap_b_ref.heap_arc().collect_undrop_headers_ordered();
    assert!(b_undrop_headers.len() >= 2);
    let b_ref_to_a: &RefData = b_undrop_headers[0].unpack().downcast_ref().unwrap();
    let b_ref_to_c: &RefData = b_undrop_headers[1].unpack().downcast_ref().unwrap();
    assert_eq!(b_ref_to_a.label, 0);
    assert_eq!(b_ref_to_c.label, 1);
    let ptr_in_a = b_ref_to_a.target.get_ref().value.ptr as usize;
    let ptr_in_c = b_ref_to_c.target.get_ref().value.ptr as usize;

    // Guard against a regression where each heap fits in one chunk —
    // the chunk-spanning lookup arithmetic would never get exercised.
    let a_chunks = heap_a_ref.heap_arc().build_chunk_index().len();
    let b_chunks = heap_b_ref.heap_arc().build_chunk_index().len();
    let c_chunks = heap_c_ref.heap_arc().build_chunk_index().len();
    eprintln!(
        "chunks per heap: A={a_chunks} B={b_chunks} C={c_chunks} \
         (values_per_heap={VALUES_PER_HEAP})"
    );
    assert!(a_chunks >= 2, "heap A should span >= 2 chunks");
    assert!(b_chunks >= 2, "heap B should span >= 2 chunks");
    assert!(c_chunks >= 2, "heap C should span >= 2 chunks");

    let state = Arc::new(StarlarkSerState::new());
    state
        .ensure_chunk_index_registered(heap_a_ref.heap_arc())
        .expect("register heap A chunk index");
    state
        .ensure_chunk_index_registered(heap_b_ref.heap_arc())
        .expect("register heap B chunk index");
    state
        .ensure_chunk_index_registered(heap_c_ref.heap_arc())
        .expect("register heap C chunk index");

    for (heap, heap_id, ptrs) in [
        (&heap_a_ref, heap_a_id, &heap_a_ptrs),
        (&heap_b_ref, heap_b_id, &heap_b_ptrs),
        (&heap_c_ref, heap_c_id, &heap_c_ptrs),
    ] {
        let heap_ptr = heap
            .heap_arc()
            .downgrade()
            .expect("heap should have an allocation")
            .heap_ptr();
        for (expected_index, &raw_ptr) in ptrs.iter().enumerate() {
            let (got_heap, got_index) = state.lookup_ptr(raw_ptr).unwrap_or_else(|| {
                panic!("ptr {raw_ptr:#x} unexpectedly missing from chunk index")
            });
            assert_eq!(got_heap, heap_id, "ptr {raw_ptr:#x} routed to wrong heap");
            assert_eq!(
                got_index, expected_index as u32,
                "ptr {raw_ptr:#x}: got value_index {got_index}, expected {expected_index}",
            );
            let resident_value = state
                .lookup_registered_value(heap_ptr, expected_index as u32, false)
                .unwrap_or_else(|| {
                    panic!("value_index {expected_index} missing from resident heap {heap_id:?}")
                });
            assert_eq!(
                resident_value.ptr_value().ptr_value_untagged() + mem::size_of::<AValueHeader>(),
                raw_ptr,
                "value_index {expected_index} resolved to the wrong resident allocation",
            );
        }
    }

    // Cross-heap pointers in B's RefData must resolve into A / C, not B.
    let (got_heap, got_index_in_a) = state.lookup_ptr(ptr_in_a).expect("target in A");
    assert_eq!(got_heap, heap_a_id);
    assert_eq!(got_index_in_a, 7);

    let (got_heap, got_index_in_c) = state.lookup_ptr(ptr_in_c).expect("target in C");
    assert_eq!(got_heap, heap_c_id);
    assert_eq!(got_index_in_c, 7);

    // B's two RefData headers occupy value_index 0/1 (non-drop bump is
    // the whole of B's serialization order — RefData has no Drop).
    let b_ref_a_ptr = b_undrop_headers[0].payload_ptr().ptr as usize;
    let b_ref_c_ptr = b_undrop_headers[1].payload_ptr().ptr as usize;
    assert_eq!(state.lookup_ptr(b_ref_a_ptr), Some((heap_b_id, 0)));
    assert_eq!(state.lookup_ptr(b_ref_c_ptr), Some((heap_b_id, 1)));

    Ok(())
}

#[test]
fn test_heap_registration_supports_different_ser_states() {
    let heap = ErasingHeap::new();
    heap.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("single_ser_state"));

    let first_state = Arc::new(StarlarkSerState::new());
    first_state
        .ensure_chunk_index_registered(heap_ref.heap_arc())
        .expect("register heap in first state");
    first_state
        .ensure_chunk_index_registered(heap_ref.heap_arc())
        .expect("repeat registration in the same state");

    let second_state = Arc::new(StarlarkSerState::new());
    second_state
        .ensure_chunk_index_registered(heap_ref.heap_arc())
        .expect("register heap in a different state");
}

#[test]
fn test_ser_state_retained_bytes_tracks_registered_heap_memory() {
    let storage = StorageContext::new();
    assert_eq!(starlark_serialization_state_retained_bytes(&storage), 0);

    let state = storage.get_or_init(StarlarkSerState::new);
    let empty_bytes = starlark_serialization_state_retained_bytes(&storage);

    let heap = ErasingHeap::new();
    for i in 0..500 {
        let _ = heap.alloc_str(&format!("value-{i}"));
    }
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("ser_state_retained_bytes"));
    state
        .ensure_chunk_index_registered(heap_ref.heap_arc())
        .expect("register heap chunk index");

    let populated_bytes = starlark_serialization_state_retained_bytes(&storage);
    assert!(
        populated_bytes > empty_bytes,
        "registered chunk metadata should increase retained bytes",
    );

    drop(heap_ref);
    assert!(
        starlark_serialization_state_retained_bytes(&storage) < populated_bytes,
        "dropping the heap should release its registered chunk metadata",
    );
}

#[test]
fn test_deser_state_retained_bytes_tracks_cached_heap_memory() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    let heap = ErasingHeap::new();
    let value = heap.alloc_simple(SimpleData {
        flag: true,
        count: 42,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("deser_state_retained_bytes"));
    // SAFETY: `heap_ref` owns the arena hosting `value`.
    let root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(heap_ref, value) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let root_key = ser_owned_frozen_value_into_storage(&backing, &root)?;
    assert_eq!(starlark_deserialization_state_retained_bytes(&handle), 0);

    drop(root);
    let restored = deser_owned_frozen_from_storage(&backing, &handle, &root_key)?;
    let retained_bytes = starlark_deserialization_state_retained_bytes(&handle);
    assert!(
        retained_bytes > 0,
        "a cached deserialized heap should retain its deserialization state",
    );

    let restored_again = deser_owned_frozen_from_storage(&backing, &handle, &root_key)?;
    assert_eq!(
        starlark_deserialization_state_retained_bytes(&handle),
        retained_bytes,
        "reusing the cached heap must not duplicate its deserialization state",
    );

    drop((restored, restored_again));
    assert_eq!(
        starlark_deserialization_state_retained_bytes(&handle),
        0,
        "the deserialized Arc cache must not keep a dropped heap alive",
    );
    Ok(())
}

#[test]
fn test_deser_scope_rejects_conflicting_live_heap_binding() {
    let make_heap = |count| {
        let heap = ErasingHeap::new();
        heap.alloc_simple(SimpleData { flag: true, count });
        heap.into_ref_named(TestHeapName::heap_name("conflicting_deser_binding"))
    };

    let first = make_heap(1);
    let second = make_heap(2);
    let heap_id = first
        .heap_arc()
        .heap_ref_id()
        .expect("heap should have a name");
    let scope = StarlarkDeserScope::new(Default::default(), Default::default());

    scope
        .register_heap(
            heap_id,
            first
                .heap_arc()
                .downgrade()
                .expect("heap should have an allocation"),
        )
        .expect("first binding should be accepted");
    scope
        .register_heap(
            heap_id,
            first
                .heap_arc()
                .downgrade()
                .expect("heap should have an allocation"),
        )
        .expect("repeating the exact binding should be accepted");

    let error = scope
        .register_heap(
            heap_id,
            second
                .heap_arc()
                .downgrade()
                .expect("heap should have an allocation"),
        )
        .expect_err("a different live heap with the same ID should be rejected");
    assert!(matches!(
        &error,
        PagableError::ConflictingHeapBinding {
            heap_id: actual,
            heap_name,
            bound_origin: HeapAllocationOrigin::Native,
            conflicting_origin: Some(HeapAllocationOrigin::Native),
            ..
        } if *actual == heap_id
            && heap_name == "TestHeapName(conflicting_deser_binding)",
    ));

    drop(first);
    scope
        .register_heap(
            heap_id,
            second
                .heap_arc()
                .downgrade()
                .expect("heap should have an allocation"),
        )
        .expect("an expired binding should be replaceable");
    assert_eq!(scope.get_heap(&heap_id).as_ref(), Some(second.heap_arc()));
}

#[test]
fn test_concurrent_scopes_reject_conflicting_live_heap_binding() {
    let owners = [1, 2].map(|count| {
        let heap = ErasingHeap::new();
        heap.alloc_simple(SimpleData { flag: true, count });
        heap.into_ref_named(TestHeapName::heap_name("cross_scope_binding"))
    });
    let heap_id = owners[0].heap_arc().heap_ref_id().unwrap();
    let bindings = Arc::default();
    let scopes = [
        StarlarkDeserScope::new(Arc::clone(&bindings), Default::default()),
        StarlarkDeserScope::new(bindings, Default::default()),
    ];
    let ready = std::sync::Barrier::new(2);
    let results = std::thread::scope(|threads| {
        let handles: Vec<_> = scopes
            .iter()
            .zip(&owners)
            .map(|(scope, owner)| {
                let ready = &ready;
                threads.spawn(move || {
                    ready.wait();
                    scope.register_heap(heap_id, owner.heap_arc().downgrade().unwrap())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(matches!(
        results.iter().find_map(|result| result.as_ref().err()),
        Some(PagableError::ConflictingHeapBinding { .. })
    ));
    let bound = scopes[0].get_heap(&heap_id).unwrap();
    assert_eq!(scopes[1].get_heap(&heap_id).as_ref(), Some(&bound));
    for scope in &scopes {
        scope
            .register_heap(heap_id, bound.downgrade().unwrap())
            .unwrap();
    }
}

#[test]
fn test_frozen_value_typed_round_trip() -> crate::Result<()> {
    use crate::values::FrozenValueTyped;

    // Create a heap with a SimpleData, then wrap it as FrozenValueTyped.
    let heap = ErasingHeap::new();
    let fv = heap.alloc_simple(SimpleData {
        flag: true,
        count: 77,
    });
    let typed = FrozenValueTyped::<SimpleData>::new(fv).unwrap();

    let root = heap.alloc_ref_data(3, typed.to_value());
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_fvt"));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    let restored_typed = FrozenValueTyped::<SimpleData>::new(ref_data.target).unwrap();
    assert_eq!(restored_typed.flag, true);
    assert_eq!(restored_typed.count, 77);

    Ok(())
}

/// A test type with `SmallMap<String, Value>` — has Drop (SmallMap), goes in drop bump.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("SmallMapData")]
struct SmallMapData<'v> {
    entries: starlark_map::small_map::SmallMap<String, Value<'v>>,
}

#[starlark_value(type = "SmallMapData", frozen_vtable)]
impl<'v> StarlarkValue<'v> for SmallMapData<'v> {}

/// A test type with `SmallMap<Value, Value>`.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("SmallMapFvData")]
struct SmallMapFvData<'v> {
    entries: starlark_map::small_map::SmallMap<Value<'v>, Value<'v>>,
}

#[starlark_value(type = "SmallMapFvData", frozen_vtable)]
impl<'v> StarlarkValue<'v> for SmallMapFvData<'v> {}

#[test]
fn test_small_map_string_key_round_trip() -> crate::Result<()> {
    use starlark_map::small_map::SmallMap;

    // SmallMap<String, Value> with values pointing to SimpleData (undrop bump).
    // SmallMapData is in drop bump.
    let heap = ErasingHeap::new();
    let v1 = heap.alloc_simple(SimpleData {
        flag: true,
        count: 10,
    });
    let v2 = heap.alloc_simple(SimpleData {
        flag: false,
        count: 20,
    });

    let root = heap.with(|h| {
        let mut entries = SmallMap::new();
        entries.insert("beta".to_owned(), ErasingHeap::restore_one(h, v2));
        entries.insert("alpha".to_owned(), ErasingHeap::restore_one(h, v1));
        erase(h.alloc_simple(SmallMapData { entries }))
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_sm_string"));

    let restored = round_trip_owned(heap_ref, root)?;
    let map_data: &SmallMapData = restored
        .as_ref()
        .value()
        .downcast_ref::<SmallMapData>()
        .unwrap();

    let keys: Vec<&str> = map_data.entries.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["beta", "alpha"]);

    let v1_restored: &SimpleData = map_data
        .entries
        .get("alpha")
        .unwrap()
        .downcast_ref::<SimpleData>()
        .unwrap();
    assert_eq!(v1_restored.count, 10);

    let v2_restored: &SimpleData = map_data
        .entries
        .get("beta")
        .unwrap()
        .downcast_ref::<SimpleData>()
        .unwrap();
    assert_eq!(v2_restored.count, 20);

    Ok(())
}

#[test]
fn test_small_map_frozen_value_key_backward_ref() -> crate::Result<()> {
    use starlark_map::small_map::SmallMap;

    // The map's HeapData values precede it in the drop-bump wire order.
    // Its string keys are in the later non-drop bump. Lazy restoration must
    // resolve both directions independently of allocation order.
    let heap = ErasingHeap::new();

    // Keys: frozen strings in undrop bump (hashable).
    let k1 = heap.alloc_str("key_one");
    let k2 = heap.alloc_str("key_two");

    // Values: HeapData in drop bump.
    let v1 = heap.alloc_simple(HeapData {
        items: vec![100],
        label: "val_one".to_owned(),
        boxed: Box::new(1),
    });
    let v2 = heap.alloc_simple(HeapData {
        items: vec![200],
        label: "val_two".to_owned(),
        boxed: Box::new(2),
    });

    let root = heap.with(|h| -> crate::Result<_> {
        let mut entries = SmallMap::new();
        entries.insert_hashed(
            ErasingHeap::restore_one(h, k1).get_hashed()?,
            ErasingHeap::restore_one(h, v1),
        );
        entries.insert_hashed(
            ErasingHeap::restore_one(h, k2).get_hashed()?,
            ErasingHeap::restore_one(h, v2),
        );
        Ok(erase(h.alloc_simple(SmallMapFvData { entries })))
    })?;
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_sm_fv_key"));

    let restored = round_trip_owned(heap_ref, root)?;
    let map_data: &SmallMapFvData = restored
        .as_ref()
        .value()
        .downcast_ref::<SmallMapFvData>()
        .unwrap();
    assert_eq!(map_data.entries.len(), 2);

    let key_strs: Vec<&str> = map_data
        .entries
        .iter()
        .map(|(k, _)| k.unpack_str().unwrap())
        .collect();
    assert_eq!(key_strs, vec!["key_one", "key_two"]);

    let val_labels: Vec<&str> = map_data
        .entries
        .iter()
        .map(|(_, v)| v.downcast_ref::<HeapData>().unwrap().label.as_str())
        .collect();
    assert_eq!(val_labels, vec!["val_one", "val_two"]);

    for (k, _) in map_data.entries.iter() {
        let hashed = k.get_hashed()?;
        assert!(
            map_data.entries.get_hashed(hashed.as_ref()).is_some(),
            "lookup by key {:?} should succeed",
            k.unpack_str()
        );
    }

    Ok(())
}

#[test]
fn test_small_map_frozen_value_key_forward_ref() -> crate::Result<()> {
    use starlark_map::small_map::SmallMap;

    // The drop-bump map precedes its non-drop string keys in wire order.
    // Resolving those forward references must initialize the strings before hashing.
    let heap = ErasingHeap::new();

    // Allocate strings in undrop bump.
    let k1 = heap.alloc_str("hello");
    let k2 = heap.alloc_str("world");

    // Values: inline ints (no heap allocation needed).
    let v1 = Value::testing_new_int(111);
    let v2 = Value::testing_new_int(222);

    let root = heap.with(|h| -> crate::Result<_> {
        let mut entries = SmallMap::new();
        entries.insert_hashed(
            ErasingHeap::restore_one(h, k1).get_hashed()?,
            ErasingHeap::restore_one(h, v1),
        );
        entries.insert_hashed(
            ErasingHeap::restore_one(h, k2).get_hashed()?,
            ErasingHeap::restore_one(h, v2),
        );
        Ok(erase(h.alloc_simple(SmallMapFvData { entries })))
    })?;

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_forward_ref"));

    let restored = round_trip_owned(heap_ref, root)?;
    let map_data: &SmallMapFvData = restored
        .as_ref()
        .value()
        .downcast_ref::<SmallMapFvData>()
        .unwrap();
    assert_eq!(map_data.entries.len(), 2);

    let key_strs: Vec<&str> = map_data
        .entries
        .iter()
        .map(|(k, _)| k.unpack_str().unwrap())
        .collect();
    assert_eq!(key_strs, vec!["hello", "world"]);

    let values: Vec<i32> = map_data
        .entries
        .iter()
        .map(|(_, v)| v.unpack_i32().unwrap())
        .collect();
    assert_eq!(values, vec![111, 222]);

    for (k, _) in map_data.entries.iter() {
        let hashed = k.get_hashed()?;
        assert!(
            map_data.entries.get_hashed(hashed.as_ref()).is_some(),
            "lookup by key {:?} should succeed",
            k.unpack_str()
        );
    }

    Ok(())
}

#[test]
fn test_range_round_trip() -> crate::Result<()> {
    use std::num::NonZeroI32;

    use crate::values::types::range::Range;

    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(Range::new(1, 10, NonZeroI32::new(2).unwrap()));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_range"));

    let restored = round_trip_owned(heap_ref, root)?;
    let range: &Range = restored.as_ref().value().downcast_ref::<Range>().unwrap();
    assert_eq!(format!("{}", range), "range(1, 10, 2)");

    Ok(())
}

#[test]
fn test_owned_frozen_round_trip() -> crate::Result<()> {
    let owned: OwnedFrozen<Value> =
        OwnedFrozen::build(TestHeapName::heap_name("test_owned"), |heap| {
            heap.alloc_simple(SimpleData {
                flag: true,
                count: 42,
            })
        });

    // Round-trip via pagable ser/de.
    let mut ser = pagable::testing::TestingSerializer::new();
    owned
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let bytes = ser.finish();

    let mut de = pagable::testing::TestingDeserializer::new(&bytes);
    let restored =
        <OwnedFrozen<Value>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;

    // Verify the restored value.
    restored.by_ref(|v| {
        let simple: &SimpleData = v.downcast_ref().expect("should be SimpleData");
        assert_eq!(simple.flag, true);
        assert_eq!(simple.count, 42);
    });

    Ok(())
}

#[test]
fn test_owned_frozen_typed_round_trip() -> crate::Result<()> {
    let owned: OwnedFrozen<ValueTyped<SimpleData>> =
        OwnedFrozen::build(TestHeapName::heap_name("test_owned_typed"), |heap| {
            heap.alloc_simple_typed(SimpleData {
                flag: false,
                count: 99,
            })
        });

    // Round-trip via pagable ser/de.
    let mut ser = pagable::testing::TestingSerializer::new();
    owned
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let bytes = ser.finish();

    let mut de = pagable::testing::TestingDeserializer::new(&bytes);
    let restored = <OwnedFrozen<ValueTyped<SimpleData>>>::pagable_deserialize(&mut de)
        .map_err(crate::Error::new_other)?;

    restored.by_ref(|v| {
        assert_eq!(v.as_ref().flag, false);
        assert_eq!(v.as_ref().count, 99);
    });

    Ok(())
}

/// Test type mirroring StackFrame: String + Option<FileSpan>.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("TestStackFrame({}, {:?})", self.name, self.location)]
struct TestStackFrame {
    name: String,
    #[starlark_pagable(pagable)]
    location: Option<FileSpan>,
}

starlark_simple_value!(TestStackFrame);

#[starlark_value(type = "TestStackFrame")]
impl<'v> StarlarkValue<'v> for TestStackFrame {}

#[test]
fn test_stack_frame_data_round_trip() -> crate::Result<()> {
    use crate::values::types::tuple::value::Tuple;

    let heap = ErasingHeap::new();

    let f0 = heap.alloc_simple(TestStackFrame {
        name: "native_func".to_owned(),
        location: None,
    });

    let codemap = CodeMap::new(
        "test.bzl".to_owned(),
        "load('foo')\ndef bar():\n  pass\n".to_owned(),
    );
    let span = codemap.full_span();
    let f1 = heap.alloc_simple(TestStackFrame {
        name: "bar".to_owned(),
        location: Some(FileSpan {
            file: codemap,
            span,
        }),
    });

    let root = heap.alloc_tuple(&[f0, f1]);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_stack_frame_data"));

    let restored = round_trip_owned(heap_ref, root)?;
    let tuple: &Tuple = restored.as_ref().value().downcast_ref::<Tuple>().unwrap();
    let content = tuple.content();
    assert_eq!(content.len(), 2);

    let data0: &TestStackFrame = content[0].downcast_ref::<TestStackFrame>().unwrap();
    assert_eq!(data0.name, "native_func");
    assert!(data0.location.is_none());

    let data1: &TestStackFrame = content[1].downcast_ref::<TestStackFrame>().unwrap();
    assert_eq!(data1.name, "bar");
    let loc = data1.location.as_ref().expect("should have location");
    assert_eq!(loc.file.filename(), "test.bzl");
    assert_eq!(loc.file.source(), "load('foo')\ndef bar():\n  pass\n");

    Ok(())
}

#[test]
fn test_native_codemap_round_trip() -> crate::Result<()> {
    static NATIVE: NativeCodeMap = NativeCodeMap::new("test_native.rs", 42, 10);
    pagable::static_value!(
        NATIVE_STATIC: NativeCodeMap = &NATIVE,
        starlark_syntax::codemap::NativeCodeMapStaticEntry
    );

    let codemap = NativeCodeMap::to_codemap(NATIVE_STATIC);
    let file_span = FileSpan {
        file: codemap,
        span: NativeCodeMap::FULL_SPAN,
    };

    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(TestStackFrame {
        name: "native_call".to_owned(),
        location: Some(file_span),
    });

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_native_codemap"));
    let restored = round_trip_owned(heap_ref, root)?;
    let data: &TestStackFrame = restored
        .as_ref()
        .value()
        .downcast_ref::<TestStackFrame>()
        .unwrap();
    assert_eq!(data.name, "native_call");
    let loc = data.location.as_ref().expect("should have location");
    assert_eq!(loc.file.filename(), "test_native.rs");
    assert_eq!(loc.file.source(), "<native>");
    assert_eq!(loc.file.id(), NativeCodeMap::to_codemap(NATIVE_STATIC).id());

    Ok(())
}

#[test]
fn test_frozen_dict_round_trip() -> crate::Result<()> {
    use crate::values::dict::AllocDict;
    use crate::values::dict::Dict;
    use crate::values::types::dict::value::DictGen;

    let heap = ErasingHeap::new();
    let root = heap.alloc(AllocDict([("hello", 1), ("world", 2)]));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_dict"));

    let restored = round_trip_owned(heap_ref, root)?;
    let restored = restored.as_ref();
    let dict = restored.value().downcast_ref::<DictGen<Dict>>().unwrap();
    assert_eq!(dict.0.content.len(), 2);
    let keys: Vec<&str> = dict
        .0
        .content
        .keys()
        .map(|k| k.unpack_str().unwrap())
        .collect();
    assert!(keys.contains(&"hello"));
    assert!(keys.contains(&"world"));
    // Looking a key up uses the hash that deserialization stored alongside it; iterating
    // the entries would pass even if that hash were wrong.
    assert_eq!(dict.0.get_str("hello").unwrap().unpack_i32(), Some(1));
    assert_eq!(dict.0.get_str("world").unwrap().unpack_i32(), Some(2));

    Ok(())
}

#[test]
fn test_frozen_struct_round_trip() -> crate::Result<()> {
    use crate::values::structs::AllocStruct;

    let heap = ErasingHeap::new();
    let root = heap.alloc(AllocStruct([("name", "alice"), ("age", "30")]));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_struct"));

    let restored = round_trip_owned(heap_ref, root)?;
    let attrs = restored.as_ref().value().dir_attr();
    assert_eq!(attrs.len(), 2);
    assert!(attrs.contains(&"name".to_owned()));
    assert!(attrs.contains(&"age".to_owned()));

    Ok(())
}

#[test]
fn test_frozen_set_round_trip() -> crate::Result<()> {
    use starlark_map::small_set::SmallSet;

    use crate::values::types::set::value::SetData;
    use crate::values::types::set::value::SetGen;

    let heap = ErasingHeap::new();

    let root = heap.with(|heap| {
        let mut content: SmallSet<Value> = SmallSet::new();
        content.insert_hashed(Value::testing_new_int(1).get_hashed()?);
        content.insert_hashed(Value::testing_new_int(2).get_hashed()?);
        content.insert_hashed(Value::testing_new_int(3).get_hashed()?);
        crate::Result::Ok(erase(heap.alloc_simple(SetGen(SetData { content }))))
    })?;

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_set"));
    let restored = round_trip_owned(heap_ref, root)?;
    let restored = restored.as_ref();
    let set = restored.value().downcast_ref::<SetGen<SetData>>().unwrap();
    assert_eq!(set.0.content.len(), 3);
    let values: Vec<i32> = set
        .0
        .content
        .iter()
        .map(|v| v.unpack_i32().unwrap())
        .collect();
    assert!(values.contains(&1));
    assert!(values.contains(&2));
    assert!(values.contains(&3));
    // As for the dict: only a lookup checks the hash deserialization stored.
    let probe = Value::testing_new_int(2).get_hashed()?;
    assert!(set.0.content.contains_hashed(probe.as_ref()));

    Ok(())
}

#[test]
fn test_frozen_record_type_round_trip() -> crate::Result<()> {
    use std::sync::Arc;

    use starlark_map::small_map::SmallMap;

    use crate::eval::ParametersSpec;
    use crate::eval::ParametersSpecParam;
    use crate::typing::Ty;
    use crate::values::record::field::Field;
    use crate::values::record::record_type::FrozenRecordType;
    use crate::values::record::record_type::RecordVariantFrozen;
    use crate::values::record::ty_record_type::TyRecordData;
    use crate::values::types::type_instance_id::TypeInstanceId;
    use crate::values::typing::type_compiled::compiled::TypeCompiled;

    let heap = ErasingHeap::new();

    // Build a single Arc<TyRecordData> shared by two FrozenRecordType
    // allocations — this exercises Arc identity preservation through pagable's
    // dedup mechanism (TyRecordData routes through `#[starlark_pagable(pagable)]`).
    let shared = Arc::new(TyRecordData {
        name: "MyRec".to_owned(),
        ty_record: Ty::any(),
        ty_record_type: Ty::any(),
        parameter_spec_prototype: ParametersSpec::<()>::new_named_only(
            "MyRec",
            [
                ("x", ParametersSpecParam::Required),
                ("y", ParametersSpecParam::Required),
            ],
        )
        .prototype(),
    });

    let id_a = TypeInstanceId::for_test(&"frozen_record_type_a");
    let id_b = TypeInstanceId::for_test(&"frozen_record_type_b");
    let (rt_a_fv, rt_b_fv) = heap.with(|heap| {
        let make_fields = || {
            let mut fields: SmallMap<String, Field> = SmallMap::new();
            fields.insert(
                "x".to_owned(),
                Field {
                    typ: TypeCompiled::any(),
                    default: None,
                },
            );
            fields.insert(
                "y".to_owned(),
                Field {
                    typ: TypeCompiled::any(),
                    default: None,
                },
            );
            fields
        };

        let rt_a_fv = erase(heap.alloc_simple(FrozenRecordType {
            id: id_a,
            ty_record_data: RecordVariantFrozen {
                ty: Some(shared.clone()),
            },
            fields: make_fields(),
        }));
        let rt_b_fv = erase(heap.alloc_simple(FrozenRecordType {
            id: id_b,
            ty_record_data: RecordVariantFrozen { ty: Some(shared) },
            fields: make_fields(),
        }));
        (rt_a_fv, rt_b_fv)
    });
    let root = heap.alloc_tuple(&[rt_a_fv, rt_b_fv]);

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_record_type"));
    // SAFETY: heap_ref owns the arena hosting root.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, root) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;

    let tuple: &crate::values::types::tuple::value::Tuple = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::tuple::value::Tuple>()
        .unwrap();
    let content = tuple.content();
    let rt_a: &FrozenRecordType = content[0].downcast_ref::<FrozenRecordType>().unwrap();
    let rt_b: &FrozenRecordType = content[1].downcast_ref::<FrozenRecordType>().unwrap();

    assert_eq!(rt_a.id, id_a);
    assert_eq!(rt_b.id, id_b);
    for rt in [rt_a, rt_b] {
        let field_names: Vec<&str> = rt.fields.keys().map(String::as_str).collect();
        assert_eq!(field_names, vec!["x", "y"]);
        for (_, field) in rt.fields.iter() {
            assert!(field.default.is_none());
        }
    }

    let data_a = rt_a
        .ty_record_data
        .ty
        .as_ref()
        .expect("ty_record_data restored");
    let data_b = rt_b
        .ty_record_data
        .ty
        .as_ref()
        .expect("ty_record_data restored");
    assert_eq!(data_a.name, "MyRec");
    assert_eq!(data_a.ty_record, Ty::any());
    assert_eq!(data_a.ty_record_type, Ty::any());
    assert_eq!(data_a.parameter_spec_prototype.len(), 2);

    assert!(
        Arc::ptr_eq(data_a, data_b),
        "pagable Arc dedup should round-trip the shared Arc<TyRecordData> as a single allocation",
    );

    Ok(())
}

#[test]
fn test_frozen_record_round_trip() -> crate::Result<()> {
    use std::sync::Arc;

    use starlark_map::small_map::SmallMap;

    use crate::eval::ParametersSpec;
    use crate::eval::ParametersSpecParam;
    use crate::typing::Ty;
    use crate::values::layout::avalue::TrailingValues;
    use crate::values::record::Record;
    use crate::values::record::field::Field;
    use crate::values::record::record_type::FrozenRecordType;
    use crate::values::record::record_type::RecordVariantFrozen;
    use crate::values::record::ty_record_type::TyRecordData;
    use crate::values::types::type_instance_id::TypeInstanceId;
    use crate::values::typing::type_compiled::compiled::TypeCompiled;

    let heap = ErasingHeap::new();

    let id = TypeInstanceId::for_test(&"frozen_record_instance_type");
    let record_type_fv = heap.with(|heap| {
        let mut fields: SmallMap<String, Field> = SmallMap::new();
        for name in ["x", "y"] {
            fields.insert(
                name.to_owned(),
                Field {
                    typ: TypeCompiled::any(),
                    default: None,
                },
            );
        }
        erase(
            heap.alloc_simple(FrozenRecordType {
                id,
                ty_record_data: RecordVariantFrozen {
                    ty: Some(Arc::new(TyRecordData {
                        name: "Point".to_owned(),
                        ty_record: Ty::any(),
                        ty_record_type: Ty::any(),
                        parameter_spec_prototype: ParametersSpec::<()>::new_named_only(
                            "Point",
                            [
                                ("x", ParametersSpecParam::Required),
                                ("y", ParametersSpecParam::Required),
                            ],
                        )
                        .prototype(),
                    })),
                },
                fields,
            }),
        )
    });
    let x = heap.alloc_simple(SimpleData {
        flag: true,
        count: 7,
    });
    let y = heap.alloc_str("seven");
    // Two instances of one type, so the type must round-trip as a single value.
    let (first, second) = heap.with(|heap| {
        let typ = ErasingHeap::restore_one(heap, record_type_fv);
        let values = ErasingHeap::restore(heap, &[x, y]);
        (
            erase(heap.alloc_record(typ, &values)),
            erase(heap.alloc_record(typ, &values[..1].repeat(2))),
        )
    });
    let root = heap.alloc_tuple(&[first, second]);

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_record"));
    // SAFETY: heap_ref owns the arena hosting root.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, root) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;

    let tuple: &crate::values::types::tuple::value::Tuple = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::tuple::value::Tuple>()
        .unwrap();
    let content = tuple.content();
    let first = Record::from_value(content[0]).expect("first element is a record");
    let second = Record::from_value(content[1]).expect("second element is a record");

    assert_eq!(first.record_type_id(), id);
    assert!(
        first.typ.ptr_eq(second.typ),
        "both instances should share the single restored record type"
    );
    let typ: &FrozenRecordType = first.typ.downcast_ref::<FrozenRecordType>().unwrap();
    assert_eq!(
        typ.fields.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["x", "y"]
    );

    assert_eq!(first.content().len(), 2);
    let first_x: &SimpleData = first.content()[0].downcast_ref::<SimpleData>().unwrap();
    assert_eq!(first_x.count, 7);
    assert_eq!(first.content()[1].unpack_str(), Some("seven"));

    let fields: Vec<(&str, Value)> = second.iter().collect();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].0, "x");
    assert_eq!(fields[1].0, "y");
    assert!(
        fields[0].1.ptr_eq(fields[1].1),
        "a value stored twice in one record should round-trip as one allocation"
    );

    Ok(())
}

#[test]
fn test_static_frozen_value_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();

    let none_fv = Value::new_none();
    let true_fv = Value::new_bool(true);
    let false_fv = Value::new_bool(false);
    let empty_tuple_fv = VALUE_EMPTY_TUPLE.unpack().to_value();
    let static_str_fv = const_frozen_string!("static_test_str").to_value();

    let r0 = heap.alloc_ref_data(1, none_fv);
    let r1 = heap.alloc_ref_data(2, true_fv);
    let r2 = heap.alloc_ref_data(3, false_fv);
    let r3 = heap.alloc_ref_data(4, empty_tuple_fv);
    let r4 = heap.alloc_ref_data(5, static_str_fv);
    let root = heap.alloc_tuple(&[r0, r1, r2, r3, r4]);

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_static"));
    let restored = round_trip_owned(heap_ref, root)?;
    let tuple: &crate::values::types::tuple::value::Tuple = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::tuple::value::Tuple>()
        .unwrap();
    let content = tuple.content();

    let ref0: &RefData = content[0].downcast_ref::<RefData>().unwrap();
    assert_eq!(ref0.label, 1);
    assert!(ref0.target.is_none());
    assert_eq!(
        ref0.target.ptr_value().ptr_value_untagged(),
        none_fv.ptr_value().ptr_value_untagged(),
    );

    let ref1: &RefData = content[1].downcast_ref::<RefData>().unwrap();
    assert_eq!(ref1.label, 2);
    assert_eq!(ref1.target.unpack_bool(), Some(true));
    assert_eq!(
        ref1.target.ptr_value().ptr_value_untagged(),
        true_fv.ptr_value().ptr_value_untagged(),
    );

    let ref2: &RefData = content[2].downcast_ref::<RefData>().unwrap();
    assert_eq!(ref2.label, 3);
    assert_eq!(ref2.target.unpack_bool(), Some(false));
    assert_eq!(
        ref2.target.ptr_value().ptr_value_untagged(),
        false_fv.ptr_value().ptr_value_untagged(),
    );

    let ref3: &RefData = content[3].downcast_ref::<RefData>().unwrap();
    assert_eq!(ref3.label, 4);
    assert_eq!(
        ref3.target.ptr_value().ptr_value_untagged(),
        empty_tuple_fv.ptr_value().ptr_value_untagged(),
    );

    let ref4: &RefData = content[4].downcast_ref::<RefData>().unwrap();
    assert_eq!(ref4.label, 5);
    assert_eq!(ref4.target.unpack_str(), Some("static_test_str"));
    assert_eq!(
        ref4.target.ptr_value().ptr_value_untagged(),
        static_str_fv.ptr_value().ptr_value_untagged(),
    );

    Ok(())
}

starlark::globals_static!(
    PAGABLE_TEST_STATIC_GLOBALS = |globals| {
        globals.set(
            "global_value",
            SimpleData {
                flag: true,
                count: 17,
            },
        );
    }
);

starlark::methods_static!(
    PAGABLE_TEST_STATIC_METHODS = |methods| {
        methods.set_attribute(
            "method_value",
            SimpleData {
                flag: false,
                count: 23,
            },
            None,
        );
    }
);

fn assert_static_value_round_trip(
    static_fv: Value<'static>,
    label: usize,
) -> crate::Result<Value<'static>> {
    let heap = ErasingHeap::new();
    let root = heap.alloc_ref_data(label, static_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_static_heap_value"));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref().unwrap();
    assert_eq!(ref_data.label, label);
    assert_eq!(
        ref_data.target.ptr_value().ptr_value_untagged(),
        static_fv.ptr_value().ptr_value_untagged(),
    );
    // The static, as just asserted, so it outlives `restored`.
    Ok(erase(ref_data.target))
}

#[test]
fn test_globals_static_heap_value_round_trip() -> crate::Result<()> {
    let static_fv = erase(
        PAGABLE_TEST_STATIC_GLOBALS
            .globals()
            .get_ref("global_value")
            .expect("static global should exist")
            .value(),
    );

    let restored = assert_static_value_round_trip(static_fv, 8)?;
    let restored_data = restored
        .downcast_ref::<SimpleData>()
        .expect("static global should still point to SimpleData");
    assert_eq!(restored_data.flag, true);
    assert_eq!(restored_data.count, 17);
    Ok(())
}

#[test]
fn test_methods_static_heap_value_round_trip() -> crate::Result<()> {
    let static_fv = PAGABLE_TEST_STATIC_METHODS
        .methods()
        .members()
        .find_map(|(name, value)| (name == "method_value").then_some(value))
        .expect("static method attribute should exist");

    assert_static_value_round_trip(static_fv, 9)?;
    Ok(())
}

fn colliding_static_heap_a() -> OwnedFrozenRef<'static, ()> {
    starlark::globals_static!(
        COLLIDING_STATIC_GLOBALS = |globals| {
            globals.set(
                "a_value",
                SimpleData {
                    flag: true,
                    count: 41,
                },
            );
        }
    );
    COLLIDING_STATIC_GLOBALS.globals().heap()
}

fn colliding_static_heap_b() -> OwnedFrozenRef<'static, ()> {
    starlark::globals_static!(
        COLLIDING_STATIC_GLOBALS = |globals| {
            globals.set(
                "b_value",
                SimpleData {
                    flag: false,
                    count: 42,
                },
            );
        }
    );
    COLLIDING_STATIC_GLOBALS.globals().heap()
}

/// Distinct static heaps can share a name (`<module_path>::<NAME>`, here via
/// two `globals_static!` in different `fn` bodies of this module). References
/// are by `StaticHeapId`, not by name, so each resolves to its own live heap.
#[test]
fn test_same_named_static_heaps_round_trip_to_distinct_heaps() -> crate::Result<()> {
    let heap_a = colliding_static_heap_a();
    let heap_b = colliding_static_heap_b();
    assert_eq!(
        heap_a.name().map(|n| n.to_string()),
        heap_b.name().map(|n| n.to_string()),
        "test premise: the two static heaps share a name"
    );
    assert!(
        heap_a != heap_b,
        "test premise: the two static heaps are distinct allocations"
    );

    let restored_a = round_trip_heap_ref(heap_a)?;
    let restored_b = round_trip_heap_ref(heap_b)?;
    assert!(
        restored_a.owner() == heap_a,
        "ref to heap A should resolve to heap A's live allocation"
    );
    assert!(
        restored_b.owner() == heap_b,
        "ref to heap B should resolve to heap B's live allocation"
    );
    Ok(())
}

/// A serialized reference to a registered static heap resolves back to the
/// live heap, not a reconstructed copy under the same name.
#[test]
fn test_static_heap_ref_round_trip_is_identity() -> crate::Result<()> {
    let static_heap = PAGABLE_TEST_STATIC_GLOBALS.globals().heap();
    let restored = round_trip_heap_ref(static_heap)?;
    assert!(
        restored.owner() == static_heap,
        "static heap ref should resolve to the live heap"
    );
    Ok(())
}

/// Paging a graph that references a static heap must not poison the shared
/// arc for other serialization consumers.
///
/// Before static heaps were referenced by name, `page_out_item` stamped a
/// `DataKey` on the process-wide `PartialPagableArc`, and any later inline
/// (testing) serialization of a graph referencing the same static heap failed
/// with "cannot serialize the inner value of a PartialPagableArc backed by
/// DataKey(..)" — an order-dependent failure across tests sharing statics
/// like `REGISTER_LIST_STATICS`.
#[test]
fn test_static_heap_paging_does_not_poison_inline_serialization() -> crate::Result<()> {
    let static_heap = PAGABLE_TEST_STATIC_GLOBALS.globals().heap();

    // Page a graph referencing the static heap through the real storage impls.
    let heap = ErasingHeap::new();
    heap.add_reference(static_heap);
    let root = heap.alloc_simple(SimpleData {
        flag: true,
        count: 31,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("paged_static_heap_user"));
    // SAFETY: `heap_ref` owns the arena hosting `root`.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, root) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;
    assert!(
        restored.owner().refs().any(|r| r == static_heap),
        "restored heap should reference the live static heap"
    );

    // Inline serialization of another graph referencing the same static heap
    // must still succeed afterwards.
    let heap = ErasingHeap::new();
    heap.add_reference(static_heap);
    let root = heap.alloc_simple(SimpleData {
        flag: false,
        count: 32,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("inline_static_heap_user"));
    let restored = round_trip_owned(heap_ref, root)?;
    let data: &SimpleData = restored.as_ref().value().downcast_ref().unwrap();
    assert_eq!(data.count, 32);
    Ok(())
}

/// Phantom type used only as the `T` of `StarlarkValueAsType<T>` in the
/// round-trip test below. Never allocated on a heap.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("AsTypeRoundTripTestType")]
struct AsTypeRoundTripTestType;

#[starlark_value(type = "AsTypeRoundTripTestType")]
impl<'v> StarlarkValue<'v> for AsTypeRoundTripTestType {}

crate::declare_starlark_value_as_type!(AS_TYPE_RT_STATIC, AsTypeRoundTripTestType);

#[test]
fn test_starlark_value_as_type_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let static_fv = AS_TYPE_RT_STATIC.to_value();
    let root = heap.alloc_ref_data(7, static_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name(
        "test_starlark_value_as_type_round_trip",
    ));

    let restored = round_trip_owned(heap_ref, root)?;
    let ref_data: &RefData = restored.as_ref().value().downcast_ref::<RefData>().unwrap();
    assert_eq!(ref_data.label, 7);
    assert_eq!(
        ref_data.target.ptr_value().ptr_value_untagged(),
        static_fv.ptr_value().ptr_value_untagged(),
    );
    Ok(())
}

// ============================================================================
// Arc<T> blanket: starlark-only T, plain `Arc<T>` field — round-trips through
// the `Arc<T>: StarlarkSerialize` blanket, no manual bridge.
// ============================================================================

/// Inner type, carried inside an `Arc`. Starlark-only (no `pagable::Pagable` impl), so the
/// `Arc` goes through the starlark bridge; it holds no value, as nothing shared by `Arc` can.
#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
struct ArcBlanketInner {
    label: u32,
}

/// Outer `StarlarkValue` with a plain `Arc<ArcBlanketInner>` field
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("ArcBlanketOuter({})", self.outer_label)]
struct ArcBlanketOuter {
    inner: Arc<ArcBlanketInner>,
    outer_label: u32,
    items: Vec<u32>,
}

starlark_simple_value!(ArcBlanketOuter);

#[starlark_value(type = "ArcBlanketOuter")]
impl<'v> StarlarkValue<'v> for ArcBlanketOuter {}

#[test]
fn test_arc_blanket_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let shared = Arc::new(ArcBlanketInner { label: 7 });

    let a = heap.alloc_simple(ArcBlanketOuter {
        inner: shared.clone(),
        outer_label: 1,
        items: vec![1, 2, 3],
    });
    let b = heap.alloc_simple(ArcBlanketOuter {
        inner: shared,
        outer_label: 2,
        items: vec![4, 5],
    });
    let root = heap.alloc_tuple(&[a, b]);

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_arc_blanket"));
    // SAFETY: heap_ref owns the arena hosting root.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, root) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;

    let tuple: &crate::values::types::tuple::value::Tuple = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::tuple::value::Tuple>()
        .unwrap();
    let content = tuple.content();
    let outer_a: &ArcBlanketOuter = content[0].downcast_ref::<ArcBlanketOuter>().unwrap();
    let outer_b: &ArcBlanketOuter = content[1].downcast_ref::<ArcBlanketOuter>().unwrap();
    assert_eq!(outer_a.outer_label, 1);
    assert_eq!(outer_b.outer_label, 2);
    assert_eq!(outer_a.items, vec![1, 2, 3]);
    assert_eq!(outer_b.items, vec![4, 5]);

    assert_eq!(outer_a.inner.label, 7);
    assert_eq!(outer_b.inner.label, 7);

    assert!(
        Arc::ptr_eq(&outer_a.inner, &outer_b.inner),
        "Arc<T> blanket should preserve Arc identity across round-trip",
    );

    Ok(())
}

// ============================================================================
// StarlarkAny<T> round-trip
// ============================================================================

/// Pure-data payload for `StarlarkAny` tests. `StarlarkPagable` derive so the
/// inner data round-trips; `register_starlark_any!` registers the typing
/// vtable entry needed for `StarlarkAny<AnyPayload>` to participate in ser/de.
#[derive(Debug, Clone, PartialEq, Eq, StarlarkPagable)]
struct AnyPayload {
    name: String,
    count: u32,
}

crate::register_starlark_any!(AnyPayload);
crate::register_any_array!(AnyPayload);

#[test]
fn test_starlark_any_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let payload = AnyPayload {
        name: "hello".to_owned(),
        count: 7,
    };
    let root = heap.alloc_any_value(payload.clone());
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_starlark_any"));

    let restored = round_trip_owned(heap_ref, root)?;
    let any_payload: &crate::values::any::StarlarkAny<AnyPayload> = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::any::StarlarkAny<AnyPayload>>()
        .unwrap();
    assert_eq!(any_payload.0, payload);

    Ok(())
}

#[test]
fn test_starlark_any_multiple_values_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let a = AnyPayload {
        name: "first".to_owned(),
        count: 1,
    };
    let b = AnyPayload {
        name: "second".to_owned(),
        count: 2,
    };
    let a_fv = heap.alloc_any_value(a.clone());
    let b_fv = heap.alloc_any_value(b.clone());
    let root = heap.alloc_tuple(&[a_fv, b_fv]);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_starlark_any_multi"));

    let restored = round_trip_owned(heap_ref, root)?;
    let tuple: &crate::values::types::tuple::value::Tuple = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::tuple::value::Tuple>()
        .unwrap();
    let content = tuple.content();
    let got_a: &crate::values::any::StarlarkAny<AnyPayload> = content[0]
        .downcast_ref::<crate::values::any::StarlarkAny<AnyPayload>>()
        .unwrap();
    let got_b: &crate::values::any::StarlarkAny<AnyPayload> = content[1]
        .downcast_ref::<crate::values::any::StarlarkAny<AnyPayload>>()
        .unwrap();
    assert_eq!(got_a.0, a);
    assert_eq!(got_b.0, b);

    Ok(())
}

// ============================================================================
// AnyArray<T> round-trip
//
// `AnyArray<T>` uses `alloc_raw_extra` for a trailing elements array — the
// elements live beyond the struct bounds. Its `AValue` impl
// (`AValueAnyArray<T>`) provides `starlark_serialize`/`starlark_deserialize`
// that walk `as_slice()` on the way out and reconstruct elements in-place on
// the way in (same pattern as `AValueList`).
// ============================================================================

#[test]
fn test_frozen_any_array_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let items = vec![
        AnyPayload {
            name: "alpha".to_owned(),
            count: 10,
        },
        AnyPayload {
            name: "beta".to_owned(),
            count: 20,
        },
        AnyPayload {
            name: "gamma".to_owned(),
            count: 30,
        },
    ];
    let root = heap.alloc_any_array_value(&items);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_any_array"));

    let restored = round_trip_owned(heap_ref, root)?;
    let any_array: &crate::values::types::any_array::AnyArray<AnyPayload> = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::any_array::AnyArray<AnyPayload>>()
        .unwrap();
    assert_eq!(any_array.as_slice().len(), 3);
    assert_eq!(any_array.as_slice()[0], items[0]);
    assert_eq!(any_array.as_slice()[1], items[1]);
    assert_eq!(any_array.as_slice()[2], items[2]);

    Ok(())
}

#[test]
fn test_frozen_any_array_empty_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.alloc_any_array_value::<AnyPayload>(&[]);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_frozen_any_array_empty"));

    let restored = round_trip_owned(heap_ref, root)?;
    let any_array: &crate::values::types::any_array::AnyArray<AnyPayload> = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::types::any_array::AnyArray<AnyPayload>>()
        .unwrap();
    assert_eq!(any_array.as_slice().len(), 0);

    Ok(())
}

/// Host type holding an `AtomicFrozenValueTypedOption` field.
#[derive(Display, Allocative, ProvidesStaticType, NoSerialize, StarlarkPagable)]
#[display("AtomicHost({})", self.label)]
struct AtomicHost<'v> {
    label: String,
    #[allocative(skip)]
    option: AtomicFrozenValueTypedOption<'v, StarlarkAnyComplex<FrozenComplexPayload>>,
}

impl std::fmt::Debug for AtomicHost<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtomicHost")
            .field("label", &self.label)
            .field("option_is_some", &self.option.load_relaxed().is_some())
            .finish()
    }
}

#[starlark_value(type = "AtomicHost", frozen_vtable)]
impl<'v> StarlarkValue<'v> for AtomicHost<'v> {}

#[test]
fn test_atomic_value_typed_option_some_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.with(|heap| {
        let payload = heap.alloc_simple_typed(StarlarkAnyComplex::new(FrozenComplexPayload {
            label: "target".to_owned(),
            numbers: vec![42],
        }));
        let payload = FrozenValueTyped::from_typed(payload).unwrap();
        erase(heap.alloc_simple(AtomicHost {
            label: "host_with_some".to_owned(),
            option: AtomicFrozenValueTypedOption::new(Some(payload)),
        }))
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name(
        "test_atomic_value_typed_option_some",
    ));

    let restored = round_trip_owned(heap_ref, root)?;
    let host: &AtomicHost = restored
        .as_ref()
        .value()
        .downcast_ref::<AtomicHost>()
        .unwrap();
    assert_eq!(host.label, "host_with_some");

    let loaded = host.option.load_relaxed();
    let payload = loaded.expect("option should be Some after round-trip");
    assert_eq!(payload.value.label, "target");
    assert_eq!(payload.value.numbers, vec![42]);

    Ok(())
}

#[test]
fn test_atomic_value_typed_option_none_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.with(|heap| {
        erase(heap.alloc_simple(AtomicHost {
            label: "host_with_none".to_owned(),
            option: AtomicFrozenValueTypedOption::new(None),
        }))
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name(
        "test_atomic_value_typed_option_none",
    ));

    let restored = round_trip_owned(heap_ref, root)?;
    let host: &AtomicHost = restored
        .as_ref()
        .value()
        .downcast_ref::<AtomicHost>()
        .unwrap();
    assert_eq!(host.label, "host_with_none");

    assert!(
        host.option.load_relaxed().is_none(),
        "option should be None after round-trip"
    );

    Ok(())
}

// ============================================================================
// StarlarkAnyComplex<T> round-trip
//
// `StarlarkAnyComplex<T>` is the GC-traceable sibling of `StarlarkAny<T>`.
// For pagable, we exercise the frozen-heap side: allocate
// `StarlarkAnyComplex<FrozenComplexPayload>` directly on a `FrozenHeap` via
// `alloc_simple` and round-trip.
// ============================================================================

#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
struct FrozenComplexPayload {
    label: String,
    numbers: Vec<u32>,
}

crate::register_starlark_any_complex!(frozen FrozenComplexPayload);

#[test]
fn test_starlark_any_complex_round_trip() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(crate::values::any_complex::StarlarkAnyComplex::new(
        FrozenComplexPayload {
            label: "complex".to_owned(),
            numbers: vec![1, 2, 3],
        },
    ));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_starlark_any_complex"));

    let restored = round_trip_owned(heap_ref, root)?;
    let got: &crate::values::any_complex::StarlarkAnyComplex<FrozenComplexPayload> = restored
        .as_ref()
        .value()
        .downcast_ref::<crate::values::any_complex::StarlarkAnyComplex<FrozenComplexPayload>>()
        .unwrap();
    assert_eq!(got.value.label, "complex");
    assert_eq!(got.value.numbers, vec![1, 2, 3]);

    Ok(())
}
#[test]
fn test_type_compiled_impl_as_starlark_value_round_trip() -> crate::Result<()> {
    use crate::typing::Ty;
    use crate::values::typing::type_compiled::compiled::DummyTypeMatcher;
    use crate::values::typing::type_compiled::compiled::TypeCompiledImplAsStarlarkValue;

    let heap = ErasingHeap::new();
    let original_ty = Ty::any();
    let root = heap.alloc_simple(
        TypeCompiledImplAsStarlarkValue::<DummyTypeMatcher>::new_for_test(
            DummyTypeMatcher,
            original_ty.clone(),
        ),
    );
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name(
        "test_type_compiled_impl_round_trip",
    ));

    let restored = round_trip_owned(heap_ref, root)?;
    let got: &TypeCompiledImplAsStarlarkValue<DummyTypeMatcher> = restored
        .as_ref()
        .value()
        .downcast_ref::<TypeCompiledImplAsStarlarkValue<DummyTypeMatcher>>()
        .unwrap();
    assert_eq!(got.ty_for_test(), &original_ty);
    let _: &DummyTypeMatcher = got.impl_for_test();

    Ok(())
}

#[test]
fn test_type_compiled_impl_with_is_str_round_trip() -> crate::Result<()> {
    use crate::typing::Ty;
    use crate::values::typing::type_compiled::compiled::TypeCompiledImplAsStarlarkValue;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::typing::type_compiled::matchers::IsStr;

    let heap = ErasingHeap::new();
    let original_ty = Ty::string();
    let root = heap.alloc_simple(TypeCompiledImplAsStarlarkValue::<IsStr>::new_for_test(
        IsStr,
        original_ty.clone(),
    ));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_type_compiled_is_str"));

    let restored = round_trip_owned(heap_ref, root)?;
    let got: &TypeCompiledImplAsStarlarkValue<IsStr> = restored
        .as_ref()
        .value()
        .downcast_ref::<TypeCompiledImplAsStarlarkValue<IsStr>>()
        .unwrap();
    assert_eq!(got.ty_for_test(), &original_ty);
    assert!(
        got.impl_for_test()
            .matches(const_frozen_string!("hi").at().to_value())
    );
    assert!(!got.impl_for_test().matches(Value::new_bool(true)));

    Ok(())
}

#[test]
fn test_type_compiled_impl_with_is_int_round_trip() -> crate::Result<()> {
    use crate::typing::Ty;
    use crate::values::typing::type_compiled::compiled::TypeCompiledImplAsStarlarkValue;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::typing::type_compiled::matchers::IsInt;

    let heap = ErasingHeap::new();
    let original_ty = Ty::int();
    let root = heap.alloc_simple(TypeCompiledImplAsStarlarkValue::<IsInt>::new_for_test(
        IsInt,
        original_ty.clone(),
    ));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_type_compiled_is_int"));

    let restored = round_trip_owned(heap_ref, root)?;
    let got: &TypeCompiledImplAsStarlarkValue<IsInt> = restored
        .as_ref()
        .value()
        .downcast_ref::<TypeCompiledImplAsStarlarkValue<IsInt>>()
        .unwrap();
    assert_eq!(got.ty_for_test(), &original_ty);
    assert!(got.impl_for_test().matches(Value::testing_new_int(42)));
    assert!(
        !got.impl_for_test()
            .matches(const_frozen_string!("x").at().to_value())
    );

    Ok(())
}

#[test]
fn test_type_compiled_impl_with_is_any_of_round_trip() -> crate::Result<()> {
    use crate::typing::Ty;
    use crate::values::typing::type_compiled::compiled::TypeCompiledImplAsStarlarkValue;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::typing::type_compiled::matcher::TypeMatcherBox;
    use crate::values::typing::type_compiled::matchers::IsAnyOf;
    use crate::values::typing::type_compiled::matchers::IsInt;
    use crate::values::typing::type_compiled::matchers::IsStr;

    let inners = vec![TypeMatcherBox::new(IsStr), TypeMatcherBox::new(IsInt)];
    let heap = ErasingHeap::new();
    let original_ty = Ty::union2(Ty::string(), Ty::int());
    let root = heap.alloc_simple(TypeCompiledImplAsStarlarkValue::<IsAnyOf>::new_for_test(
        IsAnyOf(inners),
        original_ty.clone(),
    ));
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_type_compiled_is_any_of"));

    let restored = round_trip_owned(heap_ref, root)?;
    let got: &TypeCompiledImplAsStarlarkValue<IsAnyOf> = restored
        .as_ref()
        .value()
        .downcast_ref::<TypeCompiledImplAsStarlarkValue<IsAnyOf>>()
        .unwrap();
    assert_eq!(got.ty_for_test(), &original_ty);
    assert_eq!(got.impl_for_test().0.len(), 2);
    assert!(
        got.impl_for_test()
            .matches(const_frozen_string!("s").at().to_value())
    );
    assert!(got.impl_for_test().matches(Value::testing_new_int(1)));
    assert!(!got.impl_for_test().matches(Value::new_bool(true)));

    Ok(())
}

// ============================================================================
// `starlark_deserialize_field` error context
// ============================================================================

/// The derive on an uninhabited enum: deserializing one is an error, so this must compile
/// even though no value can be built.
#[derive(Debug, StarlarkPagable)]
enum Uninhabited {}

#[test]
fn test_uninhabited_enum_derives_pagable() {
    fn assert_pagable<T: for<'fv> crate::pagable::StarlarkPagable<'fv>>() {}
    assert_pagable::<Uninhabited>();
}

#[derive(Debug, Allocative)]
struct AlwaysFailDeserialize;

impl crate::pagable::StarlarkSerialize for AlwaysFailDeserialize {
    fn starlark_serialize(
        &self,
        _ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        Ok(())
    }
}

impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for AlwaysFailDeserialize {
    fn starlark_deserialize(
        _ctx: &mut dyn crate::pagable::starlark_deserialize::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        Err(crate::Error::new_other(anyhow::anyhow!(
            "intentional test failure"
        )))
    }
}

#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("FieldErrorTestData")]
struct FieldErrorTestData {
    good_field: bool,
    bad_field: AlwaysFailDeserialize,
}

starlark_simple_value!(FieldErrorTestData);

#[starlark_value(type = "FieldErrorTestData")]
impl<'v> StarlarkValue<'v> for FieldErrorTestData {}

#[test]
fn test_deserialize_field_error_includes_field_name() {
    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(FieldErrorTestData {
        good_field: true,
        bad_field: AlwaysFailDeserialize,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_field_error"));

    let result = round_trip_owned(heap_ref, root);

    let err = result.unwrap_err();
    let err_msg = format!("{:#}", err);
    assert!(
        err_msg.contains("deserializing field bad_field"),
        "error should mention the field name, got: {}",
        err_msg,
    );
}

#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("DropFieldErrorTestData")]
struct DropFieldErrorTestData {
    /// Needs drop, so the value lands in the bump the arena walks on drop.
    owned: String,
    bad_field: AlwaysFailDeserialize,
}

starlark_simple_value!(DropFieldErrorTestData);

#[starlark_value(type = "DropFieldErrorTestData")]
impl<'v> StarlarkValue<'v> for DropFieldErrorTestData {}

/// A value that fails to deserialize leaves its slot without a payload; the
/// heap, dropped on the way out of the failed page-in, must still walk past it.
#[test]
fn test_heap_drops_cleanly_after_a_drop_value_fails_to_deserialize() {
    let heap = ErasingHeap::new();
    let root = heap.alloc_simple(DropFieldErrorTestData {
        owned: "owned".to_owned(),
        bad_field: AlwaysFailDeserialize,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_failed_drop_slot"));

    let err = round_trip_owned(heap_ref, root).expect_err("deserialization fails by construction");
    assert!(
        format!("{err:#}").contains("intentional test failure"),
        "the page-in reports the value's own error, got: {err:#}"
    );
}

/// Two roots into one heap, one of which fails to read back. Paged in
/// together, the failed slot stays in the drop bump while the other root keeps
/// the heap alive.
fn page_in_heap_with_a_failed_slot() -> crate::Result<(
    pagable::storage::in_memory::InMemoryPagableStorage,
    OwnedFrozen<Value<'static>>,
)> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    let heap = ErasingHeap::new();
    let good = heap.alloc_simple(SimpleData {
        flag: true,
        count: 5,
    });
    let bad = heap.alloc_simple(DropFieldErrorTestData {
        owned: "owned".to_owned(),
        bad_field: AlwaysFailDeserialize,
    });
    let owner = heap.into_ref_named(TestHeapName::heap_name("failed_slot_sibling"));
    // SAFETY: `owner` owns the arena hosting both values.
    let good_root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner.clone(), good) };
    let bad_root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner, bad) };

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let mut ser = SerializerForPaging::new(storage.storage_context());
    good_root
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    bad_root
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();
    let key = storage
        .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
        .map_err(crate::Error::new_other)?;
    storage.flush().map_err(crate::Error::new_other)?;
    drop((good_root, bad_root));
    storage.arc_cache().clear();

    let handle = PagableStorageHandle::new(storage.clone());
    let data = storage
        .fetch_data_blocking(&key)
        .map_err(crate::Error::new_other)?;
    let mut de = handle.root_deserializer(key, &data);
    let good =
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let failed = OwnedFrozen::<Value>::pagable_deserialize(&mut de);
    assert!(
        failed.is_err(),
        "the second root's value fails to deserialize by construction"
    );
    Ok((backing, good))
}

/// A memory profile of a heap with a failed slot reports the slot as
/// uninitialized instead of panicking, and page-out of a live value that
/// points into the heap, which walks it for the chunk index, succeeds.
#[test]
fn test_walks_over_a_failed_slot_while_a_sibling_retains_the_heap() -> crate::Result<()> {
    use allocative::FlameGraphBuilder;

    let (backing, good) = page_in_heap_with_a_failed_slot()?;

    let values: Vec<_> = good.owner().heap_arc().iter_values().collect();
    assert_eq!(values.len(), 1, "the failed slot is not a readable value");
    assert_eq!(values[0].downcast_ref::<SimpleData>().unwrap().count, 5);

    let mut graph = FlameGraphBuilder::default();
    graph.visit_root(&good);
    let flamegraph = graph.finish().flamegraph().write();
    assert!(
        flamegraph.contains("UninitializedValue"),
        "the failed slot's bytes are reported as uninitialized:\n{flamegraph}"
    );
    let summary = good.owner().heap_arc().allocated_summary();
    assert!(
        summary.summary.contains_key("UninitializedValue"),
        "the per-type summary counts the failed slot: {:?}",
        summary.summary.keys().collect::<Vec<_>>()
    );

    let live = ErasingHeap::new();
    live.add_reference(good.owner());
    let pointer = live.alloc_ref_data(1, erase(good.as_ref().value()));
    let live_owner = live.into_ref_named(TestHeapName::heap_name("points_at_failed_slot_heap"));
    // SAFETY: `live_owner` owns the arena hosting `pointer`; it retains `good`'s heap.
    let live_root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(live_owner, pointer) };
    ser_owned_frozen_value_into_storage(&backing, &live_root)?;
    Ok(())
}

/// Page-out builds a restored heap's pointer index while other threads are
/// still claiming its slots; every index published must describe the arena
/// as it is.
#[test]
fn test_serialization_index_keeps_up_with_concurrent_claims() -> crate::Result<()> {
    use dupe::Dupe;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    const VALUES: usize = 12;
    let heap = ErasingHeap::new();
    let values: Vec<_> = (0..VALUES)
        .map(|count| heap.alloc_simple(SimpleData { flag: true, count }))
        .collect();
    let owner = heap.into_ref_named(TestHeapName::heap_name("index_vs_claims"));
    // SAFETY: `owner` owns the arena hosting every value.
    let roots: Vec<OwnedFrozen<Value>> = values
        .iter()
        .map(|value| unsafe { OwnedFrozen::from_erased(owner.clone(), *value) })
        .collect();

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let mut ser = SerializerForPaging::new(storage.storage_context());
    for root in &roots {
        root.pagable_serialize(&mut ser)
            .map_err(crate::Error::new_other)?;
    }
    let (data, arcs) = ser.finish();
    let key = storage
        .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
        .map_err(crate::Error::new_other)?;
    storage.flush().map_err(crate::Error::new_other)?;
    drop(roots);
    drop(owner);
    storage.arc_cache().clear();

    let handle = PagableStorageHandle::new(storage.clone());
    let data = storage
        .fetch_data_blocking(&key)
        .map_err(crate::Error::new_other)?;
    let first = {
        let mut de = handle.root_deserializer(key, &data);
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?
    };
    let point_at = |target: &OwnedFrozen<Value<'static>>, name: String| {
        let live = ErasingHeap::new();
        live.add_reference(target.owner());
        let pointer = live.alloc_ref_data(0, erase(target.as_ref().value()));
        let live_owner = live.into_ref_named(TestHeapName::heap_name(&name));
        // SAFETY: `live_owner` owns the arena hosting `pointer` and retains the target's heap.
        unsafe { OwnedFrozen::<Value>::from_erased(live_owner, pointer) }
    };

    // Readers claim the remaining slots while page-outs rebuild the index.
    let restored = std::thread::scope(|threads| {
        let readers: Vec<_> = (1..VALUES)
            .map(|n| {
                let handle = handle.dupe();
                let data = data.dupe();
                threads.spawn(move || {
                    let mut de = handle.root_deserializer(key, &data);
                    let mut last = None;
                    for _ in 0..=n {
                        last = Some(OwnedFrozen::<Value>::pagable_deserialize(&mut de));
                    }
                    last.expect("at least one root is read")
                })
            })
            .collect();
        for i in 0..8 {
            let live_root = point_at(&first, format!("index_vs_claims_live_{i}"));
            ser_owned_frozen_value_into_storage(&backing, &live_root)
                .expect("a page-out during claims succeeds");
        }
        readers
            .into_iter()
            .map(|reader| reader.join().expect("a reader must not panic"))
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(crate::Error::new_other)?;

    let last = restored.last().expect("every root was read");
    assert_eq!(
        last.as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .expect("the last root is SimpleData")
            .count,
        VALUES - 1
    );
    let live_root = point_at(last, "index_vs_claims_last".to_owned());
    ser_owned_frozen_value_into_storage(&backing, &live_root)?;
    Ok(())
}

/// A value whose deserialization parks until released, so a walk can be run
/// while its slot is claimed but not yet written.
struct ParkChannels {
    parked: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

static PARK: std::sync::Mutex<Option<ParkChannels>> = std::sync::Mutex::new(None);
static PUBLISH_DURING_WALK: std::sync::Mutex<Option<ParkChannels>> = std::sync::Mutex::new(None);

#[cfg(panic = "unwind")]
mod claim_unwind {
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;

    static PANIC_DESER: Mutex<Option<ParkChannels>> = Mutex::new(None);

    #[derive(Debug, Allocative)]
    struct PanicAfterRelease;

    impl crate::pagable::StarlarkSerialize for PanicAfterRelease {
        fn starlark_serialize(
            &self,
            _ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
        ) -> crate::Result<()> {
            Ok(())
        }
    }

    impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for PanicAfterRelease {
        fn starlark_deserialize(
            _ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
        ) -> crate::Result<Self> {
            let channels = PANIC_DESER
                .lock()
                .unwrap()
                .take()
                .expect("test gate is set");
            channels.parked.send(()).expect("test observes the claim");
            channels
                .release
                .recv_timeout(Duration::from_secs(10))
                .expect("test releases the deserializer");
            panic!("intentional test deserializer panic");
        }
    }

    #[derive(
        Debug,
        Display,
        Allocative,
        ProvidesStaticType,
        NoSerialize,
        StarlarkPagable
    )]
    #[display("PanicDeserData")]
    struct PanicDeserData {
        owned: String,
        panic: PanicAfterRelease,
    }

    starlark_simple_value!(PanicDeserData);

    #[starlark_value(type = "PanicDeserData")]
    impl<'v> StarlarkValue<'v> for PanicDeserData {}

    #[test]
    fn test_panicking_deserializer_fails_claim_and_wakes_waiter() -> crate::Result<()> {
        use std::panic::AssertUnwindSafe;
        use std::panic::catch_unwind;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        use pagable::storage::handle::PagableStorageHandle;
        use pagable::storage::in_memory::InMemoryPagableStorage;

        let heap = ErasingHeap::new();
        let good = heap.alloc_simple(SimpleData {
            flag: true,
            count: 17,
        });
        let bad = heap.alloc_simple(PanicDeserData {
            owned: "partially initialized".to_owned(),
            panic: PanicAfterRelease,
        });
        let owner = heap.into_ref_named(TestHeapName::heap_name("panicking_slot_sibling"));
        // SAFETY: `owner` owns the arena hosting both values.
        let good_root: OwnedFrozen<Value> =
            unsafe { OwnedFrozen::from_erased(owner.clone(), good) };
        let bad_root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner, bad) };
        let backing = InMemoryPagableStorage::new();
        let good_key = ser_owned_frozen_value_into_storage(&backing, &good_root)?;
        let bad_key = ser_owned_frozen_value_into_storage(&backing, &bad_root)?;
        let storage = backing.handle();
        storage.flush().map_err(crate::Error::new_other)?;
        drop((good_root, bad_root));
        storage.arc_cache().clear();

        let handle = PagableStorageHandle::new(storage.clone());
        let good = deser_owned_frozen_from_storage(&backing, &handle, &good_key)?;
        let state = good.owner().heap_arc().deser_state().unwrap();
        let (wait_started_tx, wait_started_rx) = mpsc::channel();
        state.notify_on_wait_for_test(None, wait_started_tx);
        let data = storage
            .fetch_data_blocking(&bad_key)
            .map_err(crate::Error::new_other)?;
        let read_bad = move || {
            let mut de = handle.root_deserializer(bad_key, &data);
            OwnedFrozen::<Value>::pagable_deserialize(&mut de).map(|_| ())
        };

        let (parked_tx, parked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *PANIC_DESER.lock().unwrap() = Some(ParkChannels {
            parked: parked_tx,
            release: release_rx,
        });
        let (claimer_tx, claimer_rx) = mpsc::channel();
        let claimer = {
            let read_bad = read_bad.clone();
            thread::spawn(move || {
                claimer_tx
                    .send(catch_unwind(AssertUnwindSafe(read_bad)))
                    .expect("test observes the deserializer unwind");
            })
        };
        parked_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the claimer enters the gated deserializer");

        let (waiter_tx, waiter_rx) = mpsc::channel();
        let waiter = {
            let read_bad = read_bad.clone();
            thread::spawn(move || {
                waiter_tx
                    .send(read_bad())
                    .expect("test observes the waiting reader");
            })
        };
        wait_started_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the second reader must wait on the claimed slot");
        release_tx.send(()).expect("the claimer is still gated");

        let panic = claimer_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the claimer must finish unwinding")
            .expect_err("the deserializer must panic, not return an ordinary error");
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"intentional test deserializer panic")
        );
        let error = waiter_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the waiter must wake after the claimer panics")
            .expect_err("an abandoned claim must not become a readable value");
        assert!(format!("{error:#}").contains("value deserialization did not complete"));
        claimer.join().expect("panic was caught inside the claimer");
        waiter.join().expect("the waiting reader must not panic");

        let later = read_bad().expect_err("later readers must also see the failed slot");
        assert_eq!(format!("{later:#}"), format!("{error:#}"));
        let values: Vec<_> = good.owner().heap_arc().iter_values().collect();
        assert_eq!(values.len(), 1, "the abandoned slot remains uninitialized");
        assert_eq!(values[0].downcast_ref::<SimpleData>().unwrap().count, 17);
        Ok(())
    }
}

#[derive(Debug, Allocative)]
struct ParkField;

impl crate::pagable::StarlarkSerialize for ParkField {
    fn starlark_serialize(
        &self,
        _ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        Ok(())
    }
}

impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for ParkField {
    fn starlark_deserialize(
        _ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        if let Some(channels) = PARK.lock().unwrap().take() {
            channels.parked.send(()).ok();
            channels.release.recv().ok();
        }
        Ok(ParkField)
    }
}

#[derive(Debug, Display, ProvidesStaticType, NoSerialize, StarlarkPagable)]
#[display("ParkedData")]
struct ParkedData {
    owned: String,
    park: ParkField,
}

impl Allocative for ParkedData {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        if self.owned == "walk gate" {
            let publication = PUBLISH_DURING_WALK.lock().unwrap().take();
            if let Some(channels) = publication {
                channels.parked.send(()).unwrap();
                channels
                    .release
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("publication and a new claim must finish during the walk");
            }
        }
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(allocative::Key::new("owned"), &self.owned);
        visitor.visit_field(allocative::Key::new("park"), &self.park);
        visitor.exit();
    }
}

starlark_simple_value!(ParkedData);

#[starlark_value(type = "ParkedData")]
impl<'v> StarlarkValue<'v> for ParkedData {}

/// Walk an unfinished slot, then let its writer finish and claim another slot
/// inside a profiling callback. The snapshot must exclude the later allocation
/// without blocking the writer or retaining shared references to mutable bytes.
#[test]
fn test_walks_over_a_slot_still_being_deserialized() -> crate::Result<()> {
    use std::sync::mpsc;
    use std::time::Duration;

    use allocative::FlameGraphBuilder;
    use dupe::Dupe;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    let heap = ErasingHeap::new();
    let plain = heap.alloc_simple(ParkedData {
        owned: "walk gate".to_owned(),
        park: ParkField,
    });
    let parked = heap.alloc_simple(ParkedData {
        owned: "owned".to_owned(),
        park: ParkField,
    });
    let later = heap.alloc_simple(ParkedData {
        owned: "claimed during walk".to_owned(),
        park: ParkField,
    });
    let owner = heap.into_ref_named(TestHeapName::heap_name("slot_in_progress"));
    // SAFETY: `owner` owns the arena hosting all three values.
    let plain_root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner.clone(), plain) };
    let parked_root: OwnedFrozen<Value> =
        unsafe { OwnedFrozen::from_erased(owner.clone(), parked) };
    let later_root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner, later) };

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let mut ser = SerializerForPaging::new(storage.storage_context());
    plain_root
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    parked_root
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    later_root
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();
    let key = storage
        .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
        .map_err(crate::Error::new_other)?;
    storage.flush().map_err(crate::Error::new_other)?;
    drop((plain_root, parked_root, later_root));
    storage.arc_cache().clear();

    let handle = PagableStorageHandle::new(storage.clone());
    let data = storage
        .fetch_data_blocking(&key)
        .map_err(crate::Error::new_other)?;
    let plain = {
        let mut de = handle.root_deserializer(key, &data);
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?
    };

    let (parked_tx, parked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (published_tx, published_rx) = mpsc::channel();
    *PARK.lock().unwrap() = Some(ParkChannels {
        parked: parked_tx,
        release: release_rx,
    });
    let reader = {
        let handle = handle.dupe();
        let data = data.dupe();
        std::thread::spawn(move || {
            let mut de = handle.root_deserializer(key, &data);
            let _plain = OwnedFrozen::<Value>::pagable_deserialize(&mut de)?;
            let parked = OwnedFrozen::<Value>::pagable_deserialize(&mut de)?;
            let later = OwnedFrozen::<Value>::pagable_deserialize(&mut de)?;
            published_tx.send(()).unwrap();
            pagable::Result::Ok((parked, later))
        })
    };
    parked_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the reader parks inside the second value");

    let values: Vec<_> = plain.owner().heap_arc().iter_values().collect();
    assert_eq!(values.len(), 1, "the in-progress slot is not yet readable");
    assert_eq!(
        values[0].downcast_ref::<ParkedData>().unwrap().owned,
        "walk gate"
    );

    let mut graph = FlameGraphBuilder::default();
    graph.visit_root(&plain);
    let flamegraph = graph.finish().flamegraph().write();
    assert!(
        flamegraph.contains("UninitializedValue"),
        "the slot being deserialized is reported as uninitialized:\n{flamegraph}"
    );

    *PUBLISH_DURING_WALK.lock().unwrap() = Some(ParkChannels {
        parked: release_tx,
        release: published_rx,
    });
    let summary = plain.owner().heap_arc().allocated_summary();
    assert_eq!(
        summary
            .summary
            .values()
            .map(|counts| counts.count)
            .sum::<usize>(),
        2,
        "the walk excludes the new claim made after its chunk snapshot"
    );
    let (restored, _later) = reader
        .join()
        .expect("the reader must not panic")
        .map_err(crate::Error::new_other)?;
    assert!(
        restored
            .as_ref()
            .value()
            .downcast_ref::<ParkedData>()
            .is_some(),
        "the parked value completes once released"
    );
    let values: Vec<_> = plain.owner().heap_arc().iter_values().collect();
    assert_eq!(
        values.len(),
        3,
        "the completed and newly claimed slots are now included"
    );
    assert_eq!(
        values
            .iter()
            .filter_map(|v| v.downcast_ref::<ParkedData>())
            .map(|v| v.owned.as_str())
            .collect::<Vec<_>>(),
        ["walk gate", "owned", "claimed during walk"]
    );
    Ok(())
}

#[starlark_derive::starlark_module]
fn register_foo(builder: &mut GlobalsBuilder) {
    fn foo() -> anyhow::Result<i32> {
        Ok(1)
    }
}

starlark::globals_static!(
    STATIC_GLOBALS = |static_globals| {
        static_globals.set("x", NoneType);
    }
);

#[test]
fn test_globals_roundtrip() {
    use pagable::PagableSerialize;
    let mut globals = GlobalsBuilder::new();
    STATIC_GLOBALS.populate(&mut globals);

    register_list(&mut globals);
    globals.namespace_no_docs("ns_hidden", |_| {});
    globals.namespace("ns", |globals| {
        globals.namespace_no_docs("nested_ns_hidden", |_| {});
        globals.set("x", NoneType);
        register_dict(globals);
    });

    let globals = globals.build_named(GlobalFrozenHeapName {
        name: TESTING_GLOBALS_HEAP_NAME,
    });

    let mut serializer = pagable::testing::TestingSerializer::new();
    globals.pagable_serialize(&mut serializer).unwrap();
}

/// Round-trip an `OwnedFrozen` through `SerializerForPaging` +
/// `InMemoryPagableStorage` + `PagableDeserializerImpl` (the real concrete
/// impls used in production, not the testing ones). Consumes and drops the
/// source before page-in so the serialized heap is reconstructed.
fn round_trip_owned_pagable_ser_de_impl(
    owned: OwnedFrozen<Value<'static>>,
) -> crate::Result<OwnedFrozen<Value<'static>>> {
    use std::any::TypeId;

    use pagable::PagableDeserialize;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let handle = PagableStorageHandle::new(storage.clone());
    let storage_ctx = storage.storage_context();

    let mut ser = SerializerForPaging::new(storage_ctx);
    owned
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();

    let top_key = {
        let finished = ArcSerCache::new();
        storage
            .page_out_item(data, arcs, &finished, storage_ctx)
            .map_err(crate::Error::new_other)?
    };
    drop(owned);

    let top_data = storage
        .fetch_arc_or_data_blocking(&TypeId::of::<()>(), &top_key)
        .map_err(crate::Error::new_other)?
        .right()
        .expect("top-level key should return data, not a cached arc");

    let mut de = handle.root_deserializer(top_key, &top_data);
    <OwnedFrozen<Value>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)
}

/// A function's module contains a dictionary keyed by that same function.
#[test]
fn test_unfinished_function_dict_key_is_rejected() -> crate::Result<()> {
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;

    let ast = AstModule::parse(
        "key_cycle.star",
        "def f():\n    return 0\nd = {f: 1}\n".to_owned(),
        &Dialect::Extended,
    )?;
    let globals = GlobalsBuilder::new().build_named(GlobalFrozenHeapName {
        name: TEST_EVAL_HEAP_NAME,
    });
    let module = Module::with_temp_heap(|module| {
        Evaluator::new(&module).eval_module(ast, &globals).unwrap();
        module.freeze_named(TestHeapName::heap_name("unfinished_function_dict_key"))
    })?;
    let root = module.get("f")?;
    drop(module);
    let error = round_trip_owned_pagable_ser_de_impl(root)
        .expect_err("hashing an unfinished dictionary key must fail closed");
    assert!(
        format!("{error:#}").contains("depends on its unfinished constructor"),
        "{error:#}"
    );
    Ok(())
}

/// Same scenario as `test_cross_heap_frozen_value_round_trip` but routed
/// through `SerializerForPaging` + `InMemoryPagableStorage` +
/// `PagableDeserializerImpl`.
#[test]
fn test_cross_heap_frozen_value_round_trip_via_storage() -> crate::Result<()> {
    let dep_heap = ErasingHeap::new();
    let dep_fv = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 77,
    });
    let dep_heap_ref = dep_heap.into_ref_named(TestHeapName::heap_name("dep_storage"));

    let main_heap = ErasingHeap::new();
    main_heap.add_reference(dep_heap_ref.owner());
    let ref_fv = main_heap.alloc_ref_data(99, dep_fv);
    let main_heap_ref = main_heap.into_ref_named(TestHeapName::heap_name("main_storage"));

    // SAFETY: `main_heap_ref` keeps the arena hosting `ref_fv` alive.
    let owned = unsafe { OwnedFrozen::from_erased(main_heap_ref, ref_fv) };
    drop(dep_heap_ref);
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;

    let undrop_headers = restored.owner().heap_arc().collect_undrop_headers_ordered();
    assert_eq!(undrop_headers.len(), 1);
    let ref_data: &RefData = undrop_headers[0].unpack().downcast_ref().unwrap();
    assert_eq!(ref_data.label, 99);
    let resolved: &SimpleData = ref_data
        .target
        .downcast_ref::<SimpleData>()
        .expect("target should resolve to SimpleData in dep heap");
    assert_eq!(resolved.flag, true);
    assert_eq!(resolved.count, 77);
    Ok(())
}

/// Storage-path counterpart to `test_arc_blanket_round_trip`.
/// `ArcBlanketOuter` owns `Arc<ArcBlanketInner>` whose inner holds a value.
/// `PagableDeserializerImpl` reads the inner `Arc` body through its own
/// sub-deserializer, so a naive `seek(target.abs_pos)` would land in the
/// wrong stream; `ensure` instead seeks a deserializer reconstructed from
/// the owning heap's recipe (stored on its `HeapDeserializationState`).
#[test]
fn test_arc_blanket_round_trip_via_storage() {
    let heap = ErasingHeap::new();
    let shared = Arc::new(ArcBlanketInner { label: 99 });

    let outer_a_fv = heap.alloc_simple(ArcBlanketOuter {
        inner: shared.clone(),
        outer_label: 1,
        items: vec![10, 20, 30],
    });
    heap.alloc_simple(ArcBlanketOuter {
        inner: shared,
        outer_label: 2,
        items: vec![40, 50],
    });

    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("test_arc_blanket_storage"));

    // SAFETY: `heap_ref` keeps the arena hosting `outer_a_fv` alive.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, outer_a_fv) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned).expect("round-trip via storage");

    // Partial deser: only the root `ArcBlanketOuter` (and its transitive deps)
    // is materialized. The second `ArcBlanketOuter`, which is unreachable from
    // the root, is never allocated.
    let drop_headers = restored.owner().heap_arc().collect_drop_headers_ordered();
    assert_eq!(drop_headers.len(), 1);
    let outer_a: &ArcBlanketOuter = drop_headers[0].unpack().downcast_ref().unwrap();
    assert_eq!(outer_a.outer_label, 1);
    assert_eq!(outer_a.inner.label, 99);
}

/// Under partial deser, a shared `Arc<T>` body encoded under `HA`'s context can be decoded
/// under `HB`'s.
///
/// `HB` refs `HA` and shares the `Arc<ArcBlanketInner>`. Serializing `OFV(HB)` encodes the Arc
/// body under `HA` (walked first as `HB`'s ref); deser of `HB` decodes it from there.
#[test]
fn test_cross_heap_arc_dedup_round_trip() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    // HA: ArcBlanketOuter_A holding Arc<ArcBlanketInner>.
    let heap_a = ErasingHeap::new();
    let shared = Arc::new(ArcBlanketInner { label: 42 });
    heap_a.alloc_simple(ArcBlanketOuter {
        inner: shared.clone(),
        outer_label: 1,
        items: vec![10, 20, 30],
    });
    let heap_a_ref = heap_a.into_ref_named(TestHeapName::heap_name(
        "test_cross_heap_arc_dedup_explicit_heap_id_HA",
    ));

    // HB: refs HA, holds ArcBlanketOuter_B with a clone of the same Arc.
    let heap_b = ErasingHeap::new();
    heap_b.add_reference(heap_a_ref.owner());
    let outer_b_fv = heap_b.alloc_simple(ArcBlanketOuter {
        inner: shared,
        outer_label: 2,
        items: vec![40, 50],
    });
    let heap_b_ref = heap_b.into_ref_named(TestHeapName::heap_name(
        "test_cross_heap_arc_dedup_explicit_heap_id_HB",
    ));

    // SAFETY: heap_b_ref keeps the arena hosting outer_b_fv alive.
    let ofv_b = unsafe { OwnedFrozen::from_erased(heap_b_ref, outer_b_fv) };

    // Serializing OFV(HB, …) walks HB.refs first inside `page_out_item`, so HA
    // — and the ArcBlanketInner Arc body reachable through HA — is encoded under
    // HA's serialize context before HB's body is written.
    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key_b = ser_owned_frozen_value_into_storage(&backing, &ofv_b)?;

    drop(ofv_b);
    drop(heap_a_ref);

    // Deserialize HB. The Arc body bytes — produced under HA's serialize
    // context — are decoded inside HB's deserialize.
    let restored_b = deser_owned_frozen_from_storage(&backing, &handle, &key_b)?;
    let outer_b: &ArcBlanketOuter = restored_b
        .as_ref()
        .value()
        .downcast_ref::<ArcBlanketOuter>()
        .expect("restored_b root is ArcBlanketOuter");
    assert_eq!(outer_b.outer_label, 2);
    assert_eq!(outer_b.items, vec![40, 50]);
    assert_eq!(outer_b.inner.label, 42);

    // HB's owner refs HA; ArcBlanketOuter_A is unreachable from HB's root and intentionally
    // stays unmaterialized.
    let hb_refs: Vec<_> = restored_b.owner().refs().collect();
    assert_eq!(hb_refs.len(), 1, "HB should retain its ref to HA");
    let restored_ha = hb_refs[0];
    assert_eq!(
        restored_ha.heap_arc().collect_drop_headers_ordered().len(),
        0,
        "nothing in HA is reachable from HB's root",
    );

    Ok(())
}

/// Partial deserialization proper: allocate multiple values, only one is the
/// root. After the round-trip via the storage path, only the reachable values
/// are materialized in the arena.
#[test]
fn test_partial_deser_skips_unreachable_values() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let reachable_fv = heap.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    // These two are allocated but unreachable from the root.
    heap.alloc_simple(SimpleData {
        flag: false,
        count: 99,
    });
    heap.alloc_simple(SimpleData {
        flag: true,
        count: 100,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("partial_skip"));

    // SAFETY: `heap_ref` keeps the arena alive.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, reachable_fv) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;

    let undrop = restored.owner().heap_arc().collect_undrop_headers_ordered();
    // Only the reachable SimpleData is materialized; the other two are skipped.
    assert_eq!(undrop.len(), 1);
    let data: &SimpleData = undrop[0].unpack().downcast_ref().unwrap();
    assert_eq!(data.flag, true);
    assert_eq!(data.count, 1);
    Ok(())
}

/// Partial deser materializes values in demand-walk order, which may differ
/// from the original allocation order. Source order is [SimpleData(target),
/// RefData(root)] — A is allocated first, then B which references A. The
/// root (B) is claimed before resolving its `target` recursively restores A.
/// The restored arena ends up [B, A], not [A, B].
#[test]
fn test_partial_deser_materializes_in_demand_order() -> crate::Result<()> {
    let heap = ErasingHeap::new();
    let target_fv = heap.alloc_simple(SimpleData {
        flag: true,
        count: 42,
    });
    let root_fv = heap.alloc_ref_data(7, target_fv);
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("demand_order"));

    // SAFETY: `heap_ref` keeps the arena alive.
    let owned = unsafe { OwnedFrozen::from_erased(heap_ref, root_fv) };
    let restored = round_trip_owned_pagable_ser_de_impl(owned)?;

    let undrop = restored.owner().heap_arc().collect_undrop_headers_ordered();
    assert_eq!(undrop.len(), 2);
    // Demand-walk order: root B (RefData) was materialized first, then its
    // target A (SimpleData). Original allocation order was the reverse.
    let first: &RefData = undrop[0]
        .unpack()
        .downcast_ref::<RefData>()
        .expect("first header should be RefData (materialized first as the root)");
    let second: &SimpleData = undrop[1]
        .unpack()
        .downcast_ref::<SimpleData>()
        .expect("second header should be SimpleData (materialized via root's target)");
    assert_eq!(first.label, 7);
    assert_eq!(second.count, 42);

    // The root's `target` resolves to the SimpleData allocation.
    assert_eq!(
        first.target.ptr_value().ptr_value_untagged(),
        undrop[1] as *const _ as usize,
    );
    Ok(())
}

/// A pointer into a heap that no ref list read so far names - the shape of a
/// value relocated into a shared blob - resolves through the heap-key index,
/// and fails cleanly rather than dangling without it.
#[cfg(fbcode_build)]
#[test]
fn test_pointer_into_unlisted_heap_resolves_by_identity() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    use crate::values::layout::heap::sealed::heap_key_index::StarlarkHeapKeyIndex;

    let heap_g = ErasingHeap::new();
    let g_fv = heap_g.alloc_simple(SimpleData {
        flag: true,
        count: 99,
    });
    let g_ref = heap_g.into_ref_named(TestHeapName::heap_name("unlisted_g"));
    let g_id = g_ref.heap_arc().heap_ref_id().expect("G is named");

    // Two heaps point into G without listing it as a dependency; G outlives
    // them here. One is paged in without the index, one with it.
    let point_into_g = |label: usize, name: &str| {
        let heap = ErasingHeap::new();
        let fv = heap.alloc_ref_data(label, g_fv);
        let owner = heap.into_ref_named(TestHeapName::heap_name(name));
        assert!(
            owner.heap_arc().refs_slice().is_empty(),
            "the test only means something if the heap lists no dependencies"
        );
        // SAFETY: `owner` owns the arena hosting `fv`; G is kept alive by the test.
        unsafe { OwnedFrozen::<Value>::from_erased(owner, fv) }
    };
    let without_index = point_into_g(1, "unlisted_a1");
    let with_index = point_into_g(2, "unlisted_a2");

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    // G is written first, in a root of its own, so page-out can resolve the
    // pointers into it and the index learns its row.
    // SAFETY: `g_ref` owns the arena hosting `g_fv`.
    let g_ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(g_ref.clone(), g_fv) };
    let _g_key = ser_owned_frozen_value_into_storage(&backing, &g_ofv)?;
    let key_without = ser_owned_frozen_value_into_storage(&backing, &without_index)?;
    let key_with = ser_owned_frozen_value_into_storage(&backing, &with_index)?;
    drop(without_index);
    drop(with_index);
    drop(g_ofv);
    drop(g_ref);

    let index = handle
        .storage_context()
        .get::<StarlarkHeapKeyIndex>()
        .expect("page-out registers the index");
    let g_row = index.get(&g_id).expect("page-out indexed G's row");

    index.clear();
    assert!(
        deser_owned_frozen_from_storage(&backing, &handle, &key_without).is_err(),
        "without the index a pointer into an unlisted heap has nothing to bind"
    );

    index.insert(g_id, g_row);
    let (restored, scope) =
        deser_owned_frozen_value_with_scope_from_storage(&backing, &handle, &key_with)?;
    let weak_target = scope.get_heap(&g_id).unwrap().downgrade().unwrap();
    drop(scope);
    drop(handle);
    drop(backing);
    assert!(
        weak_target.upgrade().is_some(),
        "the restored owner must retain the target after storage is dropped"
    );
    let root: &RefData = restored
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("the restored root is RefData");
    let target: &SimpleData = root
        .target
        .to_value()
        .downcast_ref::<SimpleData>()
        .expect("the root's target lives in G");
    assert_eq!(
        target.count, 99,
        "the pointer into G resolves through the index"
    );
    drop(restored);
    assert!(
        weak_target.upgrade().is_none(),
        "the target must be freed with its last owner"
    );
    Ok(())
}

#[cfg(fbcode_build)]
#[test]
fn test_scope_cache_hit_retains_unlisted_target_in_each_owner() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    use crate::pagable::serialized_frozen_value::SerializedFrozenValue;
    use crate::pagable::starlark_deserialize_context::StarlarkDeserializerImpl;

    let target = ErasingHeap::new();
    let target_value = target.alloc_simple(SimpleData {
        flag: true,
        count: 99,
    });
    let target_owner = target.into_ref_named(TestHeapName::heap_name("cache_hit_target"));
    let target_id = target_owner
        .heap_arc()
        .heap_ref_id()
        .expect("the target is named");
    // SAFETY: `target_owner` owns the arena hosting `target_value`.
    let target_owned = unsafe { OwnedFrozen::from_erased(target_owner, target_value) };
    let backing = InMemoryPagableStorage::new();
    ser_owned_frozen_value_into_storage(&backing, &target_owned)?;

    let storage = backing.handle();
    let mut ser = SerializerForPaging::new(storage.storage_context());
    for name in ["cache_hit_owner_a", "cache_hit_owner_b"] {
        let heap = ErasingHeap::new();
        heap.alloc_simple(SimpleData {
            flag: false,
            count: 0,
        });
        let owner = heap.into_ref_named(TestHeapName::heap_name(name));
        assert!(owner.heap_arc().refs_slice().is_empty());
        owner
            .pagable_serialize(&mut ser)
            .map_err(crate::Error::new_other)?;
        // An owning carrier's wire format is its owner followed by its root pointer.
        SerializedFrozenValue::HeapPtr {
            heap_id: target_id,
            value_index: 0,
            is_str: false,
        }
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    }
    let (data, arcs) = ser.finish();
    let key = storage
        .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
        .map_err(crate::Error::new_other)?;
    storage.flush().map_err(crate::Error::new_other)?;
    drop(target_owned);
    storage.arc_cache().clear();

    let handle = PagableStorageHandle::new(storage.clone());
    let data = storage
        .fetch_data_blocking(&key)
        .map_err(crate::Error::new_other)?;
    let mut de = handle.root_deserializer(key, &data);
    let first =
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let second =
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let scope = StarlarkDeserializerImpl::get_or_create_scope(&mut de);
    let weak_target = scope.get_heap(&target_id).unwrap().downgrade().unwrap();

    // The root points into a retained heap, so reserialization must index that heap too.
    let second_key = ser_owned_frozen_value_into_storage(&backing, &second)?;
    let roundtrip = deser_owned_frozen_from_storage(&backing, &handle, &second_key)?;
    assert_eq!(
        roundtrip
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .unwrap()
            .count,
        99
    );
    drop(roundtrip);
    drop(de);
    drop(scope);
    drop(first);
    drop(handle);
    drop(storage);
    drop(backing);
    assert!(
        weak_target.upgrade().is_some(),
        "the second owner must retain the cache-hit target without the first owner or storage"
    );
    assert_eq!(
        second
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .unwrap()
            .count,
        99
    );
    drop(second);
    assert!(
        weak_target.upgrade().is_none(),
        "self-retention must not leak the target"
    );
    Ok(())
}

/// Two heaps sealed under one name, each the unlisted target of a different
/// pointer. Identity tells them apart, so each pointer resolves into the heap
/// it was written against.
#[cfg(fbcode_build)]
#[test]
fn test_pointers_into_unlisted_heaps_sealed_under_one_name_resolve_by_identity() -> crate::Result<()>
{
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    let make_target = |count| {
        let heap = ErasingHeap::new();
        let value = heap.alloc_simple(SimpleData { flag: true, count });
        let owner = heap.into_ref_named(TestHeapName::heap_name("same_name_target"));
        (owner, value)
    };
    let (g1_ref, g1_fv) = make_target(111);
    let (g2_ref, g2_fv) = make_target(222);
    assert_ne!(
        g1_ref.heap_arc().heap_ref_id(),
        g2_ref.heap_arc().heap_ref_id()
    );

    let point_into = |label: usize, name: &str, target| {
        let heap = ErasingHeap::new();
        let fv = heap.alloc_ref_data(label, target);
        let owner = heap.into_ref_named(TestHeapName::heap_name(name));
        assert!(owner.heap_arc().refs_slice().is_empty());
        // SAFETY: `owner` owns the arena hosting `fv`; the targets outlive it here.
        unsafe { OwnedFrozen::<Value>::from_erased(owner, fv) }
    };
    let into_g1 = point_into(1, "same_name_referrer_1", g1_fv);
    let into_g2 = point_into(2, "same_name_referrer_2", g2_fv);

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    // SAFETY: each owner owns the arena hosting its value.
    let g1_ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(g1_ref.clone(), g1_fv) };
    let g2_ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(g2_ref.clone(), g2_fv) };
    let _g1_key = ser_owned_frozen_value_into_storage(&backing, &g1_ofv)?;
    let _g2_key = ser_owned_frozen_value_into_storage(&backing, &g2_ofv)?;
    let key_into_g1 = ser_owned_frozen_value_into_storage(&backing, &into_g1)?;
    let key_into_g2 = ser_owned_frozen_value_into_storage(&backing, &into_g2)?;
    drop((into_g1, into_g2, g1_ofv, g2_ofv, g1_ref, g2_ref));

    let count = |key| -> crate::Result<usize> {
        let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
        let root: &RefData = restored
            .as_ref()
            .value()
            .downcast_ref::<RefData>()
            .expect("the restored root is RefData");
        Ok(root
            .target
            .to_value()
            .downcast_ref::<SimpleData>()
            .expect("the target is SimpleData")
            .count)
    };
    assert_eq!(
        count(key_into_g1)?,
        111,
        "the first pointer finds the first heap"
    );
    assert_eq!(
        count(key_into_g2)?,
        222,
        "the second pointer finds the second heap"
    );
    Ok(())
}

/// Page-in reads the owner's row and no more: a dependency nothing points
/// into is bound by name from the slot that names it, retained, and left in
/// storage.
#[cfg(fbcode_build)]
#[test]
fn test_unread_dependency_is_bound_but_not_read() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    use crate::pagable::starlark_partial_deser_stats;

    let dep = ErasingHeap::new();
    dep.alloc_simple(SimpleData {
        flag: true,
        count: 7,
    });
    let dep_ref = dep.into_ref_named(TestHeapName::heap_name("unread_dep"));

    let owner = ErasingHeap::new();
    owner.add_reference(dep_ref.owner());
    let root_fv = owner.alloc_simple(SimpleData {
        flag: false,
        count: 1,
    });
    let owner_ref = owner.into_ref_named(TestHeapName::heap_name("unread_owner"));
    // SAFETY: `owner_ref` owns the arena hosting `root_fv`.
    let ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner_ref, root_fv) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key = ser_owned_frozen_value_into_storage(&backing, &ofv)?;
    drop(ofv);
    drop(dep_ref);

    let before = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    let after = starlark_partial_deser_stats().expect("counters are on under cfg(test)");

    let root: &SimpleData = restored
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .expect("the restored root is SimpleData");
    assert_eq!(root.count, 1, "the root reads back");
    assert_eq!(
        after.heaps_loaded - before.heaps_loaded,
        1,
        "only the owner's row is read; the dependency is bound from the slot that names it"
    );
    let dep = restored
        .owner()
        .refs()
        .next()
        .expect("the owner retains its dependency");
    assert_eq!(
        dep.name().map(ToString::to_string).as_deref(),
        Some("TestHeapName(unread_dep)"),
        "the skeleton knows its name without a read"
    );
    assert!(
        !dep.heap_arc().is_header_loaded(),
        "nothing pointed into the dependency, so its row stays in storage"
    );
    Ok(())
}

#[derive(
    Debug,
    ProvidesStaticType,
    Allocative,
    starlark_derive::StarlarkPagable
)]
struct DeferredRef<'v> {
    target: Deferred<Value<'v>>,
}

crate::register_starlark_any_complex!(frozen DeferredRef<'_>);

/// Storage whose rows can be made unreadable one key at a time.
#[cfg(fbcode_build)]
struct FailingRowStorage {
    inner: Arc<dyn pagable::storage::traits::PagableStorage>,
    unreadable: std::sync::Mutex<Vec<pagable::DataKey>>,
}

#[cfg(fbcode_build)]
impl FailingRowStorage {
    fn check(&self, key: &pagable::DataKey) -> anyhow::Result<()> {
        if self.unreadable.lock().unwrap().contains(key) {
            return Err(anyhow::anyhow!("row {key:?} is unreadable"));
        }
        Ok(())
    }
}

// Written out by hand because `async_trait` is not a dependency of this crate.
#[cfg(fbcode_build)]
impl pagable::storage::traits::PagableStorage for FailingRowStorage {
    fn arc_cache(&self) -> &pagable::storage::traits::DeserializedArcCache {
        self.inner.arc_cache()
    }

    fn fetch_data_blocking(
        &self,
        key: &pagable::DataKey,
    ) -> anyhow::Result<Arc<pagable::storage::data::PagableData>> {
        self.check(key)?;
        self.inner.fetch_data_blocking(key)
    }

    fn fetch_data<'life0, 'life1, 'async_trait>(
        &'life0 self,
        key: &'life1 pagable::DataKey,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = anyhow::Result<Arc<pagable::storage::data::PagableData>>,
                > + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.check(key)?;
            self.inner.fetch_data(key).await
        })
    }

    fn schedule_for_paging(&self, arc: Box<dyn pagable::arc_erase::ArcEraseDyn>) {
        self.inner.schedule_for_paging(arc)
    }

    fn storage_context(&self) -> &pagable::StorageContext {
        self.inner.storage_context()
    }

    fn store_data_ticketed(
        &self,
        data: pagable::storage::data::PagableData,
    ) -> anyhow::Result<(pagable::DataKey, pagable::storage::traits::WriteTicket)> {
        self.inner.store_data_ticketed(data)
    }

    fn commit_frontier(&self) -> pagable::storage::traits::CommitFrontier {
        self.inner.commit_frontier()
    }

    fn discard_unwritten(&self) {
        self.inner.discard_unwritten()
    }
}

#[cfg(fbcode_build)]
#[test]
fn heap_indexed_under_two_keys_is_reported_once() {
    use pagable::DataKey;
    use pagable::traits::StorageContext;

    use crate::pagable::heap_ref_id::HeapRefId;
    use crate::values::FrozenHeapName;
    use crate::values::layout::heap::sealed::heap_key_index::StarlarkHeapKeyIndex;
    use crate::values::layout::heap::sealed::heap_key_index::take_heap_rekeys;

    let context = StorageContext::new();
    let index = context.get_or_init(StarlarkHeapKeyIndex::default);
    let heap_id = HeapRefId::from_heap_name(&FrozenHeapName::user("rekeyed"));
    index.insert(heap_id, DataKey::testing_new(1));
    index.insert(heap_id, DataKey::testing_new(1));
    assert_eq!(take_heap_rekeys(&context), 0, "the same key is not a rekey");
    index.insert(heap_id, DataKey::testing_new(2));
    assert_eq!(take_heap_rekeys(&context), 1);
    assert_eq!(take_heap_rekeys(&context), 0, "each rekey is counted once");
}

#[cfg(fbcode_build)]
#[test]
fn test_unreadable_dependency_heap_is_recorded_as_a_deferred_read_failure() -> crate::Result<()> {
    use std::sync::Mutex;

    use pagable::DeferredReadFailures;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    use crate::pagable::DeferredFieldReadsEnabled;

    let dep = ErasingHeap::new();
    let target = dep.alloc_str("in the dependency");
    let dep_ref = dep.into_ref_named(TestHeapName::heap_name("unreadable_dep"));

    let owner = ErasingHeap::new();
    owner.add_reference(dep_ref.owner());
    let root_fv = owner.with(|heap| {
        erase(heap.alloc_simple(StarlarkAnyComplex {
            value: DeferredRef {
                target: Deferred::new(ErasingHeap::restore_one(heap, target)),
            },
        }))
    });
    let owner_ref = owner.into_ref_named(TestHeapName::heap_name("unreadable_owner"));
    // SAFETY: `owner_ref` owns the arena hosting `root_fv`.
    let ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner_ref, root_fv) };

    let backing = InMemoryPagableStorage::new();
    let failing = Arc::new(FailingRowStorage {
        inner: backing.handle(),
        unreadable: Mutex::new(Vec::new()),
    });
    let handle = PagableStorageHandle::new(failing.clone());
    handle
        .storage_context()
        .get_or_init(|| DeferredFieldReadsEnabled);
    let failures = handle
        .storage_context()
        .get_or_init(DeferredReadFailures::default);
    let key = ser_owned_frozen_value_into_storage(&backing, &ofv)?;
    drop(ofv);
    drop(dep_ref);

    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    let fields = restored
        .as_ref()
        .value()
        .downcast_ref::<StarlarkAnyComplex<DeferredRef>>()
        .expect("the restored root is a DeferredRef");
    assert!(
        fields.value.target.peek().is_none(),
        "the pointer into the dependency is deferred"
    );
    let dep = restored
        .owner()
        .refs()
        .next()
        .expect("the owner retains its dependency");
    assert!(
        !dep.heap_arc().is_header_loaded(),
        "the dependency is a skeleton"
    );
    let dep_row = dep
        .heap_arc()
        .deser_state()
        .and_then(|state| state.source())
        .expect("a skeleton knows its row");
    assert!(
        failures.snapshot().is_empty(),
        "the eager page-in records nothing"
    );

    failing.unreadable.lock().unwrap().push(dep_row);
    let error = fields
        .value
        .target
        .read()
        .expect_err("a dependency whose row is unreadable cannot be read");
    let failures = failures.snapshot();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("heap `TestHeapName(unreadable_owner)`: "),
        "the failure names the heap owning the field: {failures:?} ({error:#})"
    );
    assert!(
        fields.value.target.peek().is_none(),
        "the field stays unread"
    );
    Ok(())
}

#[cfg(fbcode_build)]
#[test]
fn test_prefetch_many_resolves_fields_without_a_later_storage_read() -> crate::Result<()> {
    use std::sync::Mutex;

    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    use crate::pagable::DeferredFieldReadsEnabled;

    // `prefetch_many` resolves on the current multi-thread runtime's blocking pool.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .expect("test runtime");
    let _runtime = runtime.enter();

    let dep = ErasingHeap::new();
    let target = dep.alloc_str("in the dependency");
    let dep_ref = dep.into_ref_named(TestHeapName::heap_name("prefetched_dep"));

    let owner = ErasingHeap::new();
    owner.add_reference(dep_ref.owner());
    let root_fv = owner.with(|heap| {
        erase(heap.alloc_simple(StarlarkAnyComplex {
            value: DeferredRef {
                target: Deferred::new(ErasingHeap::restore_one(heap, target)),
            },
        }))
    });
    let owner_ref = owner.into_ref_named(TestHeapName::heap_name("prefetched_owner"));
    // SAFETY: `owner_ref` owns the arena hosting `root_fv`.
    let ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner_ref, root_fv) };

    let backing = InMemoryPagableStorage::new();
    let failing = Arc::new(FailingRowStorage {
        inner: backing.handle(),
        unreadable: Mutex::new(Vec::new()),
    });
    let handle = PagableStorageHandle::new(failing.clone());
    handle
        .storage_context()
        .get_or_init(|| DeferredFieldReadsEnabled);
    let key = ser_owned_frozen_value_into_storage(&backing, &ofv)?;
    drop(ofv);
    drop(dep_ref);

    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    let fields = restored
        .as_ref()
        .value()
        .downcast_ref::<StarlarkAnyComplex<DeferredRef>>()
        .expect("the restored root is a DeferredRef");
    let dep = restored
        .owner()
        .refs()
        .next()
        .expect("the owner retains its dependency");
    assert!(
        !dep.heap_arc().is_header_loaded(),
        "the dependency starts as a skeleton"
    );
    let dep_row = dep
        .heap_arc()
        .deser_state()
        .and_then(|state| state.source())
        .expect("a skeleton knows its row");

    Deferred::prefetch_many([&fields.value.target])?;
    assert!(
        dep.heap_arc().is_header_loaded(),
        "the prefetch loaded the dependency heap"
    );
    assert!(
        fields.value.target.peek().is_some(),
        "the prefetch resolved the field"
    );

    // With its row now unreadable, the read can only succeed because the
    // field is already resident.
    failing.unreadable.lock().unwrap().push(dep_row);
    let value = fields.value.target.read()?;
    assert_eq!(value.unpack_str(), Some("in the dependency"));
    Ok(())
}

/// Three heaps where A lists only B, B lists G, and A's root points straight
/// into G; paged out into a fresh storage with the rows of all three indexed.
fn page_out_indirect_graph(
    prefix: &str,
) -> crate::Result<(
    pagable::storage::in_memory::InMemoryPagableStorage,
    pagable::storage::handle::PagableStorageHandle,
    pagable::DataKey,
)> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    let heap_g = ErasingHeap::new();
    let g_fv = heap_g.alloc_simple(SimpleData {
        flag: true,
        count: 99,
    });
    let g_ref = heap_g.into_ref_named(TestHeapName::heap_name(&format!("{prefix}_g")));

    let heap_b = ErasingHeap::new();
    heap_b.add_reference(g_ref.owner());
    heap_b.alloc_simple(SimpleData {
        flag: false,
        count: 5,
    });
    let b_ref = heap_b.into_ref_named(TestHeapName::heap_name(&format!("{prefix}_b")));

    // G stays alive through B, which is all `add_reference` is for.
    let heap_a = ErasingHeap::new();
    heap_a.add_reference(b_ref.owner());
    let a_fv = heap_a.alloc_ref_data(7, g_fv);
    let a_ref = heap_a.into_ref_named(TestHeapName::heap_name(&format!("{prefix}_a")));
    assert_eq!(
        a_ref.heap_arc().refs_slice(),
        std::slice::from_ref(&b_ref),
        "the test only means something if G is not a direct ref of A",
    );
    // SAFETY: `a_ref` owns the arena hosting `a_fv`.
    let ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(a_ref, a_fv) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key = ser_owned_frozen_value_into_storage(&backing, &ofv)?;
    drop(ofv);
    drop(b_ref);
    drop(g_ref);
    Ok((backing, handle, key))
}

/// Reads the root back and checks its pointer into G resolved.
fn assert_points_into_g(restored: &OwnedFrozen<Value<'static>>) {
    let root: &RefData = restored
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("the restored root is RefData");
    let target: &SimpleData = root
        .target
        .to_value()
        .downcast_ref::<SimpleData>()
        .expect("the root's target lives in G");
    assert_eq!(target.count, 99, "the pointer into G resolves");
}

/// A pointer into a heap that is not a direct dependency is found in the
/// index, read, and retained by the heap that used it; the heap between them
/// stays a skeleton.
#[cfg(fbcode_build)]
#[test]
fn test_pointer_into_indirect_dependency_resolves_through_the_index() -> crate::Result<()> {
    use crate::pagable::starlark_partial_deser_stats;

    let (backing, handle, key) = page_out_indirect_graph("indexed")?;

    let before = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    assert_points_into_g(&restored);
    let after = starlark_partial_deser_stats().expect("counters are on under cfg(test)");

    assert_eq!(
        after.heaps_loaded - before.heaps_loaded,
        2,
        "A's row is read with the root and G's to claim into it; B's is not needed"
    );
    assert_eq!(
        after.heaps_with_metadata - before.heaps_with_metadata,
        2,
        "A's and G's values are parsed; B's, which nothing points into, are not"
    );
    let retained = restored.owner().heap_arc().retained_beyond_refs();
    assert_eq!(
        retained
            .iter()
            .map(|heap| heap.name().map(ToString::to_string))
            .collect::<Vec<_>>(),
        vec![Some("TestHeapName(indexed_g)".to_owned())],
        "A holds G itself: no ref list of A's accounts for the pointer"
    );
    Ok(())
}

/// A value in a heap the owner retains beyond its ref list, projected out of
/// the owner, serializes: the owner's reachable heaps include the retained one.
#[test]
fn test_projection_into_retained_dependency_reserializes() -> crate::Result<()> {
    let (backing, handle, key) = page_out_indirect_graph("projected")?;
    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    assert_points_into_g(&restored);

    let root: &RefData = restored
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("the restored root is RefData");
    let g_value = erase(root.target);
    // SAFETY: A owns the projection and retains G, which hosts `g_value`.
    let projected: OwnedFrozen<Value> =
        unsafe { OwnedFrozen::from_erased(restored.owner().to_owned(), g_value) };
    drop(restored);

    let projected_key = ser_owned_frozen_value_into_storage(&backing, &projected)?;
    let back = deser_owned_frozen_from_storage(&backing, &handle, &projected_key)?;
    let target: &SimpleData = back
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .expect("the projection is G's value");
    assert_eq!(target.count, 99);
    Ok(())
}

/// A skeleton bound by one root page-in and first read by another resolves
/// the pointers in its values: bindings are per storage, not per root.
#[test]
#[cfg(fbcode_build)]
fn test_skeleton_first_read_by_another_root_resolves_its_dependencies() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    use crate::pagable::starlark_partial_deser_stats;
    use crate::values::layout::heap::sealed::heap_key_index::StarlarkHeapKeyIndex;

    let heap_g = ErasingHeap::new();
    let g_fv = heap_g.alloc_simple(SimpleData {
        flag: true,
        count: 99,
    });
    let g_ref = heap_g.into_ref_named(TestHeapName::heap_name("two_roots_g"));

    let heap_b = ErasingHeap::new();
    heap_b.add_reference(g_ref.owner());
    let b_fv = heap_b.alloc_ref_data(3, g_fv);
    let b_ref = heap_b.into_ref_named(TestHeapName::heap_name("two_roots_b"));

    // Root 1 lists B but points at nothing in it, so B stays a skeleton for it.
    let heap_a1 = ErasingHeap::new();
    heap_a1.add_reference(b_ref.owner());
    let a1_fv = heap_a1.alloc_simple(SimpleData {
        flag: false,
        count: 1,
    });
    let a1_ref = heap_a1.into_ref_named(TestHeapName::heap_name("two_roots_a1"));
    // Root 2 points into B, whose value points into G.
    let heap_a2 = ErasingHeap::new();
    heap_a2.add_reference(b_ref.owner());
    let a2_fv = heap_a2.alloc_ref_data(7, b_fv);
    let a2_ref = heap_a2.into_ref_named(TestHeapName::heap_name("two_roots_a2"));
    // SAFETY: each ref owns the arena hosting its value.
    let ofv1: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(a1_ref, a1_fv) };
    let ofv2: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(a2_ref, a2_fv) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key1 = ser_owned_frozen_value_into_storage(&backing, &ofv1)?;
    let key2 = ser_owned_frozen_value_into_storage(&backing, &ofv2)?;
    drop((ofv1, ofv2, b_ref, g_ref));
    handle
        .storage_context()
        .get::<StarlarkHeapKeyIndex>()
        .expect("page-out registers the index")
        .clear();

    let before = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    let restored1 = deser_owned_frozen_from_storage(&backing, &handle, &key1)?;
    let mid = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    assert_eq!(
        mid.heaps_loaded - before.heaps_loaded,
        1,
        "root 1 reads its own row only: B is bound as a skeleton"
    );

    let restored2 = deser_owned_frozen_from_storage(&backing, &handle, &key2)?;
    let after = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    assert_eq!(
        after.heaps_loaded - mid.heaps_loaded,
        3,
        "root 2 reads its own row, B's, and G's"
    );
    let root: &RefData = restored2
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("root 2 is RefData");
    let in_b: &RefData = root
        .target
        .to_value()
        .downcast_ref::<RefData>()
        .expect("root 2 points at B's RefData");
    let in_g: &SimpleData = in_b
        .target
        .to_value()
        .downcast_ref::<SimpleData>()
        .expect("B's value points into G");
    assert_eq!(in_g.count, 99, "the pointer from B into G resolves");
    drop(restored1);
    Ok(())
}

/// Without the index the same pointer is found by reading the rows between
/// the origin and the target, and only those.
#[cfg(fbcode_build)]
#[test]
fn test_pointer_into_indirect_dependency_walks_from_origin_without_the_index() -> crate::Result<()>
{
    use crate::pagable::starlark_partial_deser_stats;
    use crate::values::layout::heap::sealed::heap_key_index::StarlarkHeapKeyIndex;

    let (backing, handle, key) = page_out_indirect_graph("walked")?;
    handle
        .storage_context()
        .get::<StarlarkHeapKeyIndex>()
        .expect("page-out registers the index")
        .clear();

    let before = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    assert_points_into_g(&restored);
    let after = starlark_partial_deser_stats().expect("counters are on under cfg(test)");

    assert_eq!(
        after.heaps_loaded - before.heaps_loaded,
        3,
        "A's row is read with the root, B's to find G, and G's to claim into it"
    );
    assert_eq!(
        after.heaps_with_metadata - before.heaps_with_metadata,
        2,
        "A's and G's values are parsed; B's, which nothing points into, are not"
    );
    assert_eq!(
        restored
            .owner()
            .heap_arc()
            .retained_beyond_refs()
            .iter()
            .map(|heap| heap.name().map(ToString::to_string))
            .collect::<Vec<_>>(),
        vec![Some("TestHeapName(walked_g)".to_owned())],
        "the pointer's owner retains G regardless of how resolution found it"
    );
    Ok(())
}

/// A restored heap whose dependencies are still skeletons re-serializes by
/// reusing its stored representation, without reading them.
#[cfg(fbcode_build)]
#[test]
fn test_restored_heap_with_unread_dependencies_reserializes_by_key() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    use crate::pagable::starlark_partial_deser_stats;

    let dep = ErasingHeap::new();
    dep.alloc_simple(SimpleData {
        flag: true,
        count: 7,
    });
    let dep_ref = dep.into_ref_named(TestHeapName::heap_name("reser_dep"));

    let owner = ErasingHeap::new();
    owner.add_reference(dep_ref.owner());
    let root_fv = owner.alloc_simple(SimpleData {
        flag: false,
        count: 1,
    });
    let owner_ref = owner.into_ref_named(TestHeapName::heap_name("reser_owner"));
    // SAFETY: `owner_ref` owns the arena hosting `root_fv`.
    let ofv: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner_ref, root_fv) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key = ser_owned_frozen_value_into_storage(&backing, &ofv)?;
    drop(ofv);
    drop(dep_ref);

    let restored = deser_owned_frozen_from_storage(&backing, &handle, &key)?;
    let before = starlark_partial_deser_stats().expect("counters are on under cfg(test)");
    let key_again = ser_owned_frozen_value_into_storage(&backing, &restored)?;
    let after = starlark_partial_deser_stats().expect("counters are on under cfg(test)");

    assert_eq!(
        key_again, key,
        "the same rows are named, so the same key comes back"
    );
    assert_eq!(
        after.heaps_loaded, before.heaps_loaded,
        "re-serializing reads no row: the dependency stays a skeleton"
    );
    Ok(())
}

/// Serialize an OFV into the given shared storage, returning the top
/// `DataKey`. Companion to `deser_owned_frozen_value_from_storage`.
fn ser_owned_frozen_value_into_storage(
    backing: &pagable::storage::in_memory::InMemoryPagableStorage,
    owned: &OwnedFrozen<Value<'static>>,
) -> crate::Result<pagable::DataKey> {
    use pagable::storage::support::SerializerForPaging;

    let storage = backing.handle();
    let storage_ctx = storage.storage_context();

    let mut ser = SerializerForPaging::new(storage_ctx);
    owned
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();
    let finished = ArcSerCache::new();
    let key = storage
        .page_out_item(data, arcs, &finished, storage_ctx)
        .map_err(crate::Error::new_other)?;
    Ok(key)
}

/// Deserialize an OFV from the given shared storage by its top `DataKey`.
fn deser_owned_frozen_from_storage(
    backing: &pagable::storage::in_memory::InMemoryPagableStorage,
    handle: &pagable::storage::handle::PagableStorageHandle,
    top_key: &pagable::DataKey,
) -> crate::Result<OwnedFrozen<Value<'static>>> {
    Ok(deser_owned_frozen_value_with_scope_from_storage(backing, handle, top_key)?.0)
}

fn deser_owned_frozen_value_with_scope_from_storage(
    backing: &pagable::storage::in_memory::InMemoryPagableStorage,
    handle: &pagable::storage::handle::PagableStorageHandle,
    top_key: &pagable::DataKey,
) -> crate::Result<(OwnedFrozen<Value<'static>>, Arc<StarlarkDeserScope>)> {
    use std::any::TypeId;

    use pagable::PagableDeserialize;
    use pagable::PagableDeserializer;

    let top_data = backing
        .handle()
        .fetch_arc_or_data_blocking(&TypeId::of::<()>(), top_key)
        .map_err(crate::Error::new_other)?
        .right()
        .expect("top-level key should return data, not a cached arc");
    let mut de = handle.root_deserializer(*top_key, &top_data);
    let value =
        <OwnedFrozen<Value>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let scope = de
        .storage_context()
        .get::<StarlarkDeserScope>()
        .expect("OwnedFrozen page-in should initialize a Starlark scope");
    Ok((value, scope))
}

/// Two heaps sealed under one name, read back in one page-in. Each pointer
/// resolves into the heap it was written against, and binding the second is
/// not a collision with the first.
#[test]
fn test_two_heaps_sealed_under_one_name_page_in_together() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    let make_root = |count| {
        let heap = ErasingHeap::new();
        let value = heap.alloc_simple(SimpleData { flag: true, count });
        let owner = heap.into_ref_named(TestHeapName::heap_name("same_name_together"));
        // SAFETY: `owner` owns the arena hosting `value`.
        unsafe { OwnedFrozen::<Value>::from_erased(owner, value) }
    };
    let root0 = make_root(111);
    let root1 = make_root(222);
    assert_eq!(
        TestHeapName::name_of(root0.name().unwrap()),
        TestHeapName::name_of(root1.name().unwrap()),
        "the two heaps share a name"
    );

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let mut ser = SerializerForPaging::new(storage.storage_context());
    root0
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    root1
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();
    let key = storage
        .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
        .map_err(crate::Error::new_other)?;
    storage.flush().map_err(crate::Error::new_other)?;
    drop(root0);
    drop(root1);
    storage.arc_cache().clear();

    let handle = PagableStorageHandle::new(storage.clone());
    let data = storage
        .fetch_data_blocking(&key)
        .map_err(crate::Error::new_other)?;
    let mut de = handle.root_deserializer(key, &data);
    let restored0 =
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let restored1 =
        OwnedFrozen::<Value>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let count = |restored: &OwnedFrozen<Value<'static>>| {
        restored
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .expect("each root restores its own SimpleData")
            .count
    };
    assert_eq!(count(&restored0), 111);
    assert_eq!(count(&restored1), 222);
    assert_ne!(restored0.owner(), restored1.owner());
    Ok(())
}

/// Two independently stored roots may own different live heaps with the same
/// logical name, each with its own identity.
#[test]
fn test_same_name_heaps_serialize_independently_in_shared_session() {
    same_name_heaps_serialize_independently_in_shared_session_impl()
        .expect("same-name heaps should serialize independently");
}

fn same_name_heaps_serialize_independently_in_shared_session_impl() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    let heap0 = ErasingHeap::new();
    let value0 = heap0.alloc_simple(SimpleData {
        flag: true,
        count: 111,
    });
    let owner0 = heap0.into_ref_named(TestHeapName::heap_name("same_name_independent_roots"));
    // SAFETY: `owner0` owns the arena hosting `value0`.
    let root0: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner0, value0) };

    let heap1 = ErasingHeap::new();
    let value1 = heap1.alloc_simple(SimpleData {
        flag: true,
        count: 111,
    });
    let owner1 = heap1.into_ref_named(TestHeapName::heap_name("same_name_independent_roots"));
    // SAFETY: `owner1` owns the arena hosting `value1`.
    let root1: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(owner1, value1) };

    assert_eq!(
        TestHeapName::name_of(root0.name().unwrap()),
        TestHeapName::name_of(root1.name().unwrap()),
        "the two heaps share a name"
    );
    assert_ne!(
        root0.owner().heap_arc().heap_ref_id(),
        root1.owner().heap_arc().heap_ref_id(),
        "same name, different seal, different identity"
    );
    assert_ne!(root0.owner(), root1.owner());

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());

    // Keep both roots alive so both exact heaps are resident in the shared
    // Starlark serialization state when the second root is serialized.
    let key0 = ser_owned_frozen_value_into_storage(&backing, &root0)?;
    let key1 = ser_owned_frozen_value_into_storage(&backing, &root1)?;
    assert_ne!(
        key0, key1,
        "the serialized heap nonce must distinguish otherwise identical heaps"
    );

    drop(root0);
    drop(root1);

    let restored1 = deser_owned_frozen_from_storage(&backing, &handle, &key1)?;
    let data1 = restored1
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .expect("second root should restore its own SimpleData");
    assert!(data1.flag);
    assert_eq!(data1.count, 111);

    Ok(())
}

/// A cached owner heap must bring its transitive heap graph into each root
/// page-in that reuses it, because an `OwnedFrozen` may point into one of
/// those dependency heaps rather than the owner heap itself.
#[test]
fn test_cached_owner_registers_transitive_heap_in_later_page_in() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    // Stored graph: the `OwnedFrozen` retains P, while its value physically
    // lives in L. P's reference to L makes this a valid ownership relationship.
    let leaf_heap = ErasingHeap::new();
    let leaf_value = leaf_heap.alloc_simple(SimpleData {
        flag: true,
        count: 42,
    });
    let leaf_ref = leaf_heap.into_ref_named(TestHeapName::heap_name("cached_scope_leaf"));

    let owner_heap = ErasingHeap::new();
    owner_heap.add_reference(leaf_ref.owner());
    let owner_ref = owner_heap.into_ref_named(TestHeapName::heap_name("cached_scope_owner"));
    // SAFETY: `owner_ref` retains `leaf_ref`, which owns `leaf_value`.
    let root = unsafe { OwnedFrozen::from_erased(owner_ref, leaf_value) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let root_key = ser_owned_frozen_value_into_storage(&backing, &root)?;
    drop(root);
    drop(leaf_ref);

    // The first root page-in misses the Arc cache and reconstructs both P and L.
    let first = deser_owned_frozen_from_storage(&backing, &handle, &root_key)?;
    assert_eq!(
        first
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .expect("the first root should resolve through L")
            .count,
        42,
    );

    // The second root page-in reuses cached P. It must register P -> L before
    // resolving the value's `(HeapRefId(L), value_index)` wire pointer.
    let (second, second_scope) =
        deser_owned_frozen_value_with_scope_from_storage(&backing, &handle, &root_key)?;
    assert_eq!(
        first.owner(),
        second.owner(),
        "the owner Arc should be reused"
    );
    let second_leaf_ref = second
        .owner()
        .refs()
        .next()
        .expect("P should retain L after page-in");
    let leaf_id = second_leaf_ref
        .heap_arc()
        .heap_ref_id()
        .expect("the deserialized leaf heap should remain named");
    assert_eq!(
        second_scope.get_heap(&leaf_id).as_ref(),
        Some(second_leaf_ref.heap_arc()),
        "the cache hit must bind transitive heap L for the second root",
    );
    assert_eq!(
        second
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .expect("the cached owner should still resolve through L")
            .count,
        42,
    );

    Ok(())
}

/// X and Y point into the same heap. After X is paged out and its owner is
/// dropped, Y keeps that heap resident. Paging X back in must reuse Y's heap
/// and X's original allocation instead of reconstructing a duplicate heap.
#[test]
fn test_page_in_reuses_resident_shared_heap() -> crate::Result<()> {
    use dupe::Dupe;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    let heap = ErasingHeap::new();
    let x = heap.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    let y = heap.alloc_simple(SimpleData {
        flag: false,
        count: 2,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("resident_shared"));
    let x_ptr = x.ptr_value().ptr_value_untagged();

    // SAFETY: both `OwnedFrozen`s keep their shared heap alive.
    let owned_x: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(heap_ref.dupe(), x) };
    let owned_y: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(heap_ref, y) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key_x = ser_owned_frozen_value_into_storage(&backing, &owned_x)?;
    drop(owned_x);

    // `owned_y` is now the only application-owned reference keeping the
    // original heap resident while X is paged back in.
    let (restored_x, scope) =
        deser_owned_frozen_value_with_scope_from_storage(&backing, &handle, &key_x)?;
    let heap_id = restored_x
        .owner()
        .heap_arc()
        .heap_ref_id()
        .expect("restored heap should retain its name");
    assert_eq!(
        restored_x.owner(),
        owned_y.owner(),
        "page-in should reuse the original heap kept alive by Y",
    );
    assert_eq!(
        scope.get_heap(&heap_id).as_ref(),
        Some(restored_x.heap_arc()),
        "the current root scope should bind the reused native heap",
    );
    assert_eq!(
        restored_x.as_ref().value().ptr_value().ptr_value_untagged(),
        x_ptr,
        "X should point to its original allocation in the resident heap",
    );
    let restored_data = restored_x
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .expect("restored X should be SimpleData");
    assert_eq!(restored_data.count, 1);

    Ok(())
}

/// An old serialized parent must resolve dependency indices through the dependency's
/// original recipe, even after that dependency is paged in and reserialized.
///
/// ```text
/// Initial resident heap graph:
///
///   dependency H0 (heap ID D)          old parent P0
///   +-------------------------+        +-------------------------+
///   | old index 0: target     |<--+----| parent_root.target      |
///   |   SimpleData(42)        |   |    |   RefData(9)            |
///   |                         |   |    +-------------------------+
///   | old index 1: demand_root|   |
///   |   RefData(7) -----------+---+
///   +-------------------------+   
///
///   parent_key stores P0 and encodes parent_root.target as (D, old index 0).
///   demand_root_key stores (D, old index 1).
///
/// After paging in demand_root_key, the replacement heap H1 has the same heap ID D,
/// but lazy materialization allocates in demand order:
///
///   H1 physical order                     H0 recipe mapping retained by H1
///   +-------------------------+            +----------------------------+
///   | physical 0: demand_root |            | old index 0 -> physical 1  |
///   | physical 1: target      |            | old index 1 -> physical 0  |
///   +-------------------------+            +----------------------------+
///
/// A newly serialized parent C0 references H1. Serializing C0 registers H1's
/// physical order as the resident mapping: (D, 0) -> demand_root and
/// (D, 1) -> target. C0 must nevertheless encode its pointer to demand_root as
/// the original recipe index (D, 1), not its current physical index (D, 0).
///
/// Conversely, when parent_key is restored, its old (D, 0) must use the recipe
/// mapping and resolve to target. Using one index space for either direction
/// corrupts one of these two parents.
/// ```
#[test]
fn test_resident_heap_reuse_preserves_old_recipe_value_indices() {
    resident_heap_reuse_preserves_old_recipe_value_indices_impl()
        .expect("recipe-index regression setup should succeed");
}

fn resident_heap_reuse_preserves_old_recipe_value_indices_impl() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    // Stage 1: Build H0 and P0 from the initial graph above.
    let dep_heap = ErasingHeap::new();
    let target = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 42,
    });
    let demand_root = dep_heap.alloc_ref_data(7, target);
    let dep_heap_ref = dep_heap.into_ref_named(TestHeapName::heap_name("resident_old_recipe_dep"));

    let parent_heap = ErasingHeap::new();
    parent_heap.add_reference(dep_heap_ref.owner());
    let parent_root = parent_heap.alloc_ref_data(9, target);
    let parent_heap_ref =
        parent_heap.into_ref_named(TestHeapName::heap_name("resident_old_recipe_parent"));

    // SAFETY: each owner keeps the heap containing its value alive.
    let owned_parent = unsafe { OwnedFrozen::from_erased(parent_heap_ref, parent_root) };
    let owned_demand_root = unsafe { OwnedFrozen::from_erased(dep_heap_ref.clone(), demand_root) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let parent_key = ser_owned_frozen_value_into_storage(&backing, &owned_parent)?;
    let demand_root_key = ser_owned_frozen_value_into_storage(&backing, &owned_demand_root)?;

    // Stage 2: Persist both roots, then remove every owner of H0 and P0.
    drop(owned_parent);
    drop(owned_demand_root);
    drop(dep_heap_ref);

    // Stage 3: Restore H1 from demand_root_key and verify its new physical order.
    // Its recipe still maps each old index to the correct physical address.
    let restored_demand_root =
        deser_owned_frozen_from_storage(&backing, &handle, &demand_root_key)?;
    let restored_headers = restored_demand_root
        .owner()
        .heap_arc()
        .collect_undrop_headers_ordered();
    assert_eq!(restored_headers.len(), 2);
    assert!(
        restored_headers[0]
            .unpack()
            .downcast_ref::<RefData>()
            .is_some(),
        "the demand root should materialize before its target"
    );
    assert!(
        restored_headers[1]
            .unpack()
            .downcast_ref::<SimpleData>()
            .is_some(),
        "the target should materialize after the demand root"
    );

    // Stage 4: Build C0 with a cross-heap pointer into H1. Paging out this new
    // owner registers H1's current physical order in the resident index.
    let new_parent_heap = ErasingHeap::new();
    // `add_to_frozen_heap` adds H1 as a dependency of `new_parent_heap`.
    let restored_root =
        new_parent_heap.with(|heap| erase(restored_demand_root.as_ref().add_to_frozen_heap(heap)));
    let new_parent_root = new_parent_heap.alloc_ref_data(11, restored_root);
    let new_parent_heap_ref =
        new_parent_heap.into_ref_named(TestHeapName::heap_name("resident_old_recipe_new_parent"));
    // SAFETY: `new_parent_heap_ref` keeps C0 and its H1 dependency alive.
    let owned_new_parent =
        unsafe { OwnedFrozen::from_erased(new_parent_heap_ref, new_parent_root) };
    let new_parent_key = ser_owned_frozen_value_into_storage(&backing, &owned_new_parent)?;
    drop(owned_new_parent);

    // Stage 5: Restore C0. Its pointer to demand_root must have been serialized
    // using H1's original recipe index, not H1's current physical arena index.
    let restored_new_parent = deser_owned_frozen_from_storage(&backing, &handle, &new_parent_key)?;
    let restored_new_parent = restored_new_parent
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("the new parent root should remain RefData");
    let restored_demand_root = restored_new_parent
        .target
        .downcast_ref::<RefData>()
        .unwrap_or_else(|| {
            let wrong_target = restored_new_parent
                .target
                .downcast_ref::<SimpleData>()
                .expect("the new parent target should be RefData, not another value type");
            panic!(
                "recipe index mapping is incorrect: the new parent target resolved to SimpleData({})",
                wrong_target.count
            );
        });
    assert_eq!(restored_demand_root.label, 7);
    let restored_demand_target = restored_demand_root
        .target
        .downcast_ref::<SimpleData>()
        .expect("the demand root should retain its target");
    assert_eq!(restored_demand_target.count, 42);

    // Stage 6: Restore P0. Its old (D, 0) reference must follow H1's recipe map,
    // even though the resident map now gives (D, 0) a different meaning.
    let restored_parent = deser_owned_frozen_from_storage(&backing, &handle, &parent_key)?;
    let restored_parent = restored_parent
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("the old parent root should remain RefData");
    let restored_target = restored_parent
        .target
        .downcast_ref::<SimpleData>()
        .unwrap_or_else(|| {
            let wrong_target = restored_parent
                .target
                .downcast_ref::<RefData>()
                .expect("the old parent target should be SimpleData, not another value type");
            panic!(
                "recipe index mapping is incorrect: the old parent target resolved to the demand root RefData({})",
                wrong_target.label
            );
        });
    assert!(restored_target.flag);
    assert_eq!(restored_target.count, 42);

    Ok(())
}

/// Multi-heap, multi-`OwnedFrozen` partial-deser, with incremental
/// state checks between deserialization steps:
///
/// Setup
///   dep_heap: SimpleData_d1 (count=1), SimpleData_d2 (count=2)
///   heap_a refs dep_heap; values: RefData_a → d1
///   heap_b refs dep_heap; values: RefData_b → d2
///   OFV1 = RefData_a
///   OFV2 = RefData_b
///   OFV3 = SimpleData_d1 directly (subset of what OFV1 already materializes)
///
/// Step 1 — deser OFV1
///   Expected materialization: heap_a has [RefData_a]; dep_heap has [d1].
///   d2 is allocated on the wire but not reachable from OFV1 → not materialized.
///
/// Step 2 — deser OFV2 (same shared storage / session)
///   Expected materialization: heap_b has [RefData_b]; dep_heap now has [d1, d2].
///   d1 was already materialized in step 1 → its allocation is reused (fast
///   path), no second copy. heap_b is added via the storage's arc cache (or
///   freshly fetched, depending on order).
///
/// Step 3 — deser OFV3 (subset: just SimpleData_d1, no extra work)
///   Expected materialization: nothing new. d1 is already materialized in
///   dep_heap; OFV3 just produces an `OwnedFrozen` pointing at the
///   already-allocated slot.
#[test]
fn test_multi_ofv_shared_heap_incremental_partial_deser() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    // ---- 1. Build the source heaps. ----
    let dep_heap = ErasingHeap::new();
    let d1_fv = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    let d2_fv = dep_heap.alloc_simple(SimpleData {
        flag: false,
        count: 2,
    });
    let dep_heap_ref = dep_heap.into_ref_named(TestHeapName::heap_name("incr_dep"));

    let heap_a = ErasingHeap::new();
    heap_a.add_reference(dep_heap_ref.owner());
    let a_fv = heap_a.alloc_ref_data(11, d1_fv);
    let heap_a_ref = heap_a.into_ref_named(TestHeapName::heap_name("incr_a"));

    let heap_b = ErasingHeap::new();
    heap_b.add_reference(dep_heap_ref.owner());
    let b_fv = heap_b.alloc_ref_data(22, d2_fv);
    let heap_b_ref = heap_b.into_ref_named(TestHeapName::heap_name("incr_b"));

    // SAFETY: each heap_ref keeps its arena alive.
    let ofv1 = unsafe { OwnedFrozen::from_erased(heap_a_ref, a_fv) };
    let ofv2 = unsafe { OwnedFrozen::from_erased(heap_b_ref.clone(), b_fv) };
    let ofv3 = unsafe { OwnedFrozen::from_erased(dep_heap_ref.clone(), d1_fv) };

    // ---- 2. Serialize all three into a SHARED storage. ----
    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let key1 = ser_owned_frozen_value_into_storage(&backing, &ofv1)?;
    let key2 = ser_owned_frozen_value_into_storage(&backing, &ofv2)?;
    let key3 = ser_owned_frozen_value_into_storage(&backing, &ofv3)?;

    // Drop the original OFVs so the only path to the values is through deser.
    drop(ofv1);
    drop(ofv2);
    drop(ofv3);
    drop(dep_heap_ref);
    drop(heap_b_ref);

    // ---- 3. Step 1: deser OFV1, then inspect the heap state. ----
    let restored_ofv1 = deser_owned_frozen_from_storage(&backing, &handle, &key1)?;
    let a_data: &RefData = restored_ofv1
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("ofv1 root is RefData");
    assert_eq!(a_data.label, 11);

    // heap_a has exactly one materialized value (the root).
    let a_undrop = restored_ofv1
        .owner()
        .heap_arc()
        .collect_undrop_headers_ordered();
    assert_eq!(
        a_undrop.len(),
        1,
        "step1: heap_a should hold only RefData_a"
    );

    // dep_heap (reachable via heap_a.refs) has exactly one materialized
    // value: d1 (the target of the root). d2 is unreached.
    let dep_after_1 = {
        let refs: Vec<_> = restored_ofv1.owner().refs().collect();
        assert_eq!(refs.len(), 1, "step1: heap_a refs only dep_heap");
        refs[0]
    };
    let dep_undrop_after_1 = dep_after_1.heap_arc().collect_undrop_headers_ordered();
    assert_eq!(
        dep_undrop_after_1.len(),
        1,
        "step1: dep_heap should hold only d1"
    );
    let d1_after_1: &SimpleData = dep_undrop_after_1[0].unpack().downcast_ref().unwrap();
    assert_eq!(d1_after_1.count, 1);

    let d1_ptr_after_1 = dep_undrop_after_1[0] as *const _ as usize;

    // ---- 4. Step 2: deser OFV2 from the SAME storage. ----
    let restored_ofv2 = deser_owned_frozen_from_storage(&backing, &handle, &key2)?;
    let b_data: &RefData = restored_ofv2
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("ofv2 root is RefData");
    assert_eq!(b_data.label, 22);

    // heap_b has only its root.
    let b_undrop = restored_ofv2
        .owner()
        .heap_arc()
        .collect_undrop_headers_ordered();
    assert_eq!(
        b_undrop.len(),
        1,
        "step2: heap_b should hold only RefData_b"
    );

    // The dep_heap arc returned to OFV2 is the SAME allocation as OFV1's
    // (the storage's arc cache deduplicates `Arc<FrozenFrozenHeap>` by
    // `DataKey`; `OwnedFrozen<()>` equality is `Arc::ptr_eq`).
    let dep_after_2 = {
        let refs: Vec<_> = restored_ofv2.owner().refs().collect();
        assert_eq!(refs.len(), 1);
        refs[0]
    };
    assert_eq!(
        dep_after_1, dep_after_2,
        "step2: dep_heap must be the same Arc allocation as in step1"
    );

    // Now dep_heap holds [d1, d2] — d2 was just materialized by OFV2.
    let dep_undrop_after_2 = dep_after_2.heap_arc().collect_undrop_headers_ordered();
    assert_eq!(
        dep_undrop_after_2.len(),
        2,
        "step2: dep_heap should now hold d1 (from step1) and d2 (new from step2)"
    );
    // d1 must be at the SAME address as before (fast-path reuse, no re-alloc).
    let d1_ptr_after_2 = dep_undrop_after_2[0] as *const _ as usize;
    assert_eq!(
        d1_ptr_after_1, d1_ptr_after_2,
        "step2: d1 must remain at the same allocation (fast path hit, no re-materialize)"
    );

    // OFV1's target still resolves to the same d1 address.
    assert_eq!(
        a_data.target.ptr_value().ptr_value_untagged(),
        d1_ptr_after_1
    );
    // OFV2's target resolves to d2 (the newly-materialized slot).
    let b_target: &SimpleData = b_data
        .target
        .downcast_ref::<SimpleData>()
        .expect("b.target is SimpleData");
    assert_eq!(b_target.count, 2);

    // ---- 5. Step 3: deser OFV3 — strict subset of OFV1's materialization. ----
    let restored_ofv3 = deser_owned_frozen_from_storage(&backing, &handle, &key3)?;
    // OFV3's value points directly at d1 in dep_heap.
    let d3_target: &SimpleData = restored_ofv3
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .expect("ofv3 root is SimpleData");
    assert_eq!(d3_target.count, 1);

    // OFV3 owns the dep_heap arc directly; arc-cache dedup means it's the
    // same Arc allocation.
    let dep_after_3 = restored_ofv3.owner();
    assert_eq!(
        dep_after_1, dep_after_3,
        "step3: dep_heap must remain the same Arc allocation"
    );
    let dep_undrop_after_3 = dep_after_3.heap_arc().collect_undrop_headers_ordered();
    assert_eq!(
        dep_undrop_after_3.len(),
        2,
        "step3: no new materializations — still just [d1, d2]"
    );
    // OFV3's resolved address matches d1.
    assert_eq!(
        restored_ofv3
            .as_ref()
            .value()
            .ptr_value()
            .ptr_value_untagged(),
        d1_ptr_after_1
    );

    Ok(())
}

/// A restored heap whose serialization index is already registered must mark
/// that index dirty when another value is materialized lazily.
#[test]
fn test_restored_heap_refreshes_index_after_later_materialization() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    fn serialize_parent(
        backing: &InMemoryPagableStorage,
        target: &OwnedFrozen<Value<'static>>,
        heap_name: &'static str,
    ) -> crate::Result<pagable::DataKey> {
        let heap = ErasingHeap::new();
        // This adds the target's owning heap to `heap.refs`.
        let target = heap.with(|h| erase(target.as_ref().add_to_frozen_heap(h)));
        let root = heap.alloc_ref_data(9, target);
        let heap_ref = heap.into_ref_named(TestHeapName::heap_name(heap_name));
        // SAFETY: `heap_ref` owns the arena containing `root`.
        let root: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(heap_ref, root) };
        ser_owned_frozen_value_into_storage(backing, &root)
    }

    let heap = ErasingHeap::new();
    let first = heap.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    let second = heap.alloc_simple(SimpleData {
        flag: true,
        count: 2,
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("refresh_later_dep"));
    // SAFETY: both values belong to `heap_ref`.
    let first = unsafe { OwnedFrozen::from_erased(heap_ref.clone(), first) };
    let second = unsafe { OwnedFrozen::from_erased(heap_ref, second) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let first_key = ser_owned_frozen_value_into_storage(&backing, &first)?;
    let second_key = ser_owned_frozen_value_into_storage(&backing, &second)?;
    drop(first);
    drop(second);

    let restored_first = deser_owned_frozen_from_storage(&backing, &handle, &first_key)?;
    let _first_parent_key = serialize_parent(&backing, &restored_first, "refresh_later_parent_1")?;
    let ser_state = backing
        .storage_context()
        .get::<StarlarkSerState>()
        .expect("serializing the parent should initialize Starlark serialization state");
    let first_ptr = restored_first
        .as_ref()
        .value()
        .ptr_value()
        .ptr_value_untagged();
    let first_entry = ser_state
        .chunk_entry_identity_for_ptr(first_ptr)
        .expect("the first materialized value should be indexed");

    let _clean_parent_key = serialize_parent(&backing, &restored_first, "refresh_clean_parent")?;
    assert_eq!(
        ser_state.chunk_entry_identity_for_ptr(first_ptr),
        Some(first_entry),
        "a clean restored heap should reuse its registered chunk index",
    );

    let restored_second = deser_owned_frozen_from_storage(&backing, &handle, &second_key)?;
    assert_eq!(
        restored_first.owner(),
        restored_second.owner(),
        "both values should materialize in the same restored heap",
    );
    let second_parent_key = serialize_parent(&backing, &restored_second, "refresh_later_parent_2")?;
    assert_ne!(
        ser_state.chunk_entry_identity_for_ptr(first_ptr),
        Some(first_entry),
        "materializing another value should refresh the restored heap's chunk index",
    );

    let restored_parent = deser_owned_frozen_from_storage(&backing, &handle, &second_parent_key)?;
    let restored_parent = restored_parent
        .as_ref()
        .value()
        .downcast_ref::<RefData>()
        .expect("restored parent should remain RefData");
    let restored_second = restored_parent
        .target
        .downcast_ref::<SimpleData>()
        .expect("restored parent should point to the second lazy value");
    assert_eq!(restored_second.count, 2);

    Ok(())
}

/// A clean registered owner can hide a restored transitive dependency that became dirty
/// after the ownership graph's chunk indices were first registered.
#[test]
fn test_pointer_lookup_repairs_dirty_transitive_restored_dependency() -> crate::Result<()> {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;

    let dep_heap = ErasingHeap::new();
    let first = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 1,
    });
    let second = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 2,
    });
    let third = dep_heap.alloc_simple(SimpleData {
        flag: true,
        count: 3,
    });
    let dep_heap_ref = dep_heap.into_ref_named(TestHeapName::heap_name("repair_dirty_dep"));
    // SAFETY: all values belong to `dep_heap_ref`.
    let first = unsafe { OwnedFrozen::from_erased(dep_heap_ref.clone(), first) };
    let second: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(dep_heap_ref, second) };
    let third: OwnedFrozen<Value> =
        unsafe { OwnedFrozen::unchecked_new(second.owner().to_owned(), third.to_value()) };

    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let first_key = ser_owned_frozen_value_into_storage(&backing, &first)?;
    let second_key = ser_owned_frozen_value_into_storage(&backing, &second)?;
    let third_key = ser_owned_frozen_value_into_storage(&backing, &third)?;
    drop(first);
    drop(second);
    drop(third);

    let restored_first = deser_owned_frozen_from_storage(&backing, &handle, &first_key)?;
    let middle_heap = ErasingHeap::new();
    let first_in_middle =
        middle_heap.with(|heap| erase(restored_first.as_ref().add_to_frozen_heap(heap)));
    let middle_root = middle_heap.alloc_ref_data(2, first_in_middle);
    let middle_heap_ref = middle_heap.into_ref_named(TestHeapName::heap_name("repair_middle"));
    // SAFETY: `middle_heap_ref` owns `middle_root`.
    let middle_root: OwnedFrozen<Value> =
        unsafe { OwnedFrozen::from_erased(middle_heap_ref.clone(), middle_root) };

    let owner_heap = ErasingHeap::new();
    let middle_in_owner =
        owner_heap.with(|heap| erase(middle_root.as_ref().add_to_frozen_heap(heap)));
    let owner_root = owner_heap.alloc_ref_data(1, middle_in_owner);
    let owner_heap_ref = owner_heap.into_ref_named(TestHeapName::heap_name("repair_owner"));
    // SAFETY: `owner_heap_ref` owns `owner_root`.
    let owner_root: OwnedFrozen<Value> =
        unsafe { OwnedFrozen::from_erased(owner_heap_ref.clone(), owner_root) };
    assert_eq!(
        owner_heap_ref.heap_arc().refs_slice(),
        std::slice::from_ref(&middle_heap_ref),
        "the owner should reference only the intermediate heap directly",
    );
    assert_eq!(
        middle_heap_ref.heap_arc().refs_slice(),
        std::slice::from_ref(&restored_first.owner().to_owned()),
        "the restored dependency should be reachable only through the intermediate heap",
    );
    let _owner_key = ser_owned_frozen_value_into_storage(&backing, &owner_root)?;

    let ser_state = backing
        .storage_context()
        .get::<StarlarkSerState>()
        .expect("serializing the owner should initialize Starlark serialization state");
    let restored_second = deser_owned_frozen_from_storage(&backing, &handle, &second_key)?;
    assert_eq!(
        restored_first.owner(),
        restored_second.owner(),
        "both values should materialize in the same restored dependency heap",
    );
    let second_value = restored_second.as_ref().value();
    let second_ptr = second_value.get_ref().value.ptr as usize;
    assert!(
        ser_state.lookup_ptr(second_ptr).is_none(),
        "the clean owner fast path should leave the newly materialized dependency slot unindexed",
    );

    // The owner keeps the dependency alive transitively, but its clean registration
    // prevents the normal recursive ensure path from reaching the dirty dependency.
    // SAFETY: `owner_heap_ref` references the heap containing `second_value`.
    let through_clean_owner: OwnedFrozen<Value> =
        unsafe { OwnedFrozen::unchecked_new(owner_heap_ref.clone(), second_value) };
    let repaired_key = ser_owned_frozen_value_into_storage(&backing, &through_clean_owner)?;

    let restored = deser_owned_frozen_from_storage(&backing, &handle, &repaired_key)?;
    let restored = restored
        .as_ref()
        .value()
        .downcast_ref::<SimpleData>()
        .expect("repaired pointer should round-trip as SimpleData");
    assert_eq!(restored.count, 2);

    let restored_third = deser_owned_frozen_from_storage(&backing, &handle, &third_key)?;
    let third_value = restored_third.as_ref().value();
    let third_ptr = third_value.get_ref().value.ptr as usize;
    assert!(
        ser_state.lookup_ptr(third_ptr).is_none(),
        "materializing another dependency slot should make its pointer require repair",
    );

    Ok(())
}

/// Cross-thread cycle test: two values on the same heap reference each other.
/// Two threads simultaneously deserialize one value each. Without cross-thread
/// cycle detection, this deadlocks (thread A waits for B's value, B waits for A's).
/// Cycle detection lets construction use unfinished references; readiness
/// tracking prevents owning results from escaping before the cycle is complete.
#[test]
fn test_cross_thread_cycle_does_not_deadlock() {
    use std::sync::Barrier;
    use std::time::Duration;

    use pagable::PagableDeserialize;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;

    let code = r#"
a = []
b = []
a.append(b)
b.append(a)
"#;

    let ast = AstModule::parse("cross_thread.star", code.to_owned(), &Dialect::Extended).unwrap();
    let globals = GlobalsBuilder::new().build_named(GlobalFrozenHeapName {
        name: CROSS_THREAD_GLOBALS_HEAP_NAME,
    });
    let frozen_module = Module::with_temp_heap(|module| {
        {
            let mut eval = Evaluator::new(&module);
            eval.eval_module(ast, &globals).unwrap();
        }
        module.freeze_named(TestHeapName::heap_name("cross_thread_cycle"))
    })
    .unwrap();

    let ofv_a = frozen_module.get("a").unwrap();
    let ofv_b = frozen_module.get("b").unwrap();

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let storage_ctx = storage.storage_context();

    // Serialize both `OwnedFrozen`s.
    let ser_one = |ofv: &OwnedFrozen<Value<'static>>| -> crate::Result<pagable::DataKey> {
        let mut ser = SerializerForPaging::new(storage_ctx);
        ofv.pagable_serialize(&mut ser)
            .map_err(crate::Error::new_other)?;
        let (data, arcs) = ser.finish();
        let finished = ArcSerCache::new();
        storage
            .page_out_item(data, arcs, &finished, storage_ctx)
            .map_err(crate::Error::new_other)
    };
    let key_a = ser_one(&ofv_a).unwrap();
    let key_b = ser_one(&ofv_b).unwrap();

    let handle = Arc::new(PagableStorageHandle::new(storage.clone()));
    let barrier = Arc::new(Barrier::new(2));

    let deser_one = |key: pagable::DataKey,
                     handle: Arc<PagableStorageHandle>,
                     barrier: Arc<Barrier>,
                     storage: Arc<dyn pagable::storage::traits::PagableStorage>|
     -> std::thread::JoinHandle<crate::Result<()>> {
        std::thread::spawn(move || {
            let top_data = storage
                .fetch_data_blocking(&key)
                .map_err(crate::Error::new_other)?;

            // Synchronize so both threads enter deserialization at the same time.
            barrier.wait();

            let mut de = handle.root_deserializer(key, &top_data);
            let _restored = <OwnedFrozen<Value>>::pagable_deserialize(&mut de)
                .map_err(crate::Error::new_other)?;
            Ok(())
        })
    };

    let h_a = deser_one(key_a, handle.clone(), barrier.clone(), storage.clone());
    let h_b = deser_one(key_b, handle.clone(), barrier.clone(), storage.clone());

    // Use recv_timeout to detect deadlock.
    let (tx, rx) = std::sync::mpsc::channel();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        let r_a = h_a.join().expect("thread A panicked");
        tx.send(r_a).ok();
    });
    std::thread::spawn(move || {
        let r_b = h_b.join().expect("thread B panicked");
        tx2.send(r_b).ok();
    });

    for _ in 0..2 {
        rx.recv_timeout(Duration::from_secs(10))
            .expect("cross-thread cycle test deadlocked")
            .unwrap_or_else(|e| panic!("deser thread failed: {:#}", e));
    }
}

// ---- Ser/deser micro-benchmarks ----

struct BenchResult {
    label: &'static str,
    count: usize,
    ser_us: u128,
    deser_us: u128,
    bytes: usize,
}

impl BenchResult {
    fn print(&self) {
        eprintln!(
            "  {:<30} n={:<6} ser={:>8}µs ({:>6}µs/v)  deser={:>8}µs ({:>6}µs/v)  bytes={:>8} ({:>4}B/v)",
            self.label,
            self.count,
            self.ser_us,
            self.ser_us / self.count as u128,
            self.deser_us,
            self.deser_us / self.count as u128,
            self.bytes,
            self.bytes / self.count,
        );
    }
}

fn bench_owned_frozen_value_round_trip(
    label: &'static str,
    heap_ref: OwnedFrozen<()>,
    root: Value<'static>,
    count: usize,
) -> crate::Result<BenchResult> {
    use std::any::TypeId;
    use std::time::Instant;

    use pagable::PagableDeserialize;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    let owned: OwnedFrozen<Value> = unsafe { OwnedFrozen::from_erased(heap_ref, root) };

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let handle = PagableStorageHandle::new(storage.clone());
    let storage_ctx = storage.storage_context();

    let ser_start = Instant::now();
    let mut ser = SerializerForPaging::new(storage_ctx);
    owned
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();
    let ser_us = ser_start.elapsed().as_micros();
    let bytes = data.len();

    let top_key = {
        let finished = ArcSerCache::new();
        storage
            .page_out_item(data, arcs, &finished, storage_ctx)
            .map_err(crate::Error::new_other)?
    };

    let top_data = storage
        .fetch_arc_or_data_blocking(&TypeId::of::<()>(), &top_key)
        .map_err(crate::Error::new_other)?
        .right()
        .expect("should be data");

    let deser_start = Instant::now();
    let mut de = handle.root_deserializer(top_key, &top_data);
    let _restored =
        <OwnedFrozen<Value>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
    let deser_us = deser_start.elapsed().as_micros();

    Ok(BenchResult {
        label,
        count,
        ser_us,
        deser_us,
        bytes,
    })
}

#[test]
fn bench_pagable_ser_deser_by_value_type() -> crate::Result<()> {
    eprintln!();
    eprintln!("=== Pagable ser/deser micro-benchmark ===");

    // SimpleData: primitive fields
    {
        let n = 5000;
        let heap = ErasingHeap::new();
        let mut root = Value::new_none();
        for i in 0..n {
            root = heap.alloc_simple(SimpleData {
                flag: i % 2 == 0,
                count: i,
            });
        }
        let heap_ref = heap.into_ref_named(TestHeapName::heap_name("bench_simple"));
        bench_owned_frozen_value_round_trip("SimpleData(bool,usize)", heap_ref, root, n)?.print();
    }

    // HeapData: Vec, String, Box
    {
        let n = 5000;
        let heap = ErasingHeap::new();
        let mut root = Value::new_none();
        for i in 0..n {
            root = heap.alloc_simple(HeapData {
                items: vec![i as u32; 10],
                label: format!("label_{i}"),
                boxed: Box::new(i as i64 * 100),
            });
        }
        let heap_ref = heap.into_ref_named(TestHeapName::heap_name("bench_heap_data"));
        bench_owned_frozen_value_round_trip("HeapData(Vec,String,Box)", heap_ref, root, n)?.print();
    }

    // FrozenStr: strings of various sizes
    for &str_len in &[10, 100, 1000] {
        let n = 2000;
        let heap = ErasingHeap::new();
        let s: String = "x".repeat(str_len);
        let mut root = Value::new_none();
        for _ in 0..n {
            root = heap.alloc_str(&s);
        }
        let heap_ref =
            heap.into_ref_named(TestHeapName::heap_name(&format!("bench_str_{str_len}")));
        let label = match str_len {
            10 => "FrozenStr(len=10)",
            100 => "FrozenStr(len=100)",
            1000 => "FrozenStr(len=1000)",
            _ => unreachable!(),
        };
        bench_owned_frozen_value_round_trip(label, heap_ref, root, n)?.print();
    }

    // Module eval: expressions producing ints and strings
    {
        use crate::environment::FrozenModule;
        use crate::environment::Module;
        use crate::eval::Evaluator;
        use crate::syntax::AstModule;
        use crate::syntax::Dialect;

        let mut lines = Vec::new();
        let n = 500;
        for i in 0..n {
            lines.push(format!("v{i} = {i} + 1"));
        }
        let code = lines.join("\n");

        let ast = AstModule::parse("bench_module.star", code, &Dialect::Extended)?;
        let globals = GlobalsBuilder::new().build_named(GlobalFrozenHeapName {
            name: BENCH_EVAL_HEAP_NAME,
        });
        let frozen_module = Module::with_temp_heap(|module| {
            {
                let mut eval = Evaluator::new(&module);
                eval.eval_module(ast, &globals).unwrap();
            }
            module.freeze_named(TestHeapName::heap_name("bench_module_eval"))
        })?;

        use std::any::TypeId;
        use std::time::Instant;

        use pagable::PagableDeserialize;
        use pagable::storage::handle::PagableStorageHandle;
        use pagable::storage::in_memory::InMemoryPagableStorage;
        use pagable::storage::support::SerializerForPaging;

        let backing = InMemoryPagableStorage::new();
        let storage = backing.handle();
        let handle = PagableStorageHandle::new(storage.clone());
        let storage_ctx = storage.storage_context();

        let ser_start = Instant::now();
        let mut ser = SerializerForPaging::new(storage_ctx);
        frozen_module
            .pagable_serialize(&mut ser)
            .map_err(crate::Error::new_other)?;
        let (data, arcs) = ser.finish();
        let ser_us = ser_start.elapsed().as_micros();
        let bytes = data.len();

        let top_key = {
            let finished = ArcSerCache::new();
            storage
                .page_out_item(data, arcs, &finished, storage_ctx)
                .map_err(crate::Error::new_other)?
        };

        let top_data = storage
            .fetch_arc_or_data_blocking(&TypeId::of::<()>(), &top_key)
            .map_err(crate::Error::new_other)?
            .right()
            .expect("should be data");

        let deser_start = Instant::now();
        let mut de = handle.root_deserializer(top_key, &top_data);
        let restored =
            FrozenModule::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
        let deser_us = deser_start.elapsed().as_micros();

        // Access one value to confirm correctness
        let v0 = restored.get("v0")?.as_ref().value().unpack_i32().unwrap();
        assert_eq!(v0, 1);

        BenchResult {
            label: "Module(500 int assigns)",
            count: n,
            ser_us,
            deser_us,
            bytes,
        }
        .print();
    }

    eprintln!("=== done ===");
    eprintln!();

    Ok(())
}

// ============================================================================
// Regression: concurrent page-in must not hash a not-yet-deserialized value.
//
// A `SmallMapFvData` is keyed by a `GateKey` whose deserialization blocks on a
// global gate. Thread A claims the key and blocks mid-deserialization; thread B
// then deserializes the map, which must hash that key. Before the fix, B got the
// not-yet-materialized (sentinel) pointer and hashing it panicked with
// "accessing a frozen value that has not been deserialized yet". With the fix B
// blocks until the key is materialized, then hashes the real value.
//
// The gate makes the cross-thread collision deterministic: B is only started
// once A has signalled it is blocked inside the key's deser, so the key is
// guaranteed in-progress when B reaches it.
// ============================================================================

/// Channels used by [`GateField`] to block a key's deserialization until the
/// test releases it.
struct GateChannels {
    /// Sent from inside the key's deser once this thread has claimed the key and
    /// is about to block.
    started: std::sync::mpsc::Sender<()>,
    /// The key's deser blocks on this until the test releases it.
    release: std::sync::mpsc::Receiver<()>,
}

static GATE: std::sync::Mutex<Option<GateChannels>> = std::sync::Mutex::new(None);

/// A unit field whose deserialization blocks on [`GATE`] (one-shot). Embedding it
/// in `GateKey` keeps that key's slot in-progress while another thread races to
/// hash it.
#[derive(Debug, Allocative)]
struct GateField;

impl crate::pagable::StarlarkSerialize for GateField {
    fn starlark_serialize(
        &self,
        _ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
    ) -> crate::Result<()> {
        Ok(())
    }
}

impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for GateField {
    fn starlark_deserialize(
        _ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        // One-shot: only the first deserialization (the claimer) blocks.
        if let Some(ch) = GATE.lock().unwrap().take() {
            ch.started.send(()).ok();
            ch.release.recv().ok();
        }
        Ok(GateField)
    }
}

/// Hashable key whose deserialization blocks (via [`GateField`]).
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("GateKey({})", self.id)]
struct GateKey {
    gate: GateField,
    id: u32,
}

starlark_simple_value!(GateKey);

#[starlark_value(type = "GateKey")]
impl<'v> StarlarkValue<'v> for GateKey {
    fn write_hash(&self, hasher: &mut crate::collections::StarlarkHasher) -> crate::Result<()> {
        use std::hash::Hasher;
        hasher.write_u32(self.id);
        Ok(())
    }

    fn equals(&self, other: crate::values::Value<'v>) -> crate::Result<bool> {
        Ok(other
            .downcast_ref::<GateKey>()
            .is_some_and(|o| o.id == self.id))
    }
}

/// Cross-thread regression. Thread A claims a `GateKey` and blocks inside its
/// deserialization (via the gate), so the key's slot stays in-progress. Thread B
/// then deserializes a `SmallMapFvData` keyed by it, which must hash that key.
/// Pre-fix B took the not-yet-materialized (sentinel) key and panicked hashing
/// it; with the fix B waits for the slot, then hashes the real value. The
/// `started`/`release` channels make the collision deterministic: B starts only
/// after A is confirmed blocked, and A is released only after B has reached the
/// key.
#[test]
fn test_concurrent_page_in_does_not_hash_sentinel_key() {
    use std::sync::mpsc;
    use std::time::Duration;

    use dupe::Dupe;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;
    use starlark_map::small_map::SmallMap;

    // Heap: a gated key, a value, and a map keyed by the gated key.
    let heap = ErasingHeap::new();
    let key_fv = heap.alloc_simple(GateKey {
        gate: GateField,
        id: 7,
    });
    let val_fv = heap.alloc_simple(SimpleData {
        flag: true,
        count: 42,
    });
    let map_fv = heap.with(|h| {
        let mut entries = SmallMap::new();
        entries.insert_hashed(
            ErasingHeap::restore_one(h, key_fv).get_hashed().unwrap(),
            ErasingHeap::restore_one(h, val_fv),
        );
        erase(h.alloc_simple(SmallMapFvData { entries }))
    });
    let heap_ref = heap.into_ref_named(TestHeapName::heap_name("gated_key"));

    // Two roots from the same heap: the key alone, and the map.
    // SAFETY: `heap_ref` owns the arena hosting both values.
    let ofv_key = unsafe { OwnedFrozen::from_erased(heap_ref.dupe(), key_fv) };
    let ofv_map = unsafe { OwnedFrozen::from_erased(heap_ref.dupe(), map_fv) };

    // Serialize each root through the production storage path.
    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let storage_ctx = storage.storage_context();
    let ser_one = |ofv: &OwnedFrozen<Value<'static>>| -> pagable::DataKey {
        let mut ser = SerializerForPaging::new(storage_ctx);
        ofv.pagable_serialize(&mut ser).unwrap();
        let (data, arcs) = ser.finish();
        let finished = ArcSerCache::new();
        storage
            .page_out_item(data, arcs, &finished, storage_ctx)
            .unwrap()
    };
    let key_data = ser_one(&ofv_key);
    let map_data = ser_one(&ofv_map);

    // Force page-in through deserialization. Otherwise resident-heap reuse
    // returns the source allocation and `GateField::starlark_deserialize` never runs.
    drop(ofv_key);
    drop(ofv_map);
    drop(heap_ref);

    // Install the gate so the key's deserialization blocks.
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    *GATE.lock().unwrap() = Some(GateChannels {
        started: started_tx,
        release: release_rx,
    });

    let handle = Arc::new(PagableStorageHandle::new(storage.clone()));

    let deser_one = |key: pagable::DataKey,
                     handle: Arc<PagableStorageHandle>,
                     storage: Arc<dyn pagable::storage::traits::PagableStorage>|
     -> std::thread::JoinHandle<crate::Result<()>> {
        std::thread::spawn(move || {
            let top = storage
                .fetch_data_blocking(&key)
                .map_err(crate::Error::new_other)?;
            let mut de = handle.root_deserializer(key, &top);
            <OwnedFrozen<Value>>::pagable_deserialize(&mut de).map_err(crate::Error::new_other)?;
            Ok(())
        })
    };

    // Thread A: deserialize the key. It claims the key and blocks inside GateField.
    let h_a = deser_one(key_data, handle.clone(), storage.clone());
    // Wait until A is blocked inside the key's deser (key is now in-progress).
    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("key deser never started");

    // Thread B: deserialize the map, which must hash the (in-progress) key.
    let h_b = deser_one(map_data, handle.clone(), storage.clone());

    // Give B time to reach the key — pre-fix it hashes the sentinel and panics
    // here; post-fix it blocks on the slot — then release A so the key
    // materializes (waking a post-fix waiter).
    std::thread::sleep(Duration::from_millis(500));
    release_tx.send(()).ok();

    h_a.join().expect("thread A panicked").unwrap();
    match h_b.join() {
        Ok(r) => r.unwrap_or_else(|e| panic!("map deser failed: {e:#}")),
        // Pre-fix: B panicked hashing the sentinel key — re-raise so the test fails.
        Err(panic) => std::panic::resume_unwind(panic),
    }

    *GATE.lock().unwrap() = None;
}

// `BcInstrs` (compiled bytecode) is only reachable inside a `FrozenDef` inside a
// `FrozenModule`, so these tests round-trip a module of functions through the
// production pagable path and run the deserialized bytecode.

/// Page a `FrozenModule` out and back in through the production pagable path.
fn page_out_in_module(
    frozen_module: &crate::environment::FrozenModule,
) -> crate::Result<crate::environment::FrozenModule> {
    use std::any::TypeId;

    use pagable::PagableDeserialize;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::storage::in_memory::InMemoryPagableStorage;
    use pagable::storage::support::SerializerForPaging;

    use crate::environment::FrozenModule;

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let handle = PagableStorageHandle::new(storage.clone());
    let storage_ctx = storage.storage_context();

    let mut ser = SerializerForPaging::new(storage_ctx);
    frozen_module
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();

    let top_key = {
        let finished = ArcSerCache::new();
        storage
            .page_out_item(data, arcs, &finished, storage_ctx)
            .map_err(crate::Error::new_other)?
    };

    let top_data = storage
        .fetch_arc_or_data_blocking(&TypeId::of::<()>(), &top_key)
        .map_err(crate::Error::new_other)?
        .right()
        .expect("top-level key should return data, not a cached arc");

    let mut de = handle.root_deserializer(top_key, &top_data);
    FrozenModule::pagable_deserialize(&mut de).map_err(crate::Error::new_other)
}

/// Decode a module's root pointer, verifying but excluding its seal identity.
/// The owner heap (including bytecode) is in a separate arc, not these bytes.
fn module_root_without_heap_identity(
    bytes: &[u8],
    expected_heap_id: HeapRefId,
) -> pagable::Result<(Vec<u8>, u32, bool)> {
    use pagable::PagableDeserializer;
    use pagable::testing::TestingDeserializer;

    use crate::pagable::serialized_frozen_value::SerializedFrozenValue;
    use crate::values::layout::heap::sealed::HeapSerializationNonce;

    let mut de = TestingDeserializer::new(bytes);
    let owner_tag = u8::pagable_deserialize(&mut de)?;
    anyhow::ensure!(owner_tag == 1, "expected an out-of-line module heap owner");
    let name_start = de.position().byte_pos;
    let name = FrozenHeapName::pagable_deserialize(&mut de)?;
    let name_end = de.position().byte_pos;
    let nonce = HeapSerializationNonce::pagable_deserialize(&mut de)?;
    anyhow::ensure!(
        HeapRefId::new(&name, nonce) == expected_heap_id,
        "unexpected module owner identity"
    );
    let SerializedFrozenValue::HeapPtr {
        heap_id,
        value_index,
        is_str,
    } = SerializedFrozenValue::pagable_deserialize(&mut de)?
    else {
        return Err(anyhow::anyhow!("expected a module root heap pointer"));
    };
    anyhow::ensure!(
        heap_id == expected_heap_id,
        "unexpected module heap identity"
    );
    anyhow::ensure!(
        de.position().byte_pos == bytes.len(),
        "unexpected trailing data in module root"
    );
    Ok((bytes[name_start..name_end].to_vec(), value_index, is_str))
}

#[test]
fn test_module_root_without_heap_identity() -> pagable::Result<()> {
    use pagable::testing::TestingSerializer;

    use crate::pagable::serialized_frozen_value::SerializedFrozenValue;

    let owner = OwnedFrozen::<Value>::build(TestHeapName::heap_name("module_root"), |heap| {
        heap.alloc("value").to_value()
    });
    let heap_id = owner.heap_arc().heap_ref_id().unwrap();
    let name = owner.name().unwrap();
    let nonce = owner.heap_arc().serialization_nonce().unwrap();
    let other_id = HeapRefId::from_heap_name(&TestHeapName::heap_name("other_module"));
    let mut name_ser = TestingSerializer::new();
    name.pagable_serialize(&mut name_ser)?;
    let name_bytes = name_ser.finish();
    let encode = |owner_tag: u8, root: SerializedFrozenValue| -> pagable::Result<Vec<u8>> {
        let mut ser = TestingSerializer::new();
        owner_tag.pagable_serialize(&mut ser)?;
        name.pagable_serialize(&mut ser)?;
        nonce.pagable_serialize(&mut ser)?;
        root.pagable_serialize(&mut ser)?;
        Ok(ser.finish())
    };
    let root = |value_index, is_str| SerializedFrozenValue::HeapPtr {
        heap_id,
        value_index,
        is_str,
    };

    let bytes = encode(1, root(7, false))?;
    assert_eq!(
        module_root_without_heap_identity(&bytes, heap_id)?,
        (name_bytes.clone(), 7, false)
    );
    assert_eq!(
        module_root_without_heap_identity(&encode(1, root(8, true))?, heap_id)?,
        (name_bytes, 8, true)
    );
    assert!(module_root_without_heap_identity(&bytes, other_id).is_err());
    let wrong_heap = SerializedFrozenValue::HeapPtr {
        heap_id: other_id,
        value_index: 7,
        is_str: false,
    };
    assert!(module_root_without_heap_identity(&encode(1, wrong_heap)?, heap_id).is_err());
    assert!(module_root_without_heap_identity(&encode(0, root(7, false))?, heap_id).is_err());
    assert!(
        module_root_without_heap_identity(
            &encode(1, SerializedFrozenValue::InlineInt(7))?,
            heap_id
        )
        .is_err()
    );
    assert!(module_root_without_heap_identity(&bytes[..bytes.len() - 1], heap_id).is_err());
    assert!(module_root_without_heap_identity(&[], heap_id).is_err());
    let duplicate = [bytes.as_slice(), bytes.as_slice()].concat();
    assert!(module_root_without_heap_identity(&duplicate, heap_id).is_err());
    Ok(())
}

/// Serialize a `FrozenModule`, returning the top-level data buffer.
fn serialize_module_top_bytes(
    frozen_module: &crate::environment::FrozenModule,
    storage_ctx: &pagable::StorageContext,
) -> crate::Result<Vec<u8>> {
    use pagable::storage::support::SerializerForPaging;

    let mut ser = SerializerForPaging::new(storage_ctx);
    frozen_module
        .pagable_serialize(&mut ser)
        .map_err(crate::Error::new_other)?;
    let (data, arcs) = ser.finish();
    assert_eq!(
        arcs.len(),
        1,
        "a module root should retain exactly one owner heap"
    );
    Ok(data)
}

/// Round-trips a module of functions and runs the *deserialized* bytecode.
/// Each function targets a different bytecode-arg shape (see inline notes).
#[test]
fn test_bcinstrs_def_round_trip_via_module() -> crate::Result<()> {
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;

    let code = r#"
def add(a, b):
    return a + b

def call_named():
    return add(a = 2, b = 40)

def call_star():
    xs = [3, 4]
    return add(*xs)

def use_list_method():
    xs = [1]
    xs.append(2)
    xs.append(3)
    return xs

def use_native():
    return len([10, 20, 30, 40])

def use_loop(n):
    total = 0
    for i in range(n):
        total = total + i
    return total

def use_comprehension():
    return [i * i for i in range(4)]

def use_string_method():
    return "abc".upper()

def many_locals():
    a = 1
    b = 2
    c = 3
    d = 4
    e = 5
    f = 6
    return a + b + c + d + e + f
"#;

    let ast = AstModule::parse("test_bcinstrs.star", code.to_owned(), &Dialect::Extended)?;
    let globals = GlobalsBuilder::standard().build_named(GlobalFrozenHeapName {
        name: TEST_BCINSTRS_HEAP_NAME,
    });
    let frozen_module = Module::with_temp_heap(|module| {
        {
            let mut eval = Evaluator::new(&module);
            eval.eval_module(ast, &globals).unwrap();
        }
        module.freeze_named(TestHeapName::heap_name("test_bcinstrs_def_round_trip"))
    })?;

    let restored = page_out_in_module(&frozen_module)?;

    // Each restored function carries its own captured globals, so the call
    // module needs none.
    Module::with_temp_heap(|call_module| -> crate::Result<()> {
        let mut eval = Evaluator::new(&call_module);

        // Each restored function comes to the call module's brand through the module edge,
        // which makes `restored`'s heap a reference of the call module's frozen heap.
        let get_fn = |name: &str| {
            let f = restored.get(name)?;
            crate::Result::Ok(
                call_module.frozen_heap(|fh, edge| edge.rebrand(f.as_ref().add_to_frozen_heap(fh))),
            )
        };

        let two = eval.heap().alloc(2i32).to_value();
        let three = eval.heap().alloc(3i32).to_value();
        assert_eq!(
            eval.eval_function(get_fn("add")?, &[two, three], &[])?
                .unpack_i32(),
            Some(5),
            "add(2, 3)"
        );

        // for-loop / LoopDepth; 0+1+2+3+4 == 10.
        let five = eval.heap().alloc(5i32).to_value();
        assert_eq!(
            eval.eval_function(get_fn("use_loop")?, &[five], &[])?
                .unpack_i32(),
            Some(10),
            "use_loop(5)"
        );

        // KnownMethod (str method).
        assert_eq!(
            eval.eval_function(get_fn("use_string_method")?, &[], &[])?
                .unpack_str(),
            Some("ABC"),
            "use_string_method()"
        );

        // Remaining zero-arg fns, compared via Display.
        for (name, expected) in [
            ("call_named", "42"),             // BcCallArgsFull: named args
            ("call_star", "7"),               // BcCallArgsFull: *args
            ("use_list_method", "[1, 2, 3]"), // KnownMethod: list.append
            ("use_native", "4"),              // BcNativeFunction: len
            ("use_comprehension", "[0, 1, 4, 9]"),
            ("many_locals", "21"), // non-empty InstrEnd local_names
        ] {
            let got = eval.eval_function(get_fn(name)?, &[], &[])?;
            assert_eq!(got.to_string(), expected, "calling restored `{name}`");
        }

        Ok(())
    })?;

    Ok(())
}

/// Independent compilations should assign the same index to the module root.
/// This checks the top-level record, not the bytecode in its out-of-line heap.
#[test]
fn test_frozen_module_root_serialization_deterministic() -> crate::Result<()> {
    use crate::environment::FrozenModule;
    use crate::environment::Module;
    use crate::eval::Evaluator;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;

    let code = r#"
def add(a, b):
    return a + b

def use_loop(n):
    total = 0
    for i in range(n):
        total = total + i
    return total

def use_comprehension():
    return [i * i for i in range(4)]
"#;

    // Shared globals so native-fn refs (`range`) get the same `HeapRefId` in
    // both compilations.
    let globals = GlobalsBuilder::standard().build_named(GlobalFrozenHeapName {
        name: TEST_BCINSTRS_DET_GLOBALS_HEAP_NAME,
    });

    let compile = || -> crate::Result<FrozenModule> {
        let ast = AstModule::parse(
            "test_bcinstrs_det.star",
            code.to_owned(),
            &Dialect::Extended,
        )?;
        Ok(Module::with_temp_heap(|module| {
            {
                let mut eval = Evaluator::new(&module);
                eval.eval_module(ast, &globals).unwrap();
            }
            module.freeze_named(TestHeapName::heap_name("test_bcinstrs_det"))
        })?)
    };

    let backing = pagable::storage::in_memory::InMemoryPagableStorage::new();
    let storage = backing.handle();
    let storage_ctx = storage.storage_context();

    let module_a = compile()?;
    let module_b = compile()?;
    let root_a = module_root_without_heap_identity(
        &serialize_module_top_bytes(&module_a, storage_ctx)?,
        module_a.frozen_heap().heap_arc().heap_ref_id().unwrap(),
    )
    .map_err(crate::Error::new_other)?;
    let root_b = module_root_without_heap_identity(
        &serialize_module_top_bytes(&module_b, storage_ctx)?,
        module_b.frozen_heap().heap_arc().heap_ref_id().unwrap(),
    )
    .map_err(crate::Error::new_other)?;

    assert_eq!(
        root_a, root_b,
        "the module root should be identical apart from its seal identity"
    );

    Ok(())
}
