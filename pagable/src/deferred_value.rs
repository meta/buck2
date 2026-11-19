/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Fallible, demand-loaded native fields.
//!
//! A field, not a `PagableArc`: readers get a plain `&T` borrowed from the
//! owner, so a field cannot be evicted while the owner is alive. Eviction is
//! the owner's unit, a DICE value or an arc dropped when unreferenced, after
//! which the field comes back deferred. `PagableArc` evicts independently of
//! its owner, which needs every read to go through a pin guard; that suits a
//! large, rarely read value under an owner that never goes away, and nothing
//! else. A resolved field therefore stays resident for the owner's lifetime,
//! and page-out references a restored arc by its key rather than reading the
//! fields under it.

use std::any::TypeId;
use std::cell::Cell;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::PagableDeserializer;
use crate::PagableDeserializerRecipe;
use crate::PagableSerialize;
use crate::PagableSerializer;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseDyn;
use crate::arc_erase::deserialize_arc;
use crate::read_failures::DeferredReadError;
use crate::read_failures::DeferredReadFailures;
use crate::read_failures::ReadRefused;
use crate::storage::data::DataKey;
use crate::storage::handle::PagableStorageHandle;
use crate::storage::handle::WeakPagableStorageHandle;

/// A resident value, or a fallible loader shared by all clones of the field.
///
/// Optional metadata lives with the loader, so inspecting a deferred value need
/// not restore its payload. Ready values derive metadata from `T` instead of
/// retaining a separate copy. `M = ()` keeps metadata-free fields unchanged.
///
/// Serialization resolves unread fields before writing them, including when
/// writing into another storage backend, using the value's normal serializer
/// rather than forwarding an unread source-storage key. Metadata is not part
/// of that encoding; an enclosing format must store and restore it separately.
#[derive(Clone, Allocative)]
pub struct DeferredValue<T, M = ()>(DeferredValueState<T, M>);

#[derive(Clone, Allocative)]
enum DeferredValueState<T, M> {
    Ready(T),
    Pending(Arc<PendingValue<T, M>>),
}

#[derive(Allocative)]
struct PendingValue<T, M> {
    metadata: M,
    value: OnceCell<T>,
    /// Released after a successful load.
    load: Mutex<Option<Loader<T>>>,
}

/// What an unread field still holds.
///
/// A concrete type rather than a closure so memory reports can attribute it:
/// allocative has to skip a closure, and everything it captures then shows up
/// only as unattributed allocation.
#[derive(Allocative)]
enum Loader<T> {
    /// A stored arc, restored by key.
    Arc(ArcLoader<T>),
    #[cfg(test)]
    Custom(#[allocative(skip)] Box<dyn Fn() -> crate::Result<T> + Send + Sync>),
}

/// Restores a stored arc on demand from the storage it came from.
#[derive(Allocative)]
struct ArcLoader<T> {
    #[allocative(skip)]
    storage: WeakPagableStorageHandle,
    key: DataKey,
    #[allocative(skip)]
    restore: fn(&PagableStorageHandle, DataKey) -> crate::Result<T>,
}

impl<T> Loader<T> {
    fn load(&self) -> crate::Result<T> {
        match self {
            Loader::Arc(loader) => loader.load(),
            #[cfg(test)]
            Loader::Custom(load) => load(),
        }
    }
}

impl<T> ArcLoader<T> {
    fn load(&self) -> crate::Result<T> {
        let storage = self
            .storage
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("storage closed before deferred field read"))?;
        (self.restore)(&storage, self.key).inspect_err(|error| {
            storage
                .storage_context()
                .get_or_init(DeferredReadFailures::default)
                .record_error(format_args!("arc {:?}", self.key), error);
        })
    }
}

impl<T, M> PendingValue<T, M> {
    fn load(&self) -> crate::Result<T> {
        let mut loader = self.load.lock();
        let value = loader
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("resolved native field has no loader"))?
            .load()?;
        // A resident value no longer needs the loader's resources.
        *loader = None;
        Ok(value)
    }
}

thread_local! {
    /// Native cells this thread is initializing; see [`HeldCell`].
    static HELD_CELLS: Cell<u32> = const { Cell::new(0) };
}

