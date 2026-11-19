/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::VecDeque;
use std::collections::hash_map::Entry;

use buck2_error::internal_error;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use starlark::values::Value;
use starlark::values::ValueIdentity;

use crate::interpreter::rule_defs::transitive_set::TransitiveSet;
use crate::interpreter::rule_defs::transitive_set::transitive_set::Node;

/// A traversal over a transitive set. Reading a node's children can fail
/// once values are paged, so items are results; an error ends the traversal.
pub trait TransitiveSetIteratorLike<'a, 'v>:
    Iterator<Item = buck2_error::Result<&'a TransitiveSet<'v>>>
where
    'v: 'a,
{
    fn values(self: Box<Self>) -> TransitiveSetValuesIterator<'a, 'v>;
}

fn children<'a, 'v>(set: &'a TransitiveSet<'v>) -> buck2_error::Result<&'a [Value<'v>]> {
    Ok(set.children.read()?)
}

fn assert_transitive_set<'v>(child: Value<'v>) -> &'v TransitiveSet<'v> {
    TransitiveSet::from_value(child)
        .ok_or_else(|| {
            internal_error!(
                "Invalid set: expected {:?}, got: {:?}",
                // FIXME(JakobDegen): ???
                std::any::type_name::<Value<'v>>(),
                child
            )
        })
        .unwrap()
}

/// Preorder depth-first traversal, visiting parent node first, then children in an unspecified
/// order that minimizes memory usage during traversal.
pub struct PreorderTransitiveSetIterator<'a, 'v> {
    stack: Vec<&'a TransitiveSet<'v>>,
    seen: BuckMutSet<ValueIdentity<'v>>,
}

impl<'a, 'v> PreorderTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    pub fn new(set: &'a TransitiveSet<'v>) -> Self {
        Self {
            stack: vec![set],
            seen: Default::default(),
        }
    }

    fn enqueue_children(&mut self, children: &'a [Value<'v>]) {
        for child in children.iter().rev() {
            let child = child.to_value();

            if self.seen.insert(child.identity()) {
                self.stack.push(assert_transitive_set(child));
            }
        }
    }
}

impl<'a, 'v> TransitiveSetIteratorLike<'a, 'v> for PreorderTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    fn values(self: Box<Self>) -> TransitiveSetValuesIterator<'a, 'v> {
        TransitiveSetValuesIterator { inner: self }
    }
}

impl<'a, 'v> Iterator for PreorderTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    type Item = buck2_error::Result<&'a TransitiveSet<'v>>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.stack.pop()?;
        match children(next) {
            Ok(children) => self.enqueue_children(children),
            Err(e) => {
                self.stack.clear();
                return Some(Err(e));
            }
        }
        Some(Ok(next))
    }
}

/// Postorder depth-first traversal, visiting children left-to-right before visiting their parent
/// node.
pub struct PostorderTransitiveSetIterator<'a, 'v> {
    stack: Vec<(&'a TransitiveSet<'v>, PostorderMark<'v>)>,
    seen: BuckMutSet<ValueIdentity<'v>>,
}

impl<'a, 'v> PostorderTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    pub fn new(set: &'a TransitiveSet<'v>) -> buck2_error::Result<Self> {
        let mut iterator = Self {
            stack: vec![(set, PostorderMark::Ready)],
            seen: Default::default(),
        };
        iterator.enqueue_children(children(set)?);
        Ok(iterator)
    }

    fn enqueue_children(&mut self, children: &'a [Value<'v>]) {
        for child in children.iter().rev() {
            self.stack.push((
                assert_transitive_set(*child),
                PostorderMark::Pending(child.identity()),
            ));
        }
    }
}

impl<'a, 'v> TransitiveSetIteratorLike<'a, 'v> for PostorderTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    fn values(self: Box<Self>) -> TransitiveSetValuesIterator<'a, 'v> {
        TransitiveSetValuesIterator { inner: self }
    }
}

enum PostorderMark<'v> {
    /// When the stack returns to this position, children have been visited.
    Ready,
    /// The stack may return to this position with some children not yet having
    /// been visited. Check `seen`.
    Pending(ValueIdentity<'v>),
}

