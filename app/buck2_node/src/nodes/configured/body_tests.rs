/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A node paged back in from storage has its header and reads its body on first use.

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use pagable::PagableSerializer;
use pagable::arc_erase::ArcEraseDyn;
use pagable::storage::data::DataKey;
use pagable::storage::data::PagableData;
use pagable::storage::handle::PagableStorageHandle;
use pagable::storage::in_memory::InMemoryPagableStorage;
use pagable::storage::support::SerializerForPaging;
use pagable::storage::traits::ArcSerCache;
use pagable::storage::traits::CommitFrontier;
use pagable::storage::traits::DeserializedArcCache;
use pagable::storage::traits::PagableStorage;
use pagable::storage::traits::WriteTicket;
use pagable::traits::StorageContext;

use super::*;

/// In-memory storage that counts row reads and can fail the read of one row.
struct ProbeStorage {
    inner: Arc<dyn PagableStorage>,
    context: StorageContext,
    reads: AtomicUsize,
    fail_key: Mutex<Option<DataKey>>,
}

impl ProbeStorage {
    fn new(inner: Arc<dyn PagableStorage>, defer_field_reads: bool) -> Self {
        let context = StorageContext::new();
        if defer_field_reads {
            context.get_or_init(|| DeferredFieldReadsEnabled);
        }
        Self {
            inner,
            context,
            reads: AtomicUsize::new(0),
            fail_key: Mutex::new(None),
        }
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn fail_reads_of(&self, key: Option<DataKey>) {
        *self.fail_key.lock().expect("probe mutex") = key;
    }
}

#[async_trait::async_trait]
impl PagableStorage for ProbeStorage {
    fn arc_cache(&self) -> &DeserializedArcCache {
        self.inner.arc_cache()
    }
    fn commit_frontier(&self) -> CommitFrontier {
        CommitFrontier::everything()
    }
    fn discard_unwritten(&self) {}
    fn fetch_data_blocking(&self, key: &DataKey) -> pagable::Result<Arc<PagableData>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_key.lock().expect("probe mutex").as_ref() == Some(key) {
            return Err(pagable::anyhow!("the probe has no row for this key"));
        }
        self.inner.fetch_data_blocking(key)
    }
    async fn fetch_data(&self, key: &DataKey) -> pagable::Result<Arc<PagableData>> {
        self.fetch_data_blocking(key)
    }
    fn schedule_for_paging(&self, arc: Box<dyn ArcEraseDyn>) {
        self.inner.schedule_for_paging(arc)
    }
    fn associate_arc_with_data_key(&self, arc: &dyn ArcEraseDyn, key: DataKey) {
        self.inner.associate_arc_with_data_key(arc, key)
    }
    fn storage_context(&self) -> &StorageContext {
        &self.context
    }
    fn store_data_ticketed(&self, data: PagableData) -> pagable::Result<(DataKey, WriteTicket)> {
        self.inner.store_data_ticketed(data)
    }
    fn flush(&self) -> pagable::Result<()> {
        self.inner.flush()
    }
}

fn probe_error(error: pagable::Error) -> buck2_error::Error {
    from_any_with_tag(error, ErrorTag::Tier0)
}

struct Probe {
    storage: Arc<ProbeStorage>,
    handle: PagableStorageHandle,
    // Keeps the rows alive.
    _mem: InMemoryPagableStorage,
}

impl Probe {
    fn new(defer_field_reads: bool) -> Self {
        let mem = InMemoryPagableStorage::new();
        let storage = Arc::new(ProbeStorage::new(mem.handle(), defer_field_reads));
        let handle = PagableStorageHandle::new(storage.dupe());
        Self {
            storage,
            handle,
            _mem: mem,
        }
    }

    fn store<T: PagableSerialize>(&self, value: &T) -> buck2_error::Result<DataKey> {
        let mut serializer = SerializerForPaging::new(self.storage.storage_context());
        value
            .pagable_serialize(&mut serializer)
            .map_err(probe_error)?;
        let (bytes, arcs) = serializer.finish();
        self.storage
            .page_out_item(
                bytes,
                arcs,
                &ArcSerCache::new(),
                self.storage.storage_context(),
            )
            .map_err(|error| probe_error(pagable::anyhow!("page out: {error:?}")))
    }

