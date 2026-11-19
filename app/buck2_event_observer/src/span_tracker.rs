/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_core::soft_error_or_panic;
use buck2_error::internal_error;
use buck2_events::BuckEvent;
use buck2_events::span::SpanId;
use buck2_hash::BuckMutMap;
use buck2_util::exact_size_iter::IterExactSize;
use derivative::Derivative;
use dupe::Dupe;
use linked_hash_map::LinkedHashMap;

use crate::what_ran::WhatRanRelevantAction;
use crate::what_ran::WhatRanState;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = InvalidEvent)]
enum SpanTrackerError<T: SpanTrackable> {
    #[error(
        "Tried to end a child (`{child:#?}`) that did not exist for its parent (`{parent:#?}`)."
    )]
    InvalidChildRemoval { child: T, parent: T },
    #[error("Tried to start an event not associated with a span: `{0:?}.")]
    NonSpanEvent(T),
}

/// Represents a timestamp relating to event handling.
#[derive(Debug, Clone, Copy)]
pub struct EventTimestamp(pub prost_types::Timestamp);

// Manual impl because no impl for `prost_types::Timestamp`
impl Dupe for EventTimestamp {}

#[derive(Debug, Clone)]
pub struct SpanInfo<T: SpanTrackable> {
    pub event: T,
    pub start: EventTimestamp,
}

#[derive(Debug, Clone)]
struct Span<T: SpanTrackable> {
    span_id: <T as SpanTrackable>::Id,
    info: SpanInfo<T>,
    children: LinkedHashMap<<T as SpanTrackable>::Id, ()>,
    /// Whether this span or any of its children are boring. Boring spans are showed after
    /// not-boring spans in the UI.
    boringness: usize,
}

impl<T: SpanTrackable + Dupe> Span<T> {
    /// NOTE: When adding a child, this node can become boring, but not parents of this node.
    fn add_child(&mut self, span_id: <T as SpanTrackable>::Id, boring: bool, roots: &mut Roots<T>) {
        self.children.insert(span_id, ());

        if boring {
            if self.boringness == 0 {
                roots.make_boring(self.span_id);
            }

            self.boringness += 1;
        }
    }

    fn remove_child(&mut self, child: Span<T>, roots: &mut Roots<T>) -> buck2_error::Result<()> {
        let removed = self.children.remove(&child.span_id).is_some();

        if !removed {
            return Err(SpanTrackerError::InvalidChildRemoval {
                parent: self.info.event.clone(),
                child: child.info.event,
            }
            .into());
        }

        if child.boringness > 0 {
            if self.boringness == 1 {
                roots.make_not_boring(self.span_id)
            }

            self.boringness -= 1;
        }

        Ok(())
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct SpanHandle<'a, T: SpanTrackable> {
    #[derivative(Debug = "ignore")]
    tracker: &'a SpanTracker<T>,
    span: &'a Span<T>,
}

impl<'a, T: SpanTrackable> SpanHandle<'a, T> {
    pub fn info(&self) -> &SpanInfo<T> {
        &self.span.info
    }

    pub fn children<'b>(&'b self) -> impl ExactSizeIterator<Item = SpanHandle<'b, T>> + 'b
    where
        'a: 'b,
    {
        self.span.children.iter().map(move |c| {
            let span = self
                .tracker
                .all
                .get(c.0)
                .ok_or_else(|| {
                    internal_error!(
                        "Invariant violation: span `{:?}` references non-existent child `{}`",
                        self.span.info.event,
                        c.0
                    )
                })
                .unwrap();

            SpanHandle {
                span,
                tracker: self.tracker,
            }
        })
    }
}

/// Tracking list of roots, partitioned in not-boring and boring roots. The former are yielded
/// first when iterating.
///
/// Invariants:
///
/// - A given Span is never in both roots and boring_roots.
#[derive(Clone)]
pub struct Roots<T: SpanTrackable> {
    roots: LinkedHashMap<<T as SpanTrackable>::Id, RootData>,
    boring_roots: LinkedHashMap<<T as SpanTrackable>::Id, RootData>,
    dice_counts: BuckMutMap<&'static str, u64>,
}

#[derive(Clone)]
pub struct RootData {
    dice_key_type: Option<&'static str>,
}

impl RootData {
    pub fn new<T: SpanTrackable>(span: &T) -> Self {
        Self {
            dice_key_type: span.dice_key_type(),
        }
    }
}