impl<'a, 'v> Iterator for PostorderTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    type Item = buck2_error::Result<&'a TransitiveSet<'v>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.stack.pop()? {
                (tset, PostorderMark::Ready) => return Some(Ok(tset)),
                (tset, PostorderMark::Pending(identity)) => {
                    if self.seen.insert(identity) {
                        self.stack.push((tset, PostorderMark::Ready));
                        match children(tset) {
                            Ok(children) => self.enqueue_children(children),
                            Err(e) => {
                                self.stack.clear();
                                return Some(Err(e));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Topological sort order, such that nodes are visited after all nodes that have them as
/// descendants.
///
/// This is similar to the pre-order traversal, except that when nodes are shared with more than one
/// parent it is returned in the order of its last occurrence.
pub struct TopologicalTransitiveSetIterator<'a, 'v> {
    output_stack: Vec<&'a TransitiveSet<'v>>,
    instance_counts: BuckMutMap<ValueIdentity<'v>, u32>,
}

impl<'a, 'v> TopologicalTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    pub fn new(set: &'a TransitiveSet<'v>) -> buck2_error::Result<Self> {
        Ok(Self {
            output_stack: vec![set],
            instance_counts: TopologicalTransitiveSetIterator::count_instances(set)?,
        })
    }

    fn count_instances(
        set: &'a TransitiveSet<'v>,
    ) -> buck2_error::Result<BuckMutMap<ValueIdentity<'v>, u32>> {
        let mut stack = vec![set];
        let mut instance_counts = BuckMutMap::<ValueIdentity<'v>, u32>::default();

        while let Some(next) = stack.pop() {
            for child in children(next)?.iter().rev() {
                let child = child.to_value();

                match instance_counts.entry(child.identity()) {
                    Entry::Occupied(mut o) => {
                        *o.get_mut() += 1;
                    }
                    Entry::Vacant(v) => {
                        v.insert(1);
                        stack.push(assert_transitive_set(child));
                    }
                }
            }
        }

        Ok(instance_counts)
    }

    fn enqueue_children(&mut self, children: &'a [Value<'v>]) {
        for child in children.iter().rev() {
            // It's safe to unwrap since instance_counts is populated during construction and contains
            // all nodes in the tree. `unwrap()` would only fail if the tree was modified.
            let count: &mut u32 = self.instance_counts.get_mut(&child.identity()).unwrap();

            // If this fails, the tree either contains cycles or was modified after construction.
            assert!(*count > 0, "Unexpected node when traversing tree");

            if *count == 1 {
                // Push the last occurrence of the node onto the output stack.
                self.output_stack.push(assert_transitive_set(*child));
            }

            *count -= 1;
        }
    }
}

impl<'a, 'v> TransitiveSetIteratorLike<'a, 'v> for TopologicalTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    fn values(self: Box<Self>) -> TransitiveSetValuesIterator<'a, 'v> {
        TransitiveSetValuesIterator { inner: self }
    }
}

impl<'a, 'v> Iterator for TopologicalTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    type Item = buck2_error::Result<&'a TransitiveSet<'v>>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.output_stack.pop()?;
        match children(next) {
            Ok(children) => self.enqueue_children(children),
            Err(e) => {
                self.output_stack.clear();
                return Some(Err(e));
            }
        }
        Some(Ok(next))
    }
}

/// Preorder breadth-first-search (BFS), visits parent node, then eagerly visits all children
/// left-to-right before traversing to any grandchildren.
pub struct BfsTransitiveSetIterator<'a, 'v> {
    queue: VecDeque<&'a TransitiveSet<'v>>,
    seen: BuckMutSet<ValueIdentity<'v>>,
}

impl<'a, 'v> BfsTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    pub fn new(set: &'a TransitiveSet<'v>) -> Self {
        Self {
            queue: VecDeque::from(vec![set]),
            seen: Default::default(),
        }
    }

    fn enqueue_children(&mut self, children: &'a [Value<'v>]) {
        for child in children.iter() {
            if self.seen.insert(child.identity()) {
                self.queue.push_back(assert_transitive_set(*child));
            }
        }
    }
}

impl<'a, 'v> TransitiveSetIteratorLike<'a, 'v> for BfsTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    fn values(self: Box<Self>) -> TransitiveSetValuesIterator<'a, 'v> {
        TransitiveSetValuesIterator { inner: self }
    }
}

impl<'a, 'v> Iterator for BfsTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    type Item = buck2_error::Result<&'a TransitiveSet<'v>>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.queue.pop_front()?;
        match children(next) {
            Ok(children) => self.enqueue_children(children),
            Err(e) => {
                self.queue.clear();
                return Some(Err(e));
            }
        }
        Some(Ok(next))
    }
}

/// Preorder depth-first-search (DFS).
///
/// This is similar to the pre-order traversal, except that children are guaranteed to be visited
/// left-to-right.
pub struct DfsTransitiveSetIterator<'a, 'v> {
    stack: Vec<(&'a TransitiveSet<'v>, Option<ValueIdentity<'v>>)>,
    seen: BuckMutSet<ValueIdentity<'v>>,
}

impl<'a, 'v> DfsTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    pub fn new(set: &'a TransitiveSet<'v>) -> Self {
        Self {
            stack: vec![(set, None)],
            seen: Default::default(),
        }
    }
}

impl<'a, 'v> TransitiveSetIteratorLike<'a, 'v> for DfsTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    fn values(self: Box<Self>) -> TransitiveSetValuesIterator<'a, 'v> {
        TransitiveSetValuesIterator { inner: self }
    }
}

impl<'a, 'v> Iterator for DfsTransitiveSetIterator<'a, 'v>
where
    'v: 'a,
{
    type Item = buck2_error::Result<&'a TransitiveSet<'v>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (tset, identity) = self.stack.pop()?;
            if identity.is_none_or(|id| self.seen.insert(id)) {
                let children = match children(tset) {
                    Ok(children) => children,
                    Err(e) => {
                        self.stack.clear();
                        return Some(Err(e));
                    }
                };
                for child in children.iter().rev() {
                    let child = child.to_value();
                    let child_identity = child.identity();
                    if !self.seen.contains(&child_identity) {
                        self.stack
                            .push((assert_transitive_set(child), Some(child_identity)));
                    }
                }
                return Some(Ok(tset));
            }
        }
    }
}

/// An iterator over values of a TransitiveSet. Notionally a FilterMap, but defined as its own type
/// since there are a few too many lifetimes involved to make a nice `impl Iterator<...>` work
/// here.
pub struct TransitiveSetValuesIterator<'a, 'v> {
    inner: Box<dyn TransitiveSetIteratorLike<'a, 'v> + 'a>,
}

impl<'a, 'v> Iterator for TransitiveSetValuesIterator<'a, 'v>
where
    'v: 'a,
{
    type Item = buck2_error::Result<&'a Node<'v>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                Ok(next) => {
                    if let Some(node) = next.node.as_ref() {
                        return Some(Ok(node));
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}
