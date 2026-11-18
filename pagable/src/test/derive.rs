/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#[allow(unused)]
mod inner {
    use crate as pagable;
    // don't add any more use statements here, we want to make sure that the derive macro doesn't require any to be in scope

    #[derive(crate::Pagable, Eq, PartialEq, Debug)]
    pub(super) struct Test {
        xxx: String,
        yyy: String,
        #[pagable(flatten_serde)]
        flatten_serde: Serde,
    }

    #[derive(crate::Pagable, Eq, PartialEq, Debug)]
    enum TestEnum {
        Unit,
        Named { name1: String, name2: String },
        Unnamed(String, String),
    }

    #[derive(serde::Serialize, serde::Deserialize, Eq, PartialEq, Debug)]
    struct Serde(String);

    #[derive(crate::Pagable, Eq, PartialEq, Debug)]
    struct WithDiscard {
        #[pagable(discard = "Default::default()")]
        discard: String,
        keep: String,
    }

    #[derive(crate::PagablePanic)]
    pub(super) struct TestPanic<T>(T);

    #[derive(crate::Pagable, Clone, Eq, PartialEq, Debug)]
    pub(super) struct WithStaticStr {
        name: crate::StaticStr,
        value: i32,
    }

    trait HasItem {
        type Item;
    }

    struct StringItem;

    impl HasItem for StringItem {
        type Item = String;
    }

    #[derive(crate::Pagable)]
    #[pagable(bound = "T::Item: crate::Pagable")]
    struct WithAssociatedBound<T: HasItem> {
        item: T::Item,
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;

        use dashmap::DashMap;
        use dupe::Dupe;
        use either::Either;

        use super::*;
        use crate::Pagable;
        use crate::arc_erase::ArcEraseDyn;
        use crate::pagable_arc::PagableArc;
        use crate::pagable_arc::PinnedPagableArc;
        use crate::storage::data::DataKey;
        use crate::storage::data::PagableData;
        use crate::storage::handle::PagableStorageHandle;
        use crate::storage::in_memory::InMemoryPagableStorage;
        use crate::storage::support::SerializerForPaging;
        use crate::storage::traits::PagableStorage;
        use crate::testing::EmptyPagableStorage;
        use crate::testing::TestingDeserializer;
        use crate::testing::TestingSerializer;
        use crate::traits::PagableDeserialize;
        use crate::traits::PagableSerialize;

        #[test]
        fn test_roundtrip() -> crate::Result<()> {
            let t1 = Test {
                xxx: "xxx".to_owned(),
                yyy: "yyy".to_owned(),
                flatten_serde: Serde("flatten".to_owned()),
            };
            let mut serializer = TestingSerializer::new();
            t1.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = TestingDeserializer::new(&bytes);
            let t2 = Test::pagable_deserialize(&mut deserializer)?;
            assert_eq!(t1, t2);
            Ok(())
        }

        #[test]
        fn test_discard() -> crate::Result<()> {
            let t1 = WithDiscard {
                discard: "discard".to_owned(),
                keep: "keep".to_owned(),
            };
            let mut serializer = TestingSerializer::new();
            t1.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = TestingDeserializer::new(&bytes);
            let t2 = WithDiscard::pagable_deserialize(&mut deserializer)?;
            assert_eq!(t2.discard, "");
            assert_eq!(t2.keep, "keep");
            Ok(())
        }

        #[test]
        fn test_static_str_in_struct() -> crate::Result<()> {
            // Register and create the test string const
            crate::static_str!(TEST_NAME = "test_static_str_value");

            let original = WithStaticStr {
                name: TEST_NAME,
                value: 42,
            };

            let mut serializer = TestingSerializer::new();
            original.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();

            let mut deserializer = TestingDeserializer::new(&bytes);
            let restored = WithStaticStr::pagable_deserialize(&mut deserializer)?;

            assert_eq!(original, restored);
            Ok(())
        }

        #[test]
        fn test_associated_type_bound() -> crate::Result<()> {
            let t1 = WithAssociatedBound::<StringItem> {
                item: "associated".to_owned(),
            };
            let mut serializer = TestingSerializer::new();
            t1.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = TestingDeserializer::new(&bytes);
            let t2 = WithAssociatedBound::<StringItem>::pagable_deserialize(&mut deserializer)?;
            assert_eq!(t2.item, "associated");
            Ok(())
        }