    fn restore<T: for<'de> PagableDeserialize<'de>>(&self, key: DataKey) -> buck2_error::Result<T> {
        let data = self
            .storage
            .fetch_data_blocking(&key)
            .map_err(probe_error)?;
        T::pagable_deserialize(&mut self.handle.root_deserializer(key, &data)).map_err(probe_error)
    }

    /// A fresh allocation of the stored value: nothing restored before it is reused.
    fn restore_fresh<T: for<'de> PagableDeserialize<'de>>(
        &self,
        key: DataKey,
    ) -> buck2_error::Result<T> {
        self.storage.arc_cache().clear();
        self.restore(key)
    }

    /// The key of the body row of the node stored under `key`.
    fn body_key(&self, key: DataKey) -> buck2_error::Result<DataKey> {
        let fetch = |key| self.storage.fetch_data_blocking(key).map_err(probe_error);
        let node = fetch(&fetch(&key)?.arcs[0])?;
        Ok(*node.arcs.last().expect("the body is the node's last arc"))
    }
}

fn node(label: &str, value: &str, deps: Vec<ConfiguredTargetNode>) -> ConfiguredTargetNode {
    let label = TargetLabel::testing_parse(label).configure(ConfigurationData::testing_new());
    let base = ConfiguredTargetNode::testing_new(
        label.dupe(),
        "probe_rule",
        ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
        vec![(
            "value",
            Attribute::new_const(None, "", AttrType::string()),
            OwnedCoercedAttr::new_with(|b| b.encode_string(value)).expect("probe attr"),
        )],
        None,
    );
    let body = base.body().expect("resident");
    ConfiguredTargetNode::new(
        label,
        body.target_node().dupe(),
        body.body.resolved_configuration.clone(),
        BuckFixedSortedMap::new(),
        ExecutionPlatformResolution::new_for_testing(None, Vec::new()),
        deps,
        Vec::new(),
        BuckFixedSortedMap::new(),
        PluginLists::new(),
    )
}

fn body_is_resident(node: &ConfiguredTargetNode) -> bool {
    node.0.body.0.is_resolved()
}

fn value_attr(node: &ConfiguredTargetNode) -> buck2_error::Result<ConfiguredAttr> {
    Ok(node
        .body()?
        .get("value", AttrInspectOptions::All)
        .expect("every probe node has `value`")
        .value)
}

/// Two nodes with one label and one hash that differ in their deps: the hash covers only the
/// labels of the deps, and these depend on different nodes that share a label.
fn same_label_and_hash_different_deps() -> (ConfiguredTargetNode, ConfiguredTargetNode) {
    let first = node(
        "cell//pkg:root",
        "root",
        vec![node("cell//pkg:dep", "one", Vec::new())],
    );
    let second = node(
        "cell//pkg:root",
        "root",
        vec![node("cell//pkg:dep", "two", Vec::new())],
    );
    assert_eq!(first.0.hash, second.0.hash);
    (first, second)
}

/// Two nodes with equal headers and different bodies. The hash is forced to match, as a
/// 32-bit hash sometimes will.
fn same_header_different_body() -> (ConfiguredTargetNode, ConfiguredTargetNode) {
    let dep = node("cell//pkg:dep", "dep", Vec::new());
    let first = node("cell//pkg:root", "first", vec![dep.dupe()]);
    let mut second = node("cell//pkg:root", "second", vec![dep]);
    triomphe::Arc::get_mut(&mut second.0)
        .expect("not shared yet")
        .hash = first.0.hash;
    (first, second)
}