impl<T: SpanTrackable> Default for Roots<T> {
    fn default() -> Self {
        Self {
            roots: Default::default(),
            boring_roots: Default::default(),
            dice_counts: Default::default(),
        }
    }
}

impl<T: SpanTrackable> Roots<T> {
    pub fn contains(&self, span_id: <T as SpanTrackable>::Id) -> bool {
        self.roots.contains_key(&span_id) || self.boring_roots.contains_key(&span_id)
    }

    /// Insert a span. This must be called once at most per Span ID.
    pub fn insert(&mut self, span_id: <T as SpanTrackable>::Id, boring: bool, data: RootData) {
        if let Some(dice_key_type) = data.dice_key_type {
            *self.dice_counts.entry(dice_key_type).or_default() += 1;
        }

        if boring {
            self.boring_roots.insert(span_id, data);
        } else {
            self.roots.insert(span_id, data);
        }
    }

    /// Remove a Span. It's OK if the span was never inserted.
    pub fn remove(&mut self, span_id: <T as SpanTrackable>::Id) -> Option<RootData> {
        let data = self
            .roots
            .remove(&span_id)
            .or_else(|| self.boring_roots.remove(&span_id));

        if let Some(dice_key_type) = data.as_ref().and_then(|d| d.dice_key_type) {
            // About this unwrap: the key type is never changed while the entry exists, so if we
            // delete an entry whose RootData has a key type, we must have increment it.
            match self.dice_counts.get_mut(dice_key_type) {
                Some(c) if *c > 0 => {
                    *c -= 1;
                }
                v => {
                    soft_error_or_panic!(
                        "span_tracker_dice_count_underflow",
                        buck2_error::internal_error!(
                            "Decrementing {} but it was {:?}",
                            dice_key_type,
                            v
                        )
                    );
                }
            }
        }

        data
    }

    /// If the span is currently not-boring, move it to the boring list.
    pub fn make_boring(&mut self, span_id: <T as SpanTrackable>::Id) {
        if let Some(root) = self.roots.remove(&span_id) {
            self.boring_roots.insert(span_id, root);
        }
    }

    /// If the span is currently boring, move it to the not-boring list.
    pub fn make_not_boring(&mut self, span_id: <T as SpanTrackable>::Id) {
        if let Some(root) = self.boring_roots.remove(&span_id) {
            self.roots.insert(span_id, root);
        }
    }

    pub fn len(&self) -> usize {
        self.roots.len() + self.boring_roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty() && self.boring_roots.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &<T as SpanTrackable>::Id> {
        let size = self.roots.len() + self.boring_roots.len();

        // Chain does not implement ExactSizeIterator because it may overflow. We assume that our
        // roots don't overflow.
        self.roots
            .keys()
            .chain(self.boring_roots.keys())
            .with_exact_size(size)
    }

    pub fn dice_counts(&self) -> &BuckMutMap<&'static str, u64> {
        &self.dice_counts
    }
}

/// SpanTracker tracks ongoing spans received via handle() (those are typically produced by
/// the Buck daemon). Internally, we keep track of:
///
/// - Ongoing spans that are roots. Those will be rendered on their own line in the console.
/// - All ongoing spans by id. This is used to access spans by id, such as when looking for
///   a parent span.
///
/// Internally, Spans also reference their children. It is required that SpanStart and SpanEnd
/// events for the same Span have the same parent. We guarantee that if a Span is referenced as a
/// child, it'll be found in the `all` map.
///
/// We also keep track of how many roots have ended.
#[derive(Clone)]
pub struct SpanTracker<T: SpanTrackable> {
    roots: Roots<T>,
    all: BuckMutMap<<T as SpanTrackable>::Id, Span<T>>,
    roots_completed: usize,
}

impl<T: SpanTrackable + Dupe> SpanTracker<T> {
    pub fn new() -> Self {
        Self {
            roots: Default::default(),
            all: Default::default(),
            roots_completed: 0,
        }
    }

    pub fn start_at(&mut self, event: &T) -> buck2_error::Result<()> {
        if !event.is_shown() {
            return Ok(());
        }

        let is_boring = event.is_boring();

        let span_id = event
            .span_id()
            .ok_or_else(|| SpanTrackerError::NonSpanEvent(event.dupe()))?;

        self.all.entry(span_id).or_insert_with(|| Span {
            span_id,
            info: SpanInfo {
                event: event.dupe(),
                start: event.timestamp(),
            },
            boringness: if is_boring { 1 } else { 0 },
            children: LinkedHashMap::new(),
        });

        let parent = event.parent_id().and_then(|id| self.all.get_mut(&id));

        match parent {
            Some(parent) => {
                parent.add_child(span_id, is_boring, &mut self.roots);
            }
            None => {
                self.roots.insert(span_id, is_boring, RootData::new(event));
            }
        };

        Ok(())
    }