        #[test]
        fn test_rt2() -> crate::Result<()> {
            static_assertions::assert_impl_all!(std::sync::Arc<String>: PagableSerialize, PagableDeserialize<'static>);
            static_assertions::assert_impl_all!(std::sync::Arc<std::sync::Arc<String>>: PagableSerialize, PagableDeserialize<'static>);
            let t1: std::sync::Arc<std::sync::Arc<String>> =
                std::sync::Arc::new(std::sync::Arc::new("string".to_owned()));
            let mut serializer = TestingSerializer::new();
            t1.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();
            let mut deserializer = TestingDeserializer::new(&bytes);
            let t2 =
                <std::sync::Arc<std::sync::Arc<String>>>::pagable_deserialize(&mut deserializer)?;
            assert_eq!(t1, t2);
            Ok(())
        }

        #[test]
        fn test_arc_identity_preserved() -> crate::Result<()> {
            use std::sync::Arc;

            // Create a shared arc
            let shared: Arc<String> = Arc::new("shared".to_owned());

            // Create a vec with the same arc twice
            let vec: Vec<Arc<String>> = vec![shared.clone(), shared.clone()];

            // Verify they point to the same allocation
            assert!(Arc::ptr_eq(&vec[0], &vec[1]));

            // Serialize
            let mut serializer = TestingSerializer::new();
            vec.pagable_serialize(&mut serializer)?;
            let bytes = serializer.finish();

            // Deserialize
            let mut deserializer = TestingDeserializer::new(&bytes);
            let restored: Vec<Arc<String>> = Vec::pagable_deserialize(&mut deserializer)?;

            // Verify the deserialized arcs point to the same allocation
            assert_eq!(restored.len(), 2);
            assert!(
                Arc::ptr_eq(&restored[0], &restored[1]),
                "Arc identity should be preserved across serialization"
            );
            assert_eq!(*restored[0], "shared");

            Ok(())
        }

        // Tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll.
        #[cfg(any(not(miri), target_os = "linux"))]
        #[tokio::test]
        async fn test_pagable_arc_refcounts() -> anyhow::Result<()> {
            let storage =
                PagableStorageHandle::new(std::sync::Arc::new(EmptyPagableStorage::new()));

            let arc1 = PinnedPagableArc::new("hello world".to_owned(), storage.clone() as _);
            let weak1 = PinnedPagableArc::into_pagable(arc1.clone());
            assert_eq!(weak1.pinned_count(), 2);

            weak1.unpin();

            assert_eq!(weak1.pinned_count(), 1);
            drop(arc1);

            assert_eq!(weak1.pinned_count(), 0);
            assert!(!weak1.is_paged_out());

            {
                let arc2 = weak1.clone();
                assert_eq!(weak1.pinned_count(), 0);
                let pin1 = arc2.pin().await?;
                assert_eq!(weak1.pinned_count(), 2);

                drop(pin1);
                assert_eq!(weak1.pinned_count(), 1);
                drop(arc2);
                assert_eq!(weak1.pinned_count(), 0);
            }

            // This creates a pin and converts weak1 to pinned.
            let pinned = weak1.pin().await?;
            assert_eq!(weak1.pinned_count(), 2);
            Ok(())
        }

        // Tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll.
        #[cfg(any(not(miri), target_os = "linux"))]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn test_pagable_arc_paging() -> anyhow::Result<()> {
            let mut backing_storage = InMemoryPagableStorage::new();
            let storage = PagableStorageHandle::new(backing_storage.handle());

            let pagable = PagableArc::new("hello world".to_owned(), storage.clone());
            pagable.unpin();

            assert_eq!(backing_storage.pending_paging_count(), 1);
            assert!(!pagable.is_paged_out());

            backing_storage.page_out_pending();

            assert!(pagable.is_paged_out());
            assert!(pagable.get_data_key().is_some());

            let key = pagable.get_data_key().unwrap();
            let deserialized: String = storage.deserialize_pagable_data(&key).await.unwrap();
            let arc2 = PinnedPagableArc::new(deserialized, storage.clone());

            let arc1 = pagable.pin().await?;
            assert!(pagable.is_pinned());
            assert!(!pagable.is_paged_out());

            assert_eq!(*arc1, "hello world");
            assert_eq!(&arc1, &arc2);
            Ok(())
        }

        // Tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll.
        #[cfg(any(not(miri), target_os = "linux"))]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn test_pagable_arc_shared_arc() -> anyhow::Result<()> {
            let mut backing_storage = InMemoryPagableStorage::new();
            let storage = PagableStorageHandle::new(backing_storage.handle());

            let inner_std_arc = std::sync::Arc::new("hello world".to_owned());
            let pagable1 = PagableArc::new(inner_std_arc.dupe(), storage.clone());
            let pagable2 = PagableArc::new(inner_std_arc.dupe(), storage.clone());
            pagable1.unpin();
            pagable2.unpin();

            backing_storage.page_out_pending();

            assert!(pagable1.is_paged_out());
            assert!(pagable2.is_paged_out());

            let strong1 = pagable1.pin().await?;
            let strong2 = pagable2.pin().await?;

            assert!(!PinnedPagableArc::ptr_eq(&strong1, &strong2));
            assert!(std::sync::Arc::ptr_eq(&*strong1, &*strong2));
            Ok(())
        }