#[test]
fn a_restored_node_serves_its_header_without_reading_its_body() -> buck2_error::Result<()> {
    for defer_field_reads in [true, false] {
        let probe = Probe::new(defer_field_reads);
        let shared = node("cell//pkg:shared", "shared", Vec::new());
        let deps: Vec<_> = (0..8)
            .map(|i| {
                node(
                    &format!("cell//pkg:dep{i}"),
                    &format!("dep{i}"),
                    vec![shared.dupe()],
                )
            })
            .collect();
        let original = node("cell//pkg:root", "root", deps);
        let key = probe.store(&original)?;
        let restored: ConfiguredTargetNode = probe.restore_fresh(key)?;
        let reads_for_headers = probe.storage.reads();

        assert_eq!(restored.label(), original.label());
        assert_eq!(restored.rule_type(), original.rule_type());
        assert_eq!(restored.underlying_rule_type(), original.rule_type());
        assert_eq!(restored.rule_kind(), RuleKind::Normal);
        assert!(restored.forward_target().is_none());
        assert_eq!(restored.deps().len(), 8);
        assert_eq!(restored.target_deps().count(), 8);
        assert_eq!(restored.deps()[0].deps()[0].label(), shared.label());
        assert!(
            restored.deps()[0].deps()[0].ptr_eq(&restored.deps()[1].deps()[0]),
            "a shared dep is restored once"
        );
        assert_eq!(probe.storage.reads(), reads_for_headers);
        let all_nodes = || {
            std::iter::once(&restored)
                .chain(restored.deps())
                .chain(restored.deps()[0].deps())
        };
        assert_eq!(
            all_nodes().all(body_is_resident),
            !defer_field_reads,
            "bodies are restored with the node exactly when deferral is off"
        );
        if defer_field_reads {
            assert!(!all_nodes().any(body_is_resident));
        }

        assert_eq!(
            value_attr(&restored.deps()[3])?,
            value_attr(&original.deps()[3])?
        );
        assert!(body_is_resident(&restored.deps()[3]));
        if defer_field_reads {
            assert!(probe.storage.reads() > reads_for_headers);
            assert!(
                !body_is_resident(&restored) && !body_is_resident(&restored.deps()[4]),
                "reading one body reads no other"
            );
        }

        assert_eq!(
            probe.store(&restored)?,
            key,
            "a restored node is stored under the key it was restored from"
        );
    }
    Ok(())
}

#[test]
fn prefetch_bodies_reads_the_bodies_that_are_not_in_memory() -> buck2_error::Result<()> {
    let probe = Probe::new(true);
    let deps: Vec<_> = (0..32)
        .map(|i| node(&format!("cell//pkg:dep{i}"), &format!("dep{i}"), Vec::new()))
        .collect();
    let original = node("cell//pkg:root", "root", deps);
    let key = probe.store(&original)?;
    let restored: ConfiguredTargetNode = probe.restore_fresh(key)?;
    value_attr(&restored.deps()[3])?;
    let reads_before = probe.storage.reads();

    // Only the deps are asked for. One of them is in memory already.
    ConfiguredTargetNode::prefetch_bodies(restored.deps())?;
    assert!(restored.deps().iter().all(body_is_resident));
    assert!(
        !body_is_resident(&restored),
        "a node that was not asked for is left alone"
    );
    let reads_for_bodies = probe.storage.reads() - reads_before;
    assert!(reads_for_bodies >= 31, "{reads_for_bodies}");
    for (restored, original) in restored.deps().iter().zip(original.deps()) {
        assert_eq!(value_attr(restored)?, value_attr(original)?);
    }

    ConfiguredTargetNode::prefetch_bodies(restored.deps())?;
    ConfiguredTargetNode::prefetch_bodies(original.deps())?;
    assert_eq!(
        probe.storage.reads() - reads_before,
        reads_for_bodies,
        "bodies in memory are not read again, and nodes that were built have nothing to read"
    );
    Ok(())
}

#[test]
fn prefetch_bodies_leaves_an_unreadable_body_for_body_to_report() -> buck2_error::Result<()> {
    let probe = Probe::new(true);
    let deps: Vec<_> = (0..8)
        .map(|i| node(&format!("cell//pkg:dep{i}"), &format!("dep{i}"), Vec::new()))
        .collect();
    let dep_key = probe.store(&deps[5])?;
    let key = probe.store(&node("cell//pkg:root", "root", deps))?;
    probe.storage.fail_reads_of(Some(probe.body_key(dep_key)?));
    let restored: ConfiguredTargetNode = probe.restore_fresh(key)?;

    ConfiguredTargetNode::prefetch_bodies(restored.deps())?;
    for (i, dep) in restored.deps().iter().enumerate() {
        assert_eq!(body_is_resident(dep), i != 5, "dep{i}");
    }
    let error = restored.deps()[5]
        .body()
        .expect_err("the body is still unreadable");
    assert!(error.has_tag(ErrorTag::DeferredFieldRead));
    let message = format!("{error:#}");
    assert!(
        message.contains("Failed to restore the body of configured target node `cell//pkg:dep5"),
        "{message}"
    );
    Ok(())
}