/// Marks the calling thread as initializing a native cell - a pending field or
/// an arc-cache entry - for the guard's lifetime.
///
/// While one is held, a cold [`DeferredValue::read`] is refused instead of
/// blocking. A thread that holds a cell and waits on a field could complete a
/// wait cycle with another thread, and the cells detect no cycles. Waits between
/// arc-cache cells alone cannot cycle, because rows are content addressed, so
/// refusing field reads is what rules deadlock out.
pub(crate) struct HeldCell(());

impl HeldCell {
    pub(crate) fn enter() -> Self {
        HELD_CELLS.with(|n| n.set(n.get() + 1));
        HeldCell(())
    }
}

impl Drop for HeldCell {
    fn drop(&mut self) {
        HELD_CELLS.with(|n| n.set(n.get() - 1));
    }
}

fn holding_cell() -> bool {
    HELD_CELLS.with(|n| n.get()) > 0
}

/// Runs a blocking load, telling an enclosing Tokio runtime that this worker
/// thread will block. A single-threaded runtime cannot spare its only thread.
fn blocking<R>(load: impl FnOnce() -> crate::Result<R>) -> crate::Result<R> {
    #[cfg(feature = "tokio")]
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        return match runtime.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(load),
            _ => Err(anyhow::Error::new(ReadRefused(
                "a deferred native field requires a blocking-capable runtime",
            ))),
        };
    }
    load()
}

/// The most threads that help one caller of [`DeferredValue::read_all`]:
/// `BUCK2_DEFERRED_READ_THREADS` (0 leaves every bulk read to its caller), else
/// a quarter of the available parallelism, at least 4 and at most 32.
fn read_all_helper_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        if let Some(raw) = std::env::var_os("BUCK2_DEFERRED_READ_THREADS") {
            return match raw.to_str().and_then(|v| v.parse::<usize>().ok()) {
                Some(n) if n <= 256 => n,
                _ => panic!("BUCK2_DEFERRED_READ_THREADS={raw:?} must be an integer in 0..=256"),
            };
        }
        std::thread::available_parallelism()
            .map(|n| n.get() / 4)
            .unwrap_or(4)
            .clamp(4, 32)
    })
}

/// Fields of a bulk read for each thread that helps with it. Starting a thread costs about
/// as much as reading a field, so a helper has to have several to read.
const READ_ALL_FIELDS_PER_HELPER: usize = 4;

impl<T, M> DeferredValue<T, M> {
    /// Construct a field that needs no storage access.
    pub fn new(value: T) -> Self {
        Self(DeferredValueState::Ready(value))
    }

    /// Defer a native load. The loader must not retain the storage cache that
    /// owns the field. It also cannot perform a cold read of any deferred field,
    /// its own included: `read` refuses that while a cell is held (see
    /// [`HeldCell`]), so such a loader fails instead of deadlocking.
    fn pending_loader(loader: Loader<T>, metadata: M) -> Self {
        Self(DeferredValueState::Pending(Arc::new(PendingValue {
            metadata,
            value: OnceCell::new(),
            load: Mutex::new(Some(loader)),
        })))
    }