        // With panic=abort the panic kills the in-process Miri harness.
        #[cfg(not(all(miri, panic = "abort")))]
        #[test]
        #[should_panic]
        fn test_panic_on_serialize() {
            let t = TestPanic(());
            let mut serializer = TestingSerializer::new();
            t.pagable_serialize(&mut serializer).unwrap();
        }

        // With panic=abort the panic kills the in-process Miri harness.
        #[cfg(not(all(miri, panic = "abort")))]
        #[test]
        #[should_panic]
        fn test_panic_on_deserialize() {
            let mut deserializer = TestingDeserializer::new(&[]);
            let _ = TestPanic::<()>::pagable_deserialize(&mut deserializer).unwrap();
        }

        // Tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll.
        #[cfg(any(not(miri), target_os = "linux"))]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn test_pagable_arc() -> anyhow::Result<()> {
            let mut backing_storage = InMemoryPagableStorage::new();
            let storage = PagableStorageHandle::new(backing_storage.handle());

            #[derive(Pagable, Eq, PartialEq, Debug)]
            struct UNodeData(usize);

            #[derive(Pagable, Debug, Clone, Dupe)]
            struct UNode(PagableArc<UNodeData>);
            impl UNode {
                fn new(x: usize, storage: PagableStorageHandle) -> Self {
                    Self(PagableArc::new(UNodeData(x), storage))
                }
            }

            #[derive(Pagable, Debug, Clone, Dupe)]
            struct EvalResult(PagableArc<Vec<UNode>>);
            impl EvalResult {
                fn new(x: Vec<UNode>, storage: PagableStorageHandle) -> Self {
                    Self(PagableArc::new(x, storage))
                }
            }

            #[derive(Pagable, Debug)]
            struct CfgNodeData {
                node: PinnedPagableArc<UNodeData>,
                deps: Vec<CfgNode>,
            }

            #[derive(Pagable, Debug, Clone, Dupe)]
            struct CfgNode(PagableArc<CfgNodeData>);

            impl CfgNode {
                fn new(node: UNode, deps: Vec<CfgNode>, storage: PagableStorageHandle) -> Self {
                    Self(PagableArc::new(
                        CfgNodeData {
                            node: node.0.pin_sync().unwrap(),
                            deps,
                        },
                        storage,
                    ))
                }
            }

            let node1 = UNode::new(1, storage.dupe());
            let node2 = UNode::new(2, storage.dupe());
            let node3 = UNode::new(3, storage.dupe());

            let weak_node1 = node1.0.dupe();
            let weak_node2 = node2.0.dupe();
            let weak_node3 = node3.0.dupe();

            weak_node1.unpin();
            weak_node2.unpin();
            weak_node3.unpin();