#[test]
fn an_unreadable_body_is_an_error_and_leaves_the_header_usable() -> buck2_error::Result<()> {
    let probe = Probe::new(true);
    let original = node("cell//pkg:root", "root", Vec::new());
    let key = probe.store(&original)?;
    probe.storage.fail_reads_of(Some(probe.body_key(key)?));
    let restored: ConfiguredTargetNode = probe.restore_fresh(key)?;

    let error = restored.body().expect_err("the body row cannot be read");
    assert!(error.has_tag(ErrorTag::DeferredFieldRead));
    let message = format!("{error:#}");
    assert!(
        message.contains("Failed to restore the body of configured target node `cell//pkg:root"),
        "{message}"
    );
    assert!(
        message.contains("the probe has no row for this key"),
        "{message}"
    );
    assert_eq!(restored.label(), original.label());
    assert_eq!(restored.rule_type(), original.rule_type());
    assert!(
        restored.try_eq(&restored.dupe())?,
        "a node equals itself without its body"
    );

    // The node is not poisoned: once the row can be read, so can the body.
    probe.storage.fail_reads_of(None);
    assert_eq!(value_attr(&restored)?, value_attr(&original)?);
    Ok(())
}

#[test]
fn try_eq_does_not_depend_on_whether_bodies_are_resident() -> buck2_error::Result<()> {
    let restore_pair = |probe: &Probe, nodes: &(ConfiguredTargetNode, ConfiguredTargetNode)| {
        let first_key = probe.store(&nodes.0)?;
        let second_key = probe.store(&nodes.1)?;
        probe.storage.arc_cache().clear();
        let first: ConfiguredTargetNode = probe.restore(first_key)?;
        let second: ConfiguredTargetNode = probe.restore(second_key)?;
        assert!(!body_is_resident(&first) && !body_is_resident(&second));
        buck2_error::Ok((first, second))
    };

    // Different deps: the headers settle it, whatever is resident.
    let nodes = same_label_and_hash_different_deps();
    assert!(!nodes.0.try_eq(&nodes.1)?);
    let probe = Probe::new(true);
    let (first, second) = restore_pair(&probe, &nodes)?;
    assert_eq!(first.0.hash, second.0.hash);
    assert!(!first.try_eq(&second)?);
    assert!(
        !body_is_resident(&first) && !body_is_resident(&second),
        "the deps are in the header, so no body is read"
    );
    value_attr(&first)?;
    assert!(!first.try_eq(&second)? && !second.try_eq(&first)?);
    value_attr(&second)?;
    assert!(!first.try_eq(&second)?);

    // Equal headers, different bodies: unequal with neither body read, which reads both,
    // and unequal after.
    let nodes = same_header_different_body();
    assert!(!nodes.0.try_eq(&nodes.1)?);
    let probe = Probe::new(true);
    let (first, second) = restore_pair(&probe, &nodes)?;
    assert!(!first.try_eq(&second)?);
    assert!(
        body_is_resident(&first) && body_is_resident(&second),
        "only the bodies can tell these nodes apart"
    );
    assert!(!first.try_eq(&second)?);

    // Two allocations of one node, each with its own body allocation: equal with neither
    // body read, with one, and with both.
    let probe = Probe::new(true);
    let key = probe.store(&node("cell//pkg:leaf", "leaf", Vec::new()))?;
    let first: ConfiguredTargetNode = probe.restore_fresh(key)?;
    let second: ConfiguredTargetNode = probe.restore_fresh(key)?;
    assert!(!first.ptr_eq(&second));
    assert!(first.try_eq(&second)?);
    assert!(body_is_resident(&first) && body_is_resident(&second));
    assert!(first.try_eq(&second)?);

    let first: ConfiguredTargetNode = probe.restore_fresh(key)?;
    value_attr(&first)?;
    let second: ConfiguredTargetNode = probe.restore_fresh(key)?;
    assert!(body_is_resident(&first) && !body_is_resident(&second));
    assert!(first.try_eq(&second)? && second.try_eq(&first)?);
    assert!(
        !std::ptr::eq(first.0.body()?, second.0.body()?),
        "the bodies were compared, not found to be one allocation"
    );
    Ok(())
}