    fn end(&mut self, event: &T) -> buck2_error::Result<()> {
        let span_id = event
            .span_id()
            .ok_or_else(|| SpanTrackerError::NonSpanEvent(event.dupe()))?;

        // NOTE: We temporarily violate the invariant that all `roots` are referenced in `all`, but
        // since we have a `&mut self`, we just need to be careful not to rely on this invariant
        // in this function.
        let removed = match self.all.remove(&span_id) {
            Some(removed) => removed,
            None => {
                return Ok(());
            }
        };

        // This event might not be a root, but we need to maintain the invariant that nothing can
        // be in `roots` if it's not in `all` so we still have to clear it. Besides, we need to
        // find out if it was indeed a root to track roots_completed.
        if self.roots.remove(span_id).is_some() {
            self.roots_completed += 1;
        } else {
            let parent = event.parent_id().and_then(|id| self.all.get_mut(&id));
            match parent {
                Some(parent) => {
                    parent.remove_child(removed, &mut self.roots)?;
                }
                None => {
                    // Likely a bug: if this node was shown (meaning it was in `all`), but didn't
                    // have a parent or its parent wasn't shown, then it should have been a root.
                }
            };
        }

        Ok(())
    }

    pub fn iter_roots(&self) -> impl ExactSizeIterator<Item = SpanHandle<'_, T>> + '_ {
        self.roots.iter().map(move |s| {
            // NOTE: This unwrap is safe because we guarantee that `roots` only references spans
            // that exist in `all`.
            let span = self
                .all
                .get(s)
                .expect("Root cannot be registered and missing from all()");

            SpanHandle {
                span,
                tracker: self,
            }
        })
    }

    pub fn roots_completed(&self) -> usize {
        self.roots_completed
    }

    /// Return false if span_tracker has been used.
    pub fn is_unused(&self) -> bool {
        self.roots.is_empty() && self.roots_completed == 0
    }

    pub fn roots_ongoing(&self) -> usize {
        self.roots.len()
    }

    pub fn roots(&self) -> &Roots<T> {
        &self.roots
    }
}

pub trait SpanTrackable: std::fmt::Debug + Send + Sync + 'static {
    type Id: std::fmt::Display + std::fmt::Debug + std::hash::Hash + Eq + PartialEq + Copy;

    fn span_id(&self) -> Option<Self::Id>;

    fn parent_id(&self) -> Option<Self::Id>;

    /// Determine whether this Span should be rendered. If this span's parent isn't shown (if any
    /// exists), then it'll be rendered as a root. Otherwise, it'll be rendered as a child.
    fn is_shown(&self) -> bool;

    fn is_boring(&self) -> bool;

    fn timestamp(&self) -> EventTimestamp;

    /// Report the DICE key type that contains this span. We use this to be able to tell how many
    /// spans we currently are reporting that map to a given DICE key type. The key types here
    /// should match the type we receive in the DiceStateSnapshot.
    fn dice_key_type(&self) -> Option<&'static str>;
}

impl SpanTrackable for BuckEvent {
    type Id = SpanId;

    fn span_id(&self) -> Option<Self::Id> {
        BuckEvent::span_id(self)
    }

    fn parent_id(&self) -> Option<Self::Id> {
        BuckEvent::parent_id(self)
    }

    fn is_shown(&self) -> bool {
        is_span_shown(self)
    }