            let eval_result = EvalResult::new(
                vec![node1.dupe(), node2.dupe(), node3.dupe()],
                storage.dupe(),
            );
            let cfgnode1 = CfgNode::new(node1.dupe(), vec![], storage.dupe());
            let cfgnode2 = CfgNode::new(node2.dupe(), vec![cfgnode1.dupe()], storage.dupe());
            let cfgnode3 = CfgNode::new(
                node3.dupe(),
                vec![cfgnode1.dupe(), cfgnode2.dupe()],
                storage.dupe(),
            );

            eprintln!("node1: {}", node1.0.identity());
            eprintln!("node2: {}", node2.0.identity());
            eprintln!("node3: {}", node3.0.identity());
            eprintln!("eval_result: {}", eval_result.0.identity());
            eprintln!("cfgnode1: {}", cfgnode1.0.identity());
            eprintln!("cfgnode2: {}", cfgnode2.0.identity());
            eprintln!("cfgnode3: {}", cfgnode3.0.identity());

            drop(node1);
            drop(node2);
            drop(node3);

            backing_storage.page_out_pending();

            eval_result.0.unpin();
            cfgnode1.0.unpin();
            cfgnode2.0.unpin();
            cfgnode3.0.unpin();

            backing_storage.page_out_pending();

            assert!(cfgnode1.0.get_data_key().is_some());
            assert!(cfgnode2.0.get_data_key().is_some());
            assert!(cfgnode3.0.get_data_key().is_some());
            assert!(eval_result.0.get_data_key().is_some());
            assert!(weak_node1.get_data_key().is_some());
            assert!(weak_node2.get_data_key().is_some());
            assert!(weak_node3.get_data_key().is_some());

            assert!(cfgnode1.0.is_paged_out());
            assert!(cfgnode2.0.is_paged_out());
            assert!(cfgnode3.0.is_paged_out());
            assert!(eval_result.0.is_paged_out());
            assert!(weak_node1.is_paged_out());
            assert!(weak_node2.is_paged_out());
            assert!(weak_node3.is_paged_out());

            eprintln!("upgrading cfgnode1");
            assert!(!cfgnode1.0.is_pinned());
            let strong_cfgnode1 = cfgnode1.0.pin().await?;
            assert!(cfgnode1.0.is_pinned());

            eprintln!("upgrading eval_result");
            let strong_eval_result = eval_result.0.pin().await?;

            assert!(!eval_result.0.is_paged_out());

            assert_eq!((*strong_eval_result).len(), 3);

            // Page-in hands back the allocations the `weak_node*` handles still
            // refer to, and nothing pins them: `cfgnode1` pins node1 through a
            // `PinnedPagableArc`, a different type view with its own cache entry.
            assert_eq!(strong_eval_result[0].0.identity(), weak_node1.identity());
            assert_eq!(strong_eval_result[1].0.identity(), weak_node2.identity());
            assert_eq!(strong_eval_result[2].0.identity(), weak_node3.identity());
            assert!(strong_eval_result[0].0.is_paged_out());
            assert!(strong_eval_result[1].0.is_paged_out());
            assert!(strong_eval_result[2].0.is_paged_out());
            assert!(!strong_eval_result[0].0.is_pinned());
            assert!(!strong_eval_result[1].0.is_pinned());

            let strong_node2 = strong_eval_result[1].0.pin().await?;
            assert!(strong_eval_result[1].0.is_pinned());
            assert!(!strong_eval_result[1].0.is_paged_out());

            assert!(!cfgnode3.0.is_pinned());

            let strong_cfgnode3 = cfgnode3.0.pin().await?;
            assert!(cfgnode3.0.is_pinned());

            Ok(())
        }
    }
}

mod asserts {
    use crate::traits::Pagable;
    use crate::traits::PagableDeserializeOwned;
    use crate::traits::PagableSerialize;

    static_assertions::assert_impl_all!(super::inner::Test: PagableSerialize, PagableDeserializeOwned);
    static_assertions::assert_impl_all!(super::inner::Test: Pagable);

    struct NotPagable;
    static_assertions::assert_impl_all!(super::inner::TestPanic<()>: PagableSerialize, PagableDeserializeOwned);
    static_assertions::assert_impl_all!(super::inner::TestPanic<NotPagable>: PagableSerialize, PagableDeserializeOwned);
}