    /// Inspect metadata without loading, including while another cell is held.
    /// The caller must make `from_ready` agree with the metadata supplied at
    /// deserialization. Pending values retain that metadata after a read succeeds
    /// or fails, so this operation never needs the loader or its lock.
    pub fn metadata_or_else<'a>(&'a self, from_ready: impl FnOnce(&'a T) -> &'a M) -> &'a M {
        match &self.0 {
            DeferredValueState::Ready(value) => from_ready(value),
            DeferredValueState::Pending(pending) => &pending.metadata,
        }
    }

    /// Resolve once, sharing the result across readers. Failed loads may be
    /// retried; no partially initialized value is exposed.
    ///
    /// Fails without loading when called while this thread is restoring another
    /// value, because such a read could deadlock; see `HeldCell` in this module.
    pub fn read(&self) -> Result<&T, DeferredReadError> {
        match &self.0 {
            DeferredValueState::Ready(value) => Ok(value),
            DeferredValueState::Pending(pending) => match pending.value.get() {
                Some(value) => Ok(value),
                None => {
                    if holding_cell() {
                        return Err(ReadRefused(
                            "Cold deferred field read while this thread is restoring another value; refused because it could deadlock",
                        )
                        .into());
                    }
                    Ok(blocking(|| {
                        let _held = HeldCell::enter();
                        pending.value.get_or_try_init(|| pending.load())
                    })?)
                }
            },
        }
    }

    /// Resolve every field of `fields` that is still unread, several at a time,
    /// and return once each of them is resolved or has failed.
    ///
    /// For a caller about to read the fields of many values: `read` on each
    /// would do one storage round trip after another on the caller's thread.
    /// Here the caller shares the reads with threads that live only for this
    /// call, one for every few fields up to a limit.
    ///
    /// Returns the first failure. A failed field stays unread, so a later
    /// `read` retries and reports it. Like `read`, fails without loading when
    /// called while this thread is restoring another value.
    pub fn read_all<'a>(fields: impl IntoIterator<Item = &'a Self>) -> Result<(), DeferredReadError>
    where
        T: Send + Sync + 'a,
        M: Send + Sync + 'a,
    {
        let unread: Vec<&PendingValue<T, M>> = fields
            .into_iter()
            .filter_map(|field| match &field.0 {
                DeferredValueState::Pending(pending) if pending.value.get().is_none() => {
                    Some(&**pending)
                }
                _ => None,
            })
            .collect();
        if unread.is_empty() {
            return Ok(());
        }
        if holding_cell() {
            return Err(ReadRefused(
                "Cold bulk read of deferred fields while this thread is restoring another value; refused because it could deadlock",
            )
            .into());
        }

        // Index of the next field no thread has taken.
        let next = AtomicUsize::new(0);
        let first_error = StdMutex::new(None);
        let read_fields = || {
            while let Some(field) = unread.get(next.fetch_add(1, Ordering::Relaxed)) {
                let _held = HeldCell::enter();
                if let Err(error) = field.value.get_or_try_init(|| field.load()) {
                    first_error
                        .lock()
                        .expect("bulk read error should not be poisoned")
                        .get_or_insert(error);
                }
            }
        };
        blocking(|| {
            let helpers = (unread.len() / READ_ALL_FIELDS_PER_HELPER).min(read_all_helper_limit());
            std::thread::scope(|scope| {
                for i in 0..helpers {
                    let helper = std::thread::Builder::new().name(format!("deferred-read-{i}"));
                    // A helper that cannot be started is done without: the threads that
                    // did start read its share.
                    if helper.spawn_scoped(scope, read_fields).is_err() {
                        break;
                    }
                }
                read_fields();
            });
            Ok(())
        })?;
        match first_error
            .into_inner()
            .expect("bulk read error should not be poisoned")
        {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    /// Whether reading this field can return without invoking its loader.
    pub fn is_resolved(&self) -> bool {
        match &self.0 {
            DeferredValueState::Ready(_) => true,
            DeferredValueState::Pending(pending) => pending.value.get().is_some(),
        }
    }
}

impl<A: ArcErase> DeferredValue<A> {
    /// Consume an existing Arc edge without fetching its row when enabled.
    /// Inline testing formats, which have no stable keys, remain eager.
    pub fn deserialize_arc<'de, D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
        enabled: bool,
    ) -> crate::Result<Self> {
        Self::deserialize_arc_with_metadata(deserializer, enabled, ())
    }
}

impl<A: ArcErase, M> DeferredValue<A, M> {
    /// Defer an existing Arc edge with metadata supplied by its enclosing format.
    /// The metadata must describe the stored value. If deferral is disabled or
    /// the format has no stable Arc keys, restore eagerly and discard metadata.
    pub fn deserialize_arc_with_metadata<'de, D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
        enabled: bool,
        metadata: M,
    ) -> crate::Result<Self> {
        if enabled && let Some(key) = deserializer.take_arc_key()? {
            fn restore<A: ArcErase>(
                storage: &PagableStorageHandle,
                key: DataKey,
            ) -> crate::Result<A> {
                fn decode<A: ArcErase>(
                    deserializer: &mut dyn PagableDeserializer<'_>,
                    _recipe: Arc<dyn PagableDeserializerRecipe>,
                ) -> crate::Result<Box<dyn ArcEraseDyn>> {
                    Ok(Box::new(A::deserialize_inner(deserializer)?))
                }
                let value = storage.deserialize_arc_by_key(key, TypeId::of::<A>(), decode::<A>)?;
                value
                    .as_arc_any()
                    .downcast_ref::<A>()
                    .map(ArcErase::dupe_strong)
                    .ok_or_else(|| anyhow::anyhow!("deferred arc type mismatch"))
            }
            return Ok(Self::pending_loader(
                Loader::Arc(ArcLoader {
                    storage: deserializer.storage().downgrade(),
                    key,
                    restore: restore::<A>,
                }),
                metadata,
            ));
        }
        Ok(Self::new(deserialize_arc(deserializer)?))
    }
}