/// Restores two nodes and compares them while the restore is still in progress, when a body
/// cannot be read.
#[derive(Allocative)]
struct CompareDuringRestore {
    first: ConfiguredTargetNode,
    second: ConfiguredTargetNode,
    #[allocative(skip)]
    compared: Option<Result<bool, String>>,
}

impl PagableSerialize for CompareDuringRestore {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.first.pagable_serialize(serializer)?;
        self.second.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for CompareDuringRestore {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let first = ConfiguredTargetNode::pagable_deserialize(deserializer)?;
        let second = ConfiguredTargetNode::pagable_deserialize(deserializer)?;
        let compared = Some(first.try_eq(&second).map_err(|e| format!("{e:#}")));
        assert!(!body_is_resident(&first) && !body_is_resident(&second));
        Ok(Self {
            first,
            second,
            compared,
        })
    }
}

/// The restored value, and the storage its nodes read their bodies from.
fn compare_during_restore(
    nodes: (ConfiguredTargetNode, ConfiguredTargetNode),
) -> buck2_error::Result<(Arc<CompareDuringRestore>, Probe)> {
    let probe = Probe::new(true);
    let key = probe.store(&Arc::new(CompareDuringRestore {
        first: nodes.0,
        second: nodes.1,
        compared: None,
    }))?;
    Ok((probe.restore_fresh(key)?, probe))
}

#[test]
fn try_eq_during_a_restore_is_settled_by_the_header_or_fails() -> buck2_error::Result<()> {
    let different_hash = (
        node("cell//pkg:root", "first", Vec::new()),
        node("cell//pkg:root", "second", Vec::new()),
    );
    assert_ne!(different_hash.0.0.hash, different_hash.1.0.hash);
    for nodes in [different_hash, same_label_and_hash_different_deps()] {
        let (restored, _probe) = compare_during_restore(nodes)?;
        assert_eq!(restored.compared, Some(Ok(false)));
    }

    // Only the bodies can settle this one, and they cannot be read during a restore. The
    // comparison fails; answering "equal" would be wrong, because these nodes differ.
    let (restored, _probe) = compare_during_restore(same_header_different_body())?;
    let error = restored
        .compared
        .clone()
        .expect("compared during the restore")
        .expect_err("a body was needed");
    assert!(
        error.contains("refused because it could deadlock"),
        "{error}"
    );
    assert!(!restored.first.try_eq(&restored.second)?);
    Ok(())
}

#[test]
fn a_restored_forward_node_finds_its_target_without_its_body() -> buck2_error::Result<()> {
    let probe = Probe::new(true);
    let target = node("cell//pkg:root", "root", Vec::new());
    let forward = ConfiguredTargetNode::new_forward(
        target
            .label()
            .unconfigured()
            .configure(ConfigurationData::unspecified()),
        target.dupe(),
    )?;
    let key = probe.store(&forward)?;
    let restored: ConfiguredTargetNode = probe.restore_fresh(key)?;

    assert_eq!(restored.label(), forward.label());
    assert_eq!(restored.rule_type(), &RuleType::Forward);
    assert_eq!(restored.underlying_rule_type(), target.rule_type());
    assert_eq!(restored.rule_kind(), target.rule_kind());
    assert_eq!(restored.unwrap_forward().label(), target.label());
    assert!(!body_is_resident(&restored) && !body_is_resident(restored.unwrap_forward()));

    let actual = |node: &ConfiguredTargetNode| {
        buck2_error::Ok(
            node.body()?
                .get("actual", AttrInspectOptions::All)
                .expect("a forward node has `actual`")
                .value,
        )
    };
    assert_eq!(actual(&restored)?, actual(&forward)?);
    assert!(
        !body_is_resident(restored.unwrap_forward()),
        "`actual` is served from the target's label"
    );
    Ok(())
}