    fn is_boring(&self) -> bool {
        use buck2_data::span_start_event::Data;

        match self.span_start_event().and_then(|span| span.data.as_ref()) {
            Some(Data::DynamicLambda(..)) => true,
            Some(Data::ExecutorStage(data)) => {
                use buck2_data::executor_stage_start::Stage;

                match data.stage.as_ref() {
                    Some(Stage::Local(stage)) => {
                        use buck2_data::local_stage::Stage;

                        matches!(
                            stage.stage.as_ref(),
                            Some(Stage::Queued(..) | Stage::AcquireLocalResource(..))
                        )
                    }
                    Some(Stage::Re(stage)) => {
                        use buck2_data::re_stage::Stage;

                        matches!(stage.stage.as_ref(), Some(Stage::Queue(..)))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn timestamp(&self) -> EventTimestamp {
        EventTimestamp(self.event().timestamp.unwrap())
    }

    fn dice_key_type(&self) -> Option<&'static str> {
        use buck2_data::span_start_event::Data;

        match self.span_start_event().and_then(|span| span.data.as_ref()) {
            Some(Data::ActionExecution(..)) => Some("BuildKey"),
            Some(Data::Analysis(..)) => Some("AnalysisKey"),
            _ => None,
        }
    }
}

impl<T: SpanTrackable> SpanTrackable for Arc<T> {
    type Id = <T as SpanTrackable>::Id;

    fn span_id(&self) -> Option<Self::Id> {
        SpanTrackable::span_id(self.as_ref())
    }

    fn parent_id(&self) -> Option<Self::Id> {
        SpanTrackable::parent_id(self.as_ref())
    }

    fn is_shown(&self) -> bool {
        SpanTrackable::is_shown(self.as_ref())
    }

    fn is_boring(&self) -> bool {
        SpanTrackable::is_boring(self.as_ref())
    }

    fn timestamp(&self) -> EventTimestamp {
        SpanTrackable::timestamp(self.as_ref())
    }

    fn dice_key_type(&self) -> Option<&'static str> {
        SpanTrackable::dice_key_type(self.as_ref())
    }
}

pub fn is_span_shown(event: &BuckEvent) -> bool {
    use buck2_data::span_start_event::Data;

    match event.span_start_event().and_then(|span| span.data.as_ref()) {
        Some(
            Data::Command(..)
            | Data::CommandCritical(..)
            | Data::Materialization(..)
            | Data::DiceCriticalSection(..)
            | Data::BxlEnsureArtifacts(..),
        ) => false,
        Some(
            Data::ActionExecution(..)
            | Data::FinalMaterialization(..)
            | Data::Analysis(..)
            | Data::AnalysisResolveQueries(..)
            | Data::Load(..)
            | Data::LoadPackage(..)
            | Data::TestDiscovery(..)
            | Data::TestRun(..)
            | Data::FileWatcher(..)
            | Data::SharedTask(..)
            | Data::CreateOutputSymlinks(..)
            | Data::InstallEventInfo(..)
            | Data::DiceStateUpdate(..)
            | Data::Fake(..)
            | Data::AnalysisStage(..)
            | Data::ExecutorStage(..)
            | Data::MatchDepFiles(..)
            | Data::CacheUpload(..)
            | Data::DepFileUpload(..)
            | Data::DiceBlockConcurrentCommand(..)
            | Data::DiceSynchronizeSection(..)
            | Data::DiceCleanup(..)
            | Data::ExclusiveCommandWait(..)
            | Data::DeferredPreparationStage(..)
            | Data::DynamicLambda(..)
            | Data::BxlExecution(..)
            | Data::BxlDiceInvocation(..)
            | Data::ReUpload(..)
            | Data::ReSubmit(..)
            | Data::ConnectToInstaller(..)
            | Data::LocalResources(..)
            | Data::ReleaseLocalResources(..)
            | Data::ActionErrorHandlerExecution(..)
            | Data::CqueryUniverseBuild(..)
            | Data::ComputeDetailedAggregatedMetrics(..),
        ) => true,
        None => false,
    }
}

pub type BuckEventSpanTracker = SpanTracker<Arc<BuckEvent>>;
pub type BuckEventSpanHandle<'a> = SpanHandle<'a, Arc<BuckEvent>>;
pub type BuckEventSpanInfo = SpanInfo<Arc<BuckEvent>>;

impl BuckEventSpanTracker {
    pub fn handle_event(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        if let Some(_start) = event.span_start_event() {
            self.start_at(event)?;
        } else if let Some(_end) = event.span_end_event() {
            self.end(event)?;
        }
        Ok(())
    }
}

impl WhatRanState for SpanTracker<Arc<BuckEvent>> {
    fn get(&self, span_id: SpanId) -> Option<WhatRanRelevantAction> {
        self.all
            .get(&span_id)
            .map(|e| e.info.event.data())
            .and_then(WhatRanRelevantAction::from_buck_data)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicI64;
    use std::sync::atomic::Ordering;

    use assert_matches::assert_matches;

    use super::*;

    #[derive(Copy, Clone, Dupe, Eq, PartialEq, Debug)]
    struct TestSpan {
        span_id: i64,
        parent_id: Option<i64>,
        boring: bool,
        dice_key_type: Option<&'static str>,
    }

    impl SpanTrackable for TestSpan {
        type Id = i64;

        fn span_id(&self) -> Option<Self::Id> {
            Some(self.span_id)
        }

        fn parent_id(&self) -> Option<Self::Id> {
            self.parent_id
        }

        fn is_shown(&self) -> bool {
            true
        }

        fn is_boring(&self) -> bool {
            self.boring
        }

        fn timestamp(&self) -> EventTimestamp {
            EventTimestamp(std::time::SystemTime::UNIX_EPOCH.into())
        }

        fn dice_key_type(&self) -> Option<&'static str> {
            self.dice_key_type
        }
    }

    impl TestSpan {
        fn new() -> Self {
            static CURR: AtomicI64 = AtomicI64::new(0);

            Self {
                span_id: CURR.fetch_add(1, Ordering::Relaxed),
                parent_id: None,
                boring: false,
                dice_key_type: None,
            }
        }

        fn parent(mut self, parent: TestSpan) -> Self {
            self.parent_id = Some(parent.span_id);
            self
        }

        fn boring(mut self) -> Self {
            self.boring = true;
            self
        }

        fn dice_key_type(mut self, dice_key_type: &'static str) -> Self {
            self.dice_key_type = Some(dice_key_type);
            self
        }
    }

    #[test]
    fn test_boring_via_self() -> buck2_error::Result<()> {
        let boring = TestSpan::new().boring();
        let not_boring = TestSpan::new();

        let mut tracker = SpanTracker::new();
        tracker.start_at(&boring)?;
        tracker.start_at(&not_boring)?;

        let mut iter = tracker.iter_roots();
        {
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, not_boring);
            });
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, boring);
            });
        }

        Ok(())
    }

    #[test]
    fn test_boring_via_child() -> buck2_error::Result<()> {
        let parent = TestSpan::new();
        let child = TestSpan::new().parent(parent).boring();

        let other = TestSpan::new();
        let other2 = TestSpan::new();

        let mut tracker = SpanTracker::new();
        tracker.start_at(&parent)?;

        {
            let mut iter = tracker.iter_roots();
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, parent);
            });
        }

        tracker.start_at(&other)?;

        {
            let mut iter = tracker.iter_roots();
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, parent);
            });
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, other);
            });
        }

        tracker.start_at(&child)?;
        {
            let mut iter = tracker.iter_roots();
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, other);
            });
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, parent);
            });
        }

        tracker.end(&child)?;
        tracker.start_at(&other2)?;
        {
            let mut iter = tracker.iter_roots();
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, other);
            });
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, parent);
            });
            assert_matches!(iter.next(), Some(hdl) => {
                assert_eq!(hdl.info().event, other2);
            });
        }

        Ok(())
    }

    #[test]
    fn test_iter_roots_len() -> buck2_error::Result<()> {
        let e1 = TestSpan::new();
        let e2 = TestSpan::new().boring();
        let e3 = TestSpan::new();

        let mut tracker = SpanTracker::new();
        tracker.start_at(&e1)?;
        tracker.start_at(&e2)?;
        tracker.start_at(&e3)?;

        {
            let mut iter = tracker.iter_roots();
            assert_eq!(iter.len(), 3);

            iter.next();
            assert_eq!(iter.len(), 2);

            iter.next();
            assert_eq!(iter.len(), 1);

            iter.next();
            assert_eq!(iter.len(), 0);

            iter.next();
            assert_eq!(iter.len(), 0);
        }

        Ok(())
    }

    #[test]
    fn test_dice_counts() -> buck2_error::Result<()> {
        let foo = TestSpan::new().dice_key_type("foo");
        let bar = TestSpan::new().dice_key_type("bar");

        let mut tracker = SpanTracker::new();
        tracker.start_at(&foo)?;
        tracker.start_at(&bar)?;

        assert_eq!(tracker.roots.dice_counts["foo"], 1);
        assert_eq!(tracker.roots.dice_counts["bar"], 1);

        tracker.end(&foo)?;
        assert_eq!(tracker.roots.dice_counts["foo"], 0);
        assert_eq!(tracker.roots.dice_counts["bar"], 1);

        tracker.end(&bar)?;
        assert_eq!(tracker.roots.dice_counts["foo"], 0);
        assert_eq!(tracker.roots.dice_counts["bar"], 0);

        Ok(())
    }
}