impl<T: PagableSerialize, M> PagableSerialize for DeferredValue<T, M> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        self.read()?.pagable_serialize(serializer)
    }
}

impl<T: fmt::Debug, M> fmt::Debug for DeferredValue<T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            DeferredValueState::Ready(value) => f.debug_tuple("Ready").field(value).finish(),
            DeferredValueState::Pending(pending) => f
                .debug_tuple("Deferred")
                .field(&pending.value.get())
                .finish(),
        }
    }
}

#[cfg(test)]
impl<T> DeferredValue<T> {
    fn pending(load: impl Fn() -> crate::Result<T> + Send + Sync + 'static) -> Self {
        Self::pending_loader(Loader::Custom(Box::new(load)), ())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    static_assertions::const_assert_eq!(
        std::mem::size_of::<DeferredValue<Arc<u8>, [u8; 32]>>(),
        std::mem::size_of::<DeferredValue<Arc<u8>>>(),
    );

    #[test]
    fn metadata_survives_failed_and_successful_reads() -> crate::Result<()> {
        let loads = Arc::new(AtomicUsize::new(0));
        let count = loads.clone();
        let value = DeferredValue::pending_loader(
            Loader::Custom(Box::new(move || {
                if count.fetch_add(1, Ordering::Relaxed) == 0 {
                    anyhow::bail!("first read failed");
                }
                Ok(17)
            })),
            17,
        );
        {
            let _held = HeldCell::enter();
            assert_eq!(*value.metadata_or_else(|v| v), 17);
        }
        assert_eq!(loads.load(Ordering::Relaxed), 0);
        assert!(value.read().is_err());
        assert_eq!(*value.metadata_or_else(|v| v), 17);
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        assert_eq!(*value.clone().read()?, 17);
        assert_eq!(*value.metadata_or_else(|v| v), 17);
        assert_eq!(loads.load(Ordering::Relaxed), 2);
        let ready = DeferredValue::new(17);
        assert_eq!(value.metadata_or_else(|v| v), ready.metadata_or_else(|v| v));
        Ok(())
    }

    #[test]
    fn concurrent_clones_load_once() -> crate::Result<()> {
        let loads = Arc::new(AtomicUsize::new(0));
        let count = loads.clone();
        let value = DeferredValue::pending(move || {
            count.fetch_add(1, Ordering::Relaxed);
            Ok(42)
        });
        let barrier = Arc::new(Barrier::new(8));
        std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| {
                    let value = value.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        assert_eq!(*value.read().expect("native value should load"), 42);
                    })
                })
                .collect::<Vec<_>>();
            for handle in handles {
                handle.join().expect("reader should not panic");
            }
        });
        assert!(value.is_resolved());
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn cold_read_inside_a_load_is_refused() {
        let inner = DeferredValue::pending(|| Ok(1));
        let inner_for_loader = inner.clone();
        let outer = DeferredValue::pending(move || Ok(*inner_for_loader.read()? + 1));
        let error = outer.read().expect_err("a nested cold read must fail");
        assert!(error.to_string().contains("could deadlock"), "{error:#}");
        assert!(!inner.is_resolved(), "the refused read must not load");
        assert!(!outer.is_resolved());

        // Resolved fields stay readable inside a load, and the outer field
        // loads normally once nothing cold is left beneath it.
        assert_eq!(*inner.read().expect("a top-level read loads"), 1);
        assert_eq!(
            *outer
                .read()
                .expect("reading a resolved field inside a load is fine"),
            2
        );
    }

    #[test]
    fn cold_read_while_holding_a_cell_is_refused() {
        let value = DeferredValue::pending(|| Ok(3));
        let ready = DeferredValue::<_>::new(4);
        {
            let _held = HeldCell::enter();
            assert!(value.read().is_err());
            assert_eq!(*ready.read().expect("resident values need no load"), 4);
        }
        assert_eq!(*value.read().expect("the guard is released on drop"), 3);
    }

    #[test]
    fn failure_can_be_retried() -> crate::Result<()> {
        let count = AtomicUsize::new(0);
        let value = DeferredValue::pending(move || {
            if count.fetch_add(1, Ordering::Relaxed) == 0 {
                anyhow::bail!("first read failed");
            }
            Ok(17)
        });
        assert!(value.read().is_err());
        assert!(!value.is_resolved());
        assert_eq!(*value.read()?, 17);
        Ok(())
    }

    #[test]
    fn successful_read_releases_the_loader() -> crate::Result<()> {
        let retained = Arc::new(17);
        let weak = Arc::downgrade(&retained);
        let value = DeferredValue::pending(move || Ok(*retained));
        assert!(weak.upgrade().is_some());
        assert_eq!(*value.read()?, 17);
        assert!(weak.upgrade().is_none());
        Ok(())
    }

    // Tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll.
    #[cfg(any(not(miri), target_os = "linux"))]
    #[cfg(feature = "tokio")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn multi_thread_runtime_reads_on_a_worker() -> crate::Result<()> {
        let value = DeferredValue::pending(|| Ok(17));
        // The test body runs on the thread driving `block_on`, where `block_in_place` just
        // calls the loader; only a spawned task runs on a worker it must hand off.
        let read = tokio::spawn(async move { value.read().copied() })
            .await
            .expect("the reading task does not panic")?;
        assert_eq!(read, 17);
        Ok(())
    }

    // Tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll.
    #[cfg(any(not(miri), target_os = "linux"))]
    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn single_thread_runtime_rejects_unresolved_reads() -> crate::Result<()> {
        let value = DeferredValue::pending(|| Ok(17));
        assert!(value.read().is_err());
        assert!(!value.is_resolved());
        assert_eq!(*DeferredValue::<_>::new(17).read()?, 17);
        Ok(())
    }

    #[test]
    fn unread_field_reports_its_loader_to_allocative() -> crate::Result<()> {
        use crate::storage::in_memory::InMemoryPagableStorage;
        use crate::storage::support::SerializerForPaging;
        use crate::storage::traits::ArcSerCache;

        let mem = InMemoryPagableStorage::new();
        let storage = mem.handle();
        let value: Arc<Vec<u8>> = Arc::new(vec![4, 5, 6]);
        let mut serializer = SerializerForPaging::new(storage.storage_context());
        value.pagable_serialize(&mut serializer)?;
        let (data, arcs) = serializer.finish();
        let key = storage
            .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let handle = PagableStorageHandle::new(storage.clone());
        let row = storage.fetch_data_blocking(&key)?;
        let mut deserializer = handle.root_deserializer(key, &row);
        let deferred: DeferredValue<Arc<Vec<u8>>> =
            DeferredValue::deserialize_arc(&mut deserializer, true)?;
        drop(deserializer);

        let mut graph = allocative::FlameGraphBuilder::default();
        graph.visit_root(&deferred);
        let flame = graph.finish_and_write_flame_graph();
        assert!(flame.contains("ArcLoader"), "{flame}");
        Ok(())
    }

    #[test]
    fn read_all_resolves_unread_fields_on_more_than_one_thread() -> crate::Result<()> {
        // The first two loads wait for each other, so they finish only if two threads
        // are loading at once.
        let rendezvous = Arc::new(Barrier::new(2));
        let loads = Arc::new(AtomicUsize::new(0));
        let fields: Vec<DeferredValue<usize>> = (0..64)
            .map(|i| {
                let rendezvous = rendezvous.clone();
                let loads = loads.clone();
                DeferredValue::pending(move || {
                    if loads.fetch_add(1, Ordering::SeqCst) < 2 {
                        rendezvous.wait();
                    }
                    Ok(i)
                })
            })
            .chain([DeferredValue::new(64)])
            .collect();

        DeferredValue::read_all(&fields)?;
        assert!(fields.iter().all(DeferredValue::is_resolved));
        assert_eq!(loads.load(Ordering::SeqCst), 64, "each field loads once");
        for (i, field) in fields.iter().enumerate() {
            assert_eq!(*field.read()?, i);
        }

        DeferredValue::read_all(&fields)?;
        assert_eq!(loads.load(Ordering::SeqCst), 64, "nothing is left to load");
        Ok(())
    }

    #[test]
    fn read_all_of_a_few_fields_reads_them_on_the_caller() -> crate::Result<()> {
        let threads = Arc::new(StdMutex::new(Vec::new()));
        let fields: Vec<DeferredValue<usize>> = (0..READ_ALL_FIELDS_PER_HELPER - 1)
            .map(|i| {
                let threads = threads.clone();
                DeferredValue::pending(move || {
                    threads.lock().unwrap().push(std::thread::current().id());
                    Ok(i)
                })
            })
            .collect();

        DeferredValue::read_all(&fields)?;
        assert!(fields.iter().all(DeferredValue::is_resolved));
        let caller = std::thread::current().id();
        assert_eq!(*threads.lock().unwrap(), vec![caller; fields.len()]);
        Ok(())
    }

    #[test]
    fn read_all_returns_a_failure_and_resolves_the_other_fields() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let fields: Vec<DeferredValue<usize>> = (0..16)
            .map(|i| {
                let attempts = attempts.clone();
                DeferredValue::pending(move || {
                    if i == 5 {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        anyhow::bail!("field 5 has no row");
                    }
                    Ok(i)
                })
            })
            .collect();

        let error = DeferredValue::read_all(&fields).expect_err("one field cannot be read");
        assert!(
            error.to_string().contains("field 5 has no row"),
            "{error:#}"
        );
        for (i, field) in fields.iter().enumerate() {
            assert_eq!(field.is_resolved(), i != 5, "field {i}");
        }
        // The failed field is left for a later read to retry and report.
        fields[5].read().expect_err("still unreadable");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn read_all_is_refused_while_restoring_another_value() -> crate::Result<()> {
        let fields = [
            DeferredValue::pending(|| Ok(1)),
            DeferredValue::pending(|| Ok(2)),
        ];
        {
            let _cell = HeldCell::enter();
            let error = DeferredValue::read_all(&fields).expect_err("a cell is held");
            assert!(error.to_string().contains("could deadlock"), "{error:#}");
            assert!(!fields.iter().any(DeferredValue::is_resolved));
        }
        DeferredValue::read_all(&fields)?;
        assert!(fields.iter().all(DeferredValue::is_resolved));
        Ok(())
    }

    #[test]
    fn failed_arc_load_is_recorded_as_a_deferred_read_failure() -> crate::Result<()> {
        use crate::storage::in_memory::InMemoryPagableStorage;
        use crate::storage::support::SerializerForPaging;
        use crate::storage::traits::ArcSerCache;

        let mem = InMemoryPagableStorage::new();
        let storage = mem.handle();
        // One byte with the varint continuation bit set: as `Vec<u64>` it is a
        // truncated integer, so decoding the row under that type fails.
        let value: Arc<Vec<u8>> = Arc::new(vec![0xC8]);
        let mut serializer = SerializerForPaging::new(storage.storage_context());
        value.pagable_serialize(&mut serializer)?;
        let (data, arcs) = serializer.finish();
        let key = storage
            .page_out_item(data, arcs, &ArcSerCache::new(), storage.storage_context())
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        drop(value);
        storage.arc_cache().clear();
        let handle = PagableStorageHandle::new(storage.clone());
        let row = storage.fetch_data_blocking(&key)?;
        let mut deserializer = handle.root_deserializer(key, &row);
        let deferred: DeferredValue<Arc<Vec<u64>>> =
            DeferredValue::deserialize_arc(&mut deserializer, true)?;
        drop(deserializer);
        let failures = handle
            .storage_context()
            .get_or_init(DeferredReadFailures::default);
        assert!(
            failures.snapshot().is_empty(),
            "nothing failed during page-in"
        );

        deferred
            .read()
            .expect_err("the arc's row does not decode as a `Vec<u64>`");
        let recorded = failures.snapshot();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert!(recorded[0].starts_with("arc "), "{recorded:?}");
        assert_eq!(failures.snapshot(), recorded, "the record persists");
        assert!(!deferred.is_resolved());
        Ok(())
    }

    #[cfg(panic = "unwind")]
    #[test]
    fn panicking_loader_can_be_retried() -> crate::Result<()> {
        let count = AtomicUsize::new(0);
        let value = DeferredValue::pending(move || {
            assert_ne!(count.fetch_add(1, Ordering::Relaxed), 0, "injected panic");
            Ok(17)
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| value.read())).is_err());
        assert_eq!(*value.read()?, 17);
        Ok(())
    }
}
