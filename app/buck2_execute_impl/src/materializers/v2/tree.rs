/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The disk tree: what is on disk under buck-out, who may read or write it, and who is waiting
//! to.
//!
//! A trie over buck-out paths. A node may be a *tracked root*, a path whose content is known to
//! be on disk, held as its entry with an access time and a classification. A tracked directory
//! is one root: reads inside it resolve against the directory's contents, and a write strictly
//! inside it expands it into one root per child first, so that the rest stays tracked. A node
//! may also carry a *lease*, read or write, the requests waiting for one, and tasks left running
//! under a write that was released.
//!
//! A request is for one root and says what it wants there ([`Want`]): content the holder needs
//! to read, or a path it is about to write. Each time the request is examined, the tree answers
//! whether a content want is already satisfied (read mode) or the holder has to put the content
//! there itself (write mode). A write root is granted with its *exclusions*: the tracked roots
//! below it that already hold what the want has there. They stay tracked, everything else under
//! the root is untracked by the grant, and the holder may not touch them. So under a held write
//! root, tracked means excluded, and the invariant that ties the two halves together is: a root
//! is tracked only while nobody holds it for write, except what a write excludes, and read is
//! granted only on a match.
//!
//! Two roots conflict when they are not both reads and one is a prefix of, or equal to, the
//! other, except that a read strictly inside a write is fine when it lies on tracked, that is
//! excluded, content. A request that conflicts with a held root, or with a request that arrived
//! before it and is still waiting, waits. So a queued writer blocks readers that arrive after it
//! on its paths and cannot be starved by a stream of readers, and a request queued behind a
//! writer takes read mode once that writer has reported. A request waits only for requests that
//! arrived before it: whether two requests conflict at a root cannot change once the later one
//! has been granted, so the later one is never what the earlier one waits for.
//!
//! A grant is a [`Hold`], a token the tree alone creates, which ends only by going back into the
//! tree: [`DiskTree::report`] turns a write hold into a read hold, [`DiskTree::release_read`] and
//! [`DiskTree::release_write`] end one. Nothing here blocks, notifies or does IO. The tree is
//! owned by the materializer's processor thread, and every operation returns its [`Effects`]: the
//! grants it produced, each with the reply handle its requester left, and the change to the
//! tracked roots for the caller to mirror into sqlite. The reply handle is opaque to the tree.

use std::collections::VecDeque;
use std::fmt;

use allocative::Allocative;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use jiff::Timestamp;

use crate::materializers::deferred::artifact_tree::ArtifactClassification;
use crate::materializers::deferred::artifact_tree::ArtifactMetadata;
use crate::materializers::v2::entry::DiskEntry;
use crate::materializers::v2::task::Task;

/// One tracked root. Its entry holds plain digests (`entry.rs`): the tree keeps nothing alive in
/// the interner.
#[derive(Clone, Allocative)]
pub(crate) struct Tracked {
    pub(crate) entry: DiskEntry,
    #[allocative(skip)]
    pub(crate) last_access_time: Timestamp,
    pub(crate) classification: ArtifactClassification,
}

impl Dupe for Tracked {}

impl Tracked {
    pub(crate) fn size(&self) -> u64 {
        self.entry.size()
    }
}

impl fmt::Debug for Tracked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tracked")
            .field("entry", &self.entry)
            .field("last_access_time", &self.last_access_time)
            .field("classification", &self.classification)
            .finish()
    }
}

/// What the tree says about a path.
pub(crate) enum Lookup<'a> {
    /// Tracked at exactly this path.
    Exact(&'a Tracked),
    /// Strictly inside a tracked directory; `entry` is what that directory holds at the remaining
    /// path, `None` when it holds nothing there.
    Inside {
        root: &'a Tracked,
        entry: Option<DiskEntry>,
    },
    /// A tracked file or symlink lies strictly above the path, so nothing can be at the path.
    UnderLeaf {
        root: &'a Tracked,
    },
    Untracked,
}

/// What an operation did to the tracked roots, for the caller to mirror into sqlite and to keep
/// its totals by: `removed` are roots that no longer are (replaced, untracked, or expanded), as
/// they were, `added` the roots that now exist and did not before or changed, `touched` the roots
/// whose access time was bumped. `removed` and `added` each name a path at most once; a path in
/// both was replaced, and the removal comes first.
#[derive(Default, Debug)]
pub(crate) struct TreeChange {
    pub(crate) removed: Vec<(ProjectRelativePathBuf, Tracked)>,
    pub(crate) added: Vec<(ProjectRelativePathBuf, Tracked)>,
    pub(crate) touched: Vec<ProjectRelativePathBuf>,
}

impl TreeChange {
    /// Appends a later change to this one.
    pub(crate) fn extend(&mut self, later: TreeChange) {
        for (path, tracked) in later.removed {
            self.removed(path, tracked);
        }
        self.added.extend(later.added);
        self.touched.extend(later.touched);
    }

    /// Records that `path` stopped being a root, including when this same change made it one.
    fn removed(&mut self, path: ProjectRelativePathBuf, tracked: Tracked) {
        self.added.retain(|(added, _)| *added != path);
        self.removed.push((path, tracked));
    }
}

#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    Read,
    Write,
}

/// What a request needs at its root.
#[derive(Clone, Debug)]
pub(crate) enum Want {
    /// The holder reads this content at the root. Granted in read mode when it is tracked there,
    /// in write mode when the holder has to put it there first.
    Content(ArtifactMetadata),
    /// The holder produces whatever ends up at the root.
    Write,
    /// The holder makes this content at the root unless it is tracked there: granted in read
    /// mode when it is, in write mode over the whole root otherwise, nothing excluded, since the
    /// holder writes all of it.
    Produce(ArtifactMetadata),
}

/// A request's identity, chosen by the requester; what a waiting request is withdrawn by. One
/// request may wait at several roots under one id.
#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RequestId(pub(crate) u64);

/// A root held for reading, until it goes back into the tree.
#[must_use = "a hold ends only by going back into the tree"]
#[derive(Debug)]
pub(crate) struct ReadHold(ProjectRelativePathBuf);

/// A root held for writing, until it goes back into the tree. Nothing is tracked strictly above
/// it, and what is tracked under it is excluded from it.
#[must_use = "a hold ends only by going back into the tree"]
#[derive(Debug)]
pub(crate) struct WriteHold(ProjectRelativePathBuf);

#[must_use = "a hold ends only by going back into the tree"]
#[derive(Debug)]
pub(crate) enum Hold {
    Read(ReadHold),
    Write(WriteHold),
}

impl ReadHold {
    pub(crate) fn path(&self) -> &ProjectRelativePath {
        &self.0
    }
}

impl WriteHold {
    pub(crate) fn path(&self) -> &ProjectRelativePath {
        &self.0
    }
}

impl Hold {
    pub(crate) fn path(&self) -> &ProjectRelativePath {
        match self {
            Hold::Read(hold) => hold.path(),
            Hold::Write(hold) => hold.path(),
        }
    }
}

/// What a cleaner expects to find at a path it is about to take, from its scan. The take is
/// refused when the tree says otherwise: the path has been used or changed since.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Expect {
    Untracked,
    Tracked { last_access_time: Timestamp },
}

/// A root as granted.
#[derive(Debug)]
pub(crate) struct Grant {
    pub(crate) hold: Hold,
    /// For a write: the tracked roots below it that already hold what the want has there. The
    /// holder leaves them alone.
    pub(crate) exclusions: Vec<ProjectRelativePathBuf>,
    /// For a write: tasks that holders before this one left running on paths overlapping the
    /// root, still running unless they were already cancelled. The holder sees them to their end
    /// before it writes.
    pub(crate) tasks: Vec<Task>,
    /// For a write: what was tracked at exactly the root before the grant untracked it. `None`
    /// when nothing was, including a root that had been expanded into roots below it.
    pub(crate) displaced: Option<DiskEntry>,
}

/// What an operation did: the grants it produced, each with the reply handle its requester left,
/// in the order the requests arrived, and the change to the tracked roots.
pub(crate) struct Effects<R> {
    pub(crate) grants: Vec<(R, Grant)>,
    pub(crate) change: TreeChange,
    /// Tracked roots whose last lease the operation ended, with nothing waiting for them.
    pub(crate) freed: Vec<ProjectRelativePathBuf>,
}

impl<R> Default for Effects<R> {
    fn default() -> Self {
        Self {
            grants: Vec::new(),
            change: TreeChange::default(),
            freed: Vec::new(),
        }
    }
}

impl<R> Effects<R> {
    /// Appends the effects of a later operation.
    pub(crate) fn extend(&mut self, later: Effects<R>) {
        self.grants.extend(later.grants);
        self.change.extend(later.change);
        self.freed.extend(later.freed);
    }
}

/// Why a report was refused: a root tracked under the reported path, which the holder promised
/// to leave alone, holds something else than what the report says is there.
#[derive(Debug)]
pub(crate) struct Disagreement {
    pub(crate) root: ProjectRelativePathBuf,
}

/// A want as it resolves against the tree at one moment.
#[derive(Clone, Debug)]
struct Root {
    path: ProjectRelativePathBuf,
    mode: Mode,
    exclusions: Vec<ProjectRelativePathBuf>,
}

impl Root {
    fn excludes(&self, path: &ProjectRelativePath) -> bool {
        self.exclusions
            .iter()
            .any(|excluded| path.starts_with(excluded))
    }
}

#[derive(Debug, Clone, Copy)]
enum LeaseState {
    Read { holders: u32 },
    Write,
}

impl LeaseState {
    fn mode(self) -> Mode {
        match self {
            LeaseState::Read { .. } => Mode::Read,
            LeaseState::Write => Mode::Write,
        }
    }
}

struct Waiter<R> {
    id: RequestId,
    /// Arrival order, across the whole tree.
    ticket: u64,
    want: Want,
    reply: R,
}

/// Counts of what lies strictly below a node, so that the walks that look for it can skip
/// subtrees that have none.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct Below {
    /// Leased nodes (nodes, not holders).
    leases: u32,
    /// Write-leased nodes.
    writes: u32,
    waiting: u32,
    tasks: u32,
}

#[derive(Allocative)]
#[allocative(bound = "")]
struct Node<R> {
    children: BuckMutMap<FileNameBuf, Node<R>>,
    /// A tracked root ends here. Nothing below is tracked, a tracked directory's contents being
    /// in its entry, but nodes below may exist: they carry leases, waiting requests and tasks on
    /// paths inside the root.
    tracked: Option<Tracked>,
    /// The lease at exactly this path.
    #[allocative(skip)]
    lease: Option<LeaseState>,
    /// Requests for exactly this path, in arrival order.
    #[allocative(skip)]
    waiting: VecDeque<Waiter<R>>,
    /// A task whose root is this path and whose holder released it: handed over uncancelled to a
    /// writer of the root granted in the same step, cancelled otherwise, and kept until a writer
    /// over the path takes it to wait for its end. At most one, since a writer over the path
    /// takes every task around it before it starts its own.
    #[allocative(skip)]
    task: Option<Task>,
    #[allocative(skip)]
    below: Below,
}

impl<R> Default for Node<R> {
    fn default() -> Self {
        Self {
            children: BuckMutMap::default(),
            tracked: None,
            lease: None,
            waiting: VecDeque::new(),
            task: None,
            below: Below::default(),
        }
    }
}

impl<R> Node<R> {
    fn is_empty(&self) -> bool {
        self.tracked.is_none()
            && self.lease.is_none()
            && self.waiting.is_empty()
            && self.task.is_none()
            && self.children.is_empty()
    }

    fn get<'a>(&self, mut components: impl Iterator<Item = &'a FileName>) -> Option<&Node<R>> {
        match components.next() {
            None => Some(self),
            Some(name) => self.children.get(name)?.get(components),
        }
    }

    fn get_mut<'a>(
        &mut self,
        mut components: impl Iterator<Item = &'a FileName>,
    ) -> Option<&mut Node<R>> {
        match components.next() {
            None => Some(self),
            Some(name) => self.children.get_mut(name)?.get_mut(components),
        }
    }

    fn get_or_insert<'a>(
        &mut self,
        mut components: impl Iterator<Item = &'a FileName>,
    ) -> &mut Node<R> {
        match components.next() {
            None => self,
            Some(name) => self
                .children
                .entry(name.to_owned())
                .or_default()
                .get_or_insert(components),
        }
    }

    /// Every tracked root at or below this node, with its path relative to `at`.
    fn tracked_below(
        &self,
        at: &ProjectRelativePath,
        out: &mut Vec<(ProjectRelativePathBuf, Tracked)>,
    ) {
        if let Some(tracked) = &self.tracked {
            out.push((at.to_buf(), tracked.dupe()));
            return;
        }
        for (name, child) in &self.children {
            child.tracked_below(&at.join(name), out);
        }
    }

    /// Forgets every tracked root at or below the node, pruning nodes that become empty, and
    /// reports the removed roots with their paths relative to `at`.
    fn untrack_below(&mut self, at: &ProjectRelativePath, change: &mut TreeChange) {
        if let Some(tracked) = self.tracked.take() {
            change.removed(at.to_buf(), tracked);
        }
        self.children.retain(|name, child| {
            child.untrack_below(&at.join(name), change);
            !child.is_empty()
        });
    }

    /// Whether granting `root` conflicts with a lease held at an ancestor, at the path itself, or
    /// anywhere below it. `components` is what remains of the root's path below this node.
    fn held_conflict<'a>(
        &self,
        root: &Root,
        mut components: impl Iterator<Item = &'a FileName>,
    ) -> bool {
        match components.next() {
            Some(name) => {
                // This node is a strict ancestor of the root. A write conflicts with any lease
                // here. A read does not: it resolved to read because its content is tracked, and
                // under a held write only excluded content is tracked, which the write leaves
                // alone.
                if root.mode == Mode::Write && self.lease.is_some() {
                    return true;
                }
                match self.children.get(name) {
                    Some(child) => child.held_conflict(root, components),
                    None => false,
                }
            }
            None => {
                if let Some(held) = self.lease
                    && !(root.mode == Mode::Read && held.mode() == Mode::Read)
                {
                    return true;
                }
                match root.mode {
                    Mode::Read => self.below.writes > 0,
                    Mode::Write => {
                        self.below.leases > 0
                            && self.leased_below_outside(root, &mut root.path.to_buf())
                    }
                }
            }
        }
    }

    /// Whether a lease strictly below this node conflicts with the write `root`: any write does,
    /// a read does unless the root excludes it. `path` is this node's path and is used as scratch.
    fn leased_below_outside(&self, root: &Root, path: &mut ProjectRelativePathBuf) -> bool {
        for (name, child) in &self.children {
            path.push(name);
            let conflict = match child.lease {
                Some(LeaseState::Write) => true,
                Some(LeaseState::Read { .. }) => !root.excludes(path),
                None => false,
            } || (child.below.leases > 0 && child.leased_below_outside(root, path));
            path.pop();
            if conflict {
                return true;
            }
        }
        false
    }

    /// The tickets and paths of the waiters at or below this node, whose path is `at`.
    fn waiters_below(
        &self,
        at: &mut ProjectRelativePathBuf,
        out: &mut Vec<(u64, ProjectRelativePathBuf)>,
    ) {
        out.extend(
            self.waiting
                .iter()
                .map(|waiter| (waiter.ticket, at.clone())),
        );
        if self.below.waiting == 0 {
            return;
        }
        for (name, child) in &self.children {
            at.push(name);
            child.waiters_below(at, out);
            at.pop();
        }
    }

    /// The paths of the nodes at or below this one, whose path is `at`, that hold a task.
    fn tasks_below(&self, at: &mut ProjectRelativePathBuf, out: &mut Vec<ProjectRelativePathBuf>) {
        if self.task.is_some() {
            out.push(at.clone());
        }
        if self.below.tasks == 0 {
            return;
        }
        for (name, child) in &self.children {
            at.push(name);
            child.tasks_below(at, out);
            at.pop();
        }
    }
}

#[derive(Allocative)]
#[allocative(bound = "")]
pub(crate) struct DiskTree<R> {
    root: Node<R>,
    next_ticket: u64,
}

impl<R> DiskTree<R> {
    pub(crate) fn new() -> Self {
        Self {
            root: Node::default(),
            next_ticket: 0,
        }
    }

    // ---- tracked roots ----

    pub(crate) fn lookup(&self, path: &ProjectRelativePath) -> Lookup<'_> {
        let mut node = &self.root;
        let mut components = path.iter();
        loop {
            if let Some(root) = &node.tracked {
                let remaining = components.as_path();
                if remaining.is_empty() {
                    return Lookup::Exact(root);
                }
                return match &root.entry {
                    DiskEntry::Dir(_) => Lookup::Inside {
                        root,
                        entry: root.entry.find(remaining),
                    },
                    _ => Lookup::UnderLeaf { root },
                };
            }
            let Some(name) = components.next() else {
                return Lookup::Untracked;
            };
            match node.children.get(name) {
                Some(child) => node = child,
                None => return Lookup::Untracked,
            }
        }
    }

    /// Whether `entry` is what is tracked at `path`, as a root or inside one.
    pub(crate) fn matches(&self, path: &ProjectRelativePath, entry: &ArtifactMetadata) -> bool {
        match self.lookup(path) {
            Lookup::Exact(tracked) => tracked.entry.matches(entry),
            Lookup::Inside {
                entry: Some(inside),
                ..
            } => inside.matches(entry),
            Lookup::Inside { entry: None, .. } | Lookup::UnderLeaf { .. } | Lookup::Untracked => {
                false
            }
        }
    }

    /// Gives the root at `path` `classification`, as a removal and an addition so that whatever
    /// mirrors the tree follows; nothing when `path` is not a root or is so classified already.
    pub(crate) fn promote(
        &mut self,
        path: &ProjectRelativePath,
        classification: ArtifactClassification,
    ) -> Option<TreeChange> {
        let tracked = self.root.get_mut(path.iter())?.tracked.as_mut()?;
        if tracked.classification == classification {
            return None;
        }
        let before = tracked.dupe();
        tracked.classification = classification;
        let after = tracked.dupe();
        let mut change = TreeChange::default();
        change.removed(path.to_buf(), before);
        change.added.push((path.to_buf(), after));
        Some(change)
    }

    /// Makes `path` a tracked root holding `node`, in a tree that holds no lease: what sqlite
    /// remembered from before a restart. Roots do not nest: a row that would nest with one loaded
    /// before it is refused, which is what a damaged table looks like.
    #[must_use = "a refused row is not tracked"]
    pub(crate) fn load(&mut self, path: ProjectRelativePathBuf, node: Tracked) -> bool {
        match self.lookup(&path) {
            Lookup::Untracked => {}
            Lookup::Exact(_) | Lookup::Inside { .. } | Lookup::UnderLeaf { .. } => return false,
        }
        let slot = self.root.get_or_insert(path.iter());
        if !slot.children.is_empty() {
            return false;
        }
        slot.tracked = Some(node);
        true
    }

    /// Forgets whatever is tracked at or below `path`, except the roots in `keep`. Tracked
    /// directories strictly above `path` are expanded first, so that their other children stay
    /// tracked; a tracked file or symlink strictly above `path` is forgotten, since it is in the
    /// way of whatever is about to be at `path`.
    fn untrack_except(
        &mut self,
        path: &ProjectRelativePath,
        keep: &[ProjectRelativePathBuf],
    ) -> TreeChange {
        let mut change = TreeChange::default();
        self.expand_above(path, &mut change);
        if keep.is_empty() {
            if let Some(node) = self.root.get_mut(path.iter()) {
                node.untrack_below(path, &mut change);
            }
        } else {
            let roots: Vec<ProjectRelativePathBuf> = self
                .roots_at_or_below(path)
                .into_iter()
                .map(|(root, _)| root)
                .filter(|root| !keep.contains(root))
                .collect();
            for root in roots {
                if let Some(node) = self.root.get_mut(root.iter()) {
                    node.untrack_below(&root, &mut change);
                }
                self.prune(&root);
            }
        }
        self.prune(path);
        change
    }

    /// Bumps the access time of the root holding `path`, if any, and returns that root's path.
    pub(crate) fn touch(
        &mut self,
        path: &ProjectRelativePath,
        now: Timestamp,
    ) -> Option<ProjectRelativePathBuf> {
        let mut node = &mut self.root;
        let mut components = path.iter();
        let mut at = ProjectRelativePathBuf::unchecked_new(String::new());
        loop {
            if let Some(tracked) = &mut node.tracked {
                tracked.last_access_time = now;
                return Some(at);
            }
            let name = components.next()?;
            node = node.children.get_mut(name)?;
            at.push(name);
        }
    }

    /// Every tracked root, in no particular order.
    pub(crate) fn iter(&self) -> Vec<(ProjectRelativePathBuf, Tracked)> {
        self.roots_at_or_below(ProjectRelativePath::empty())
    }

    /// Every tracked root at or below `path`, with its path. Empty when `path` lies inside a
    /// root: that root is above it, not below.
    pub(crate) fn roots_at_or_below(
        &self,
        path: &ProjectRelativePath,
    ) -> Vec<(ProjectRelativePathBuf, Tracked)> {
        let mut out = Vec::new();
        match self.lookup(path) {
            Lookup::Exact(tracked) => out.push((path.to_buf(), tracked.dupe())),
            Lookup::Inside { .. } | Lookup::UnderLeaf { .. } => {}
            Lookup::Untracked => {
                if let Some(node) = self.root.get(path.iter()) {
                    node.tracked_below(path, &mut out);
                }
            }
        }
        out
    }

    /// Replaces every tracked directory strictly above `path` by one root per child, and forgets
    /// a tracked file or symlink strictly above it, so that afterwards no root lies strictly
    /// above `path`.
    fn expand_above(&mut self, path: &ProjectRelativePath, change: &mut TreeChange) {
        // Each round expands one level; the root's child on the way to `path` may itself be a
        // directory that needs expanding.
        loop {
            let (root_path, root) = match self.lookup(path) {
                Lookup::Inside { root, .. } | Lookup::UnderLeaf { root } => {
                    let root = root.dupe();
                    let root_path = self.root_above(path);
                    (root_path, root)
                }
                Lookup::Exact(_) | Lookup::Untracked => return,
            };
            let node = self
                .root
                .get_mut(root_path.iter())
                .expect("the root was just found");
            node.tracked = None;
            change.removed(root_path.clone(), root.dupe());
            if let DiskEntry::Dir(dir) = &root.entry {
                for (name, entry) in dir.entries() {
                    let child_path = root_path.join(name);
                    let child = Tracked {
                        entry: entry.dupe(),
                        last_access_time: root.last_access_time,
                        classification: root.classification,
                    };
                    self.root.get_or_insert(child_path.iter()).tracked = Some(child.dupe());
                    change.added.push((child_path, child));
                }
            }
        }
    }

    /// The path of the tracked root strictly above `path`. Only valid when `lookup` says `Inside`
    /// or `UnderLeaf`.
    fn root_above(&self, path: &ProjectRelativePath) -> ProjectRelativePathBuf {
        let mut node = &self.root;
        let mut at = ProjectRelativePathBuf::unchecked_new(String::new());
        for name in path.iter() {
            if node.tracked.is_some() {
                return at;
            }
            node = &node.children[name];
            at.push(name);
        }
        at
    }

    /// Drops empty nodes on the way from `path` up to the root.
    fn prune(&mut self, path: &ProjectRelativePath) {
        fn go<'a, R>(node: &mut Node<R>, mut components: impl Iterator<Item = &'a FileName>) {
            if let Some(name) = components.next()
                && let Some(child) = node.children.get_mut(name)
            {
                go(child, components);
                if child.is_empty() {
                    node.children.remove(name);
                }
            }
        }
        go(&mut self.root, path.iter());
    }

    /// Applies `f` to the counters of every strict ancestor of `path`, whose nodes must exist.
    fn adjust_below(&mut self, path: &ProjectRelativePath, f: impl Fn(&mut Below)) {
        let mut node = &mut self.root;
        for name in path.iter() {
            f(&mut node.below);
            node = node
                .children
                .get_mut(name)
                .expect("counters are adjusted along paths whose nodes exist");
        }
    }

    // ---- leases ----

    /// Asks for `want` at `path`. Granted at once when nothing held and no earlier waiting
    /// request conflicts with it; otherwise the request waits, and a later release, report or
    /// cancellation hands it out. Either way the grant comes back through the effects, with
    /// `reply`. A request that asks for several roots asks for them in one go, so that between
    /// any two requests the arrival order is the same at every root.
    pub(crate) fn acquire(
        &mut self,
        path: ProjectRelativePathBuf,
        want: Want,
        id: RequestId,
        reply: R,
    ) -> Effects<R> {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let root = self.resolve(&path, &want);
        let mut effects = Effects::default();
        if self.grantable(&root, ticket) {
            let grant = self.grant(root, &mut effects.change);
            effects.grants.push((reply, grant));
        } else {
            self.root
                .get_or_insert(path.iter())
                .waiting
                .push_back(Waiter {
                    id,
                    ticket,
                    want,
                    reply,
                });
            self.adjust_below(&path, |below| below.waiting += 1);
        }
        effects
    }

    /// Ends a read hold.
    pub(crate) fn release_read(&mut self, hold: ReadHold) -> Effects<R> {
        let ReadHold(path) = hold;
        self.remove_lease(&path);
        let mut effects = Effects::default();
        self.grant_waiting(&path, &mut effects);
        if self.is_freed(&path) {
            effects.freed.push(path);
        }
        effects
    }

    /// Whether `path` is a tracked root that nothing holds or waits for.
    fn is_freed(&self, path: &ProjectRelativePath) -> bool {
        self.root.get(path.iter()).is_some_and(|node| {
            node.tracked.is_some() && node.lease.is_none() && node.waiting.is_empty()
        })
    }

    /// Ends a write hold, filing the tasks the holder hands back under their roots. A writer of
    /// such a root granted in this same step takes the tasks along, still running; tasks nobody
    /// takes are cancelled, and stay where they are until a writer over their root takes them to
    /// wait for their end.
    pub(crate) fn release_write(&mut self, hold: WriteHold, tasks: Vec<Task>) -> Effects<R> {
        let WriteHold(path) = hold;
        self.remove_lease(&path);
        let roots: Vec<ProjectRelativePathBuf> =
            tasks.iter().map(|task| task.root().to_buf()).collect();
        for task in tasks {
            self.file_task(task);
        }
        let mut effects = Effects::default();
        self.grant_waiting(&path, &mut effects);
        for root in roots {
            if let Some(task) = self
                .root
                .get_mut(root.iter())
                .and_then(|node| node.task.as_mut())
            {
                task.cancel();
            }
        }
        if self.is_freed(&path) {
            effects.freed.push(path);
        }
        effects
    }

    /// Records that the holder put `node` at its root, which it holds for reading from here on:
    /// the content is final and readers may share it. Tracking under a held write happens here
    /// and nowhere else, which is what makes "tracked under a write" mean "excluded from it"
    /// everywhere the conflict rules look; the roots tracked under the path are what the holder
    /// promised to leave alone, so the report must agree with them, or it is refused and the
    /// hold stays a write.
    pub(crate) fn report(
        &mut self,
        hold: WriteHold,
        node: Tracked,
    ) -> Result<(ReadHold, Effects<R>), (WriteHold, Disagreement)> {
        let path = hold.path();
        match self.lookup(path) {
            Lookup::Untracked => {}
            Lookup::Exact(_) | Lookup::Inside { .. } | Lookup::UnderLeaf { .. } => {
                panic!("lease invariant broken: `{path}` is tracked while held for writing")
            }
        }
        for (root, tracked) in self.roots_at_or_below(path) {
            let below = root
                .strip_prefix(path)
                .expect("a root below the path starts with it");
            let agrees = node
                .entry
                .find(below)
                .is_some_and(|wanted| tracked.entry.same_content(&wanted));
            if !agrees {
                return Err((hold, Disagreement { root }));
            }
        }
        let WriteHold(path) = hold;
        let mut effects = Effects::default();
        let slot = self.root.get_or_insert(path.iter());
        slot.untrack_below(&path, &mut effects.change);
        slot.tracked = Some(node.dupe());
        effects.change.added.push((path.clone(), node));
        self.downgrade_lease(&path);
        self.grant_waiting(&path, &mut effects);
        Ok((ReadHold(path), effects))
    }

    /// Takes `path` for writing on behalf of the cleaner, if it is as `expect` says and nothing
    /// held or waiting conflicts. Never waits, and never jumps ahead of a waiting request, so the
    /// cleaner is no party to any wait. The grant untracks the root, as any write does.
    pub(crate) fn take_for_cleaning(
        &mut self,
        path: ProjectRelativePathBuf,
        expect: Expect,
    ) -> Option<(Grant, TreeChange)> {
        let as_expected = match (self.lookup(&path), expect) {
            (Lookup::Untracked, Expect::Untracked) => !self.contains_roots_below(&path),
            (Lookup::Exact(tracked), Expect::Tracked { last_access_time }) => {
                tracked.last_access_time == last_access_time
            }
            _ => false,
        };
        if !as_expected {
            return None;
        }
        let root = Root {
            path,
            mode: Mode::Write,
            exclusions: Vec::new(),
        };
        if !self.grantable(&root, self.next_ticket) {
            return None;
        }
        let mut change = TreeChange::default();
        let grant = self.grant(root, &mut change);
        Some((grant, change))
    }

    /// Holds `path` for reading if content is tracked there and nothing held or waiting
    /// conflicts; never waits, and does not count as a use, so that the cleaner can tell
    /// afterwards whether anyone else used the root, and so that lending a root's content to
    /// another path does not keep the root alive. With `as_of`, the root must be tracked with
    /// exactly that access time; without, any tracked content will do, at a root or inside one.
    pub(crate) fn hold_for_reading(
        &mut self,
        path: ProjectRelativePathBuf,
        as_of: Option<Timestamp>,
    ) -> Option<ReadHold> {
        match (self.lookup(&path), as_of) {
            (Lookup::Exact(tracked), Some(at)) if tracked.last_access_time == at => {}
            (Lookup::Exact(_), None) | (Lookup::Inside { entry: Some(_), .. }, None) => {}
            _ => return None,
        }
        let root = Root {
            path,
            mode: Mode::Read,
            exclusions: Vec::new(),
        };
        if !self.grantable(&root, self.next_ticket) {
            return None;
        }
        self.insert_lease(&root.path, Mode::Read);
        Some(ReadHold(root.path))
    }

    /// Whether any root is tracked at or below `path`.
    pub(crate) fn contains_roots_below(&self, path: &ProjectRelativePath) -> bool {
        fn any_tracked<R>(node: &Node<R>) -> bool {
            node.tracked.is_some() || node.children.values().any(any_tracked)
        }
        match self.lookup(path) {
            Lookup::Exact(_) => true,
            Lookup::Inside { .. } | Lookup::UnderLeaf { .. } => false,
            Lookup::Untracked => self.root.get(path.iter()).is_some_and(any_tracked),
        }
    }

    /// Withdraws a waiting request at `path`. Nothing happens if it is not waiting there: never
    /// queued, or already granted, in which case the grant was handed out through the effects of
    /// whatever granted it.
    pub(crate) fn cancel(&mut self, path: &ProjectRelativePath, id: RequestId) -> Effects<R> {
        let mut effects = Effects::default();
        let Some(node) = self.root.get_mut(path.iter()) else {
            return effects;
        };
        let Some(position) = node.waiting.iter().position(|waiter| waiter.id == id) else {
            return effects;
        };
        node.waiting.remove(position);
        self.adjust_below(path, |below| below.waiting -= 1);
        self.prune(path);
        self.grant_waiting(path, &mut effects);
        effects
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        let mut out = Vec::new();
        self.root.waiters_below(
            &mut ProjectRelativePathBuf::unchecked_new(String::new()),
            &mut out,
        );
        out.len()
    }

    /// Nothing held, nothing waiting, no task left anywhere.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.root.lease.is_none()
            && self.root.waiting.is_empty()
            && self.root.task.is_none()
            && self.root.below == Below::default()
    }

    /// The mode and exclusions `want` would be granted with right now.
    fn resolve(&self, path: &ProjectRelativePath, want: &Want) -> Root {
        let (mode, exclusions) = match want {
            Want::Write => (Mode::Write, Vec::new()),
            Want::Content(entry) => {
                if self.matches(path, entry) {
                    (Mode::Read, Vec::new())
                } else {
                    (Mode::Write, self.exclusions(path, entry))
                }
            }
            Want::Produce(entry) => {
                if self.matches(path, entry) {
                    (Mode::Read, Vec::new())
                } else {
                    (Mode::Write, Vec::new())
                }
            }
        };
        Root {
            path: path.to_buf(),
            mode,
            exclusions,
        }
    }

    /// The tracked roots strictly below `path` that hold what `entry` has at their relative path.
    fn exclusions(
        &self,
        path: &ProjectRelativePath,
        entry: &ArtifactMetadata,
    ) -> Vec<ProjectRelativePathBuf> {
        let DirectoryEntry::Dir(dir) = entry else {
            return Vec::new();
        };
        self.roots_at_or_below(path)
            .into_iter()
            .filter_map(|(root, tracked)| {
                let below = root.strip_prefix(path).ok()?;
                if below.is_empty() {
                    return None;
                }
                let wanted = find_in(dir, below)?;
                tracked.entry.matches(&wanted).then_some(root)
            })
            .collect()
    }

    /// Whether `root` may be granted now to a request with `ticket`: nothing held and no request
    /// that arrived before it and is still waiting conflicts with it.
    fn grantable(&self, root: &Root, ticket: u64) -> bool {
        !self.root.held_conflict(root, root.path.iter()) && !self.waiting_conflict(root, ticket)
    }

    /// Whether a request waiting at an ancestor of the root, at its path, or anywhere below it,
    /// that arrived before `ticket`, conflicts with `root`. Each is resolved as the tree stands.
    fn waiting_conflict(&self, root: &Root, ticket: u64) -> bool {
        self.waiters_around(&root.path)
            .into_iter()
            .take_while(|(earlier, _)| *earlier < ticket)
            .any(|(earlier, path)| {
                let waiter = self
                    .root
                    .get(path.iter())
                    .and_then(|node| node.waiting.iter().find(|waiter| waiter.ticket == earlier))
                    .expect("the waiter was just found there");
                conflicts(root, &self.resolve(&path, &waiter.want))
            })
    }

    /// The tickets and paths of every request waiting at an ancestor of `path`, at `path`, or
    /// below it: the requests whose fate a change at `path` can affect. In arrival order.
    fn waiters_around(&self, path: &ProjectRelativePath) -> Vec<(u64, ProjectRelativePathBuf)> {
        let mut out = Vec::new();
        let mut node = &self.root;
        let mut at = ProjectRelativePathBuf::unchecked_new(String::new());
        for name in path.iter() {
            out.extend(
                node.waiting
                    .iter()
                    .map(|waiter| (waiter.ticket, at.clone())),
            );
            match node.children.get(name) {
                Some(child) => {
                    node = child;
                    at.push(name);
                }
                None => {
                    out.sort();
                    return out;
                }
            }
        }
        node.waiters_below(&mut at, &mut out);
        out.sort();
        out
    }

    /// Records the lease. A write untracks what the holder is about to replace and takes along
    /// the tasks running on overlapping paths; a read bumps the access time.
    fn grant(&mut self, root: Root, change: &mut TreeChange) -> Grant {
        self.insert_lease(&root.path, root.mode);
        let (hold, tasks, displaced) = match root.mode {
            Mode::Read => {
                if let Some(touched) = self.touch(&root.path, Timestamp::now()) {
                    change.touched.push(touched);
                }
                (Hold::Read(ReadHold(root.path)), Vec::new(), None)
            }
            Mode::Write => {
                let untracked = self.untrack_except(&root.path, &root.exclusions);
                let displaced = untracked
                    .removed
                    .iter()
                    .find(|(path, _)| *path == root.path)
                    .map(|(_, tracked)| tracked.entry.dupe());
                change.extend(untracked);
                let tasks = self.take_tasks_around(&root.path);
                (Hold::Write(WriteHold(root.path)), tasks, displaced)
            }
        };
        Grant {
            hold,
            exclusions: root.exclusions,
            tasks,
            displaced,
        }
    }

    /// Grants every request waiting around `path` that, as the tree now stands, no longer
    /// conflicts with anything held or with a request that arrived before it, in arrival order.
    /// Only requests around `path` can have been unblocked by a change there, and a grant made
    /// here blocks exactly what the waiting request blocked, so nothing further away changes.
    fn grant_waiting(&mut self, path: &ProjectRelativePath, effects: &mut Effects<R>) {
        for (ticket, at) in self.waiters_around(path) {
            let want = self
                .root
                .get(at.iter())
                .and_then(|node| node.waiting.iter().find(|waiter| waiter.ticket == ticket))
                .map(|waiter| waiter.want.clone())
                .expect("the waiter was just found there");
            // Resolved against the tree as the grants before it in this pass left it, so that a
            // request behind one that untracked a root sees the root gone.
            let root = self.resolve(&at, &want);
            if !self.grantable(&root, ticket) {
                continue;
            }
            let node = self
                .root
                .get_mut(at.iter())
                .expect("the waiter was just found there");
            let position = node
                .waiting
                .iter()
                .position(|waiter| waiter.ticket == ticket)
                .expect("the waiter was just found there");
            let waiter = node.waiting.remove(position).expect("position is in range");
            self.adjust_below(&at, |below| below.waiting -= 1);
            let grant = self.grant(root, &mut effects.change);
            effects.grants.push((waiter.reply, grant));
        }
    }

    /// Takes every task at an ancestor of `path`, at `path`, or below it.
    fn take_tasks_around(&mut self, path: &ProjectRelativePath) -> Vec<Task> {
        let mut holders = Vec::new();
        let mut node = &self.root;
        let mut at = ProjectRelativePathBuf::unchecked_new(String::new());
        let mut reached = true;
        for name in path.iter() {
            if node.task.is_some() {
                holders.push(at.clone());
            }
            match node.children.get(name) {
                Some(child) => {
                    node = child;
                    at.push(name);
                }
                None => {
                    reached = false;
                    break;
                }
            }
        }
        if reached {
            node.tasks_below(&mut at, &mut holders);
        }
        let mut taken = Vec::new();
        for holder in holders {
            let node = self
                .root
                .get_mut(holder.iter())
                .expect("the task was just found there");
            taken.push(node.task.take().expect("the task was just found there"));
            self.adjust_below(&holder, |below| below.tasks -= 1);
            self.prune(&holder);
        }
        taken
    }

    /// Parks a handed-back task at its root. Nothing can be there: the holder handing it back
    /// took every task around its root when it was granted.
    fn file_task(&mut self, task: Task) {
        let root = task.root().to_buf();
        let node = self.root.get_or_insert(root.iter());
        assert!(
            node.task.is_none(),
            "lease invariant broken: two tasks handed back for `{root}`"
        );
        node.task = Some(task);
        self.adjust_below(&root, |below| below.tasks += 1);
    }

    /// Records a lease at the path; the caller has established that it conflicts with nothing.
    fn insert_lease(&mut self, path: &ProjectRelativePath, mode: Mode) {
        let node = self.root.get_or_insert(path.iter());
        let (state, newly_leased) = match (node.lease, mode) {
            (None, Mode::Read) => (LeaseState::Read { holders: 1 }, true),
            (Some(LeaseState::Read { holders }), Mode::Read) => (
                LeaseState::Read {
                    holders: holders + 1,
                },
                false,
            ),
            (None, Mode::Write) => (LeaseState::Write, true),
            (Some(held), requested) => {
                panic!("lease invariant broken: granting {requested:?} over {held:?}")
            }
        };
        node.lease = Some(state);
        if newly_leased {
            self.adjust_below(path, |below| {
                below.leases += 1;
                if mode == Mode::Write {
                    below.writes += 1;
                }
            });
        }
    }

    /// Removes one hold at the path, pruning nodes that become empty.
    fn remove_lease(&mut self, path: &ProjectRelativePath) {
        let node = self
            .root
            .get_mut(path.iter())
            .expect("lease invariant broken: releasing a path that is not leased");
        let (state, was) = match node.lease {
            Some(LeaseState::Read { holders }) if holders > 1 => (
                Some(LeaseState::Read {
                    holders: holders - 1,
                }),
                None,
            ),
            Some(held) => (None, Some(held.mode())),
            None => panic!("lease invariant broken: releasing a path that is not leased"),
        };
        node.lease = state;
        if let Some(mode) = was {
            self.adjust_below(path, |below| {
                below.leases -= 1;
                if mode == Mode::Write {
                    below.writes -= 1;
                }
            });
            self.prune(path);
        }
    }

    /// Turns the write lease at the path into a read lease held once.
    fn downgrade_lease(&mut self, path: &ProjectRelativePath) {
        let node = self
            .root
            .get_mut(path.iter())
            .expect("lease invariant broken: downgrading a path that is not leased");
        match node.lease {
            Some(LeaseState::Write) => {}
            held => panic!("lease invariant broken: downgrading {held:?}"),
        }
        node.lease = Some(LeaseState::Read { holders: 1 });
        self.adjust_below(path, |below| below.writes -= 1);
    }
}

/// Whether two roots may not be held at the same time.
fn conflicts(a: &Root, b: &Root) -> bool {
    if a.mode == Mode::Read && b.mode == Mode::Read {
        return false;
    }
    if a.path == b.path {
        return true;
    }
    if b.path.starts_with(&a.path) {
        return !(a.mode == Mode::Write && b.mode == Mode::Read && a.excludes(&b.path));
    }
    if a.path.starts_with(&b.path) {
        return !(b.mode == Mode::Write && a.mode == Mode::Read && b.excludes(&a.path));
    }
    false
}

/// What `dir` holds at `path`, as an owned entry.
fn find_in(dir: &ActionSharedDirectory, path: &ForwardRelativePath) -> Option<ArtifactMetadata> {
    let mut components = path.iter().peekable();
    let mut current = dir;
    while let Some(name) = components.next() {
        match current.get(name)? {
            DirectoryEntry::Dir(next) => current = next,
            DirectoryEntry::Leaf(leaf) => {
                return if components.peek().is_none() {
                    Some(DirectoryEntry::Leaf(leaf.dupe()))
                } else {
                    None
                };
            }
        }
    }
    Some(DirectoryEntry::Dir(current.dupe()))
}

#[cfg(test)]
impl<R> DiskTree<R> {
    /// Recomputes every counter from scratch and checks that the trie has no empty nodes, no
    /// tracked root nested in another, and no parked task where a writer would have taken it: at
    /// a leased node, or with a write held above or below it.
    fn check_invariants(&self) {
        fn walk<R>(node: &Node<R>, at_root: bool, under_tracked: bool, under_write: bool) -> Below {
            assert!(at_root || !node.is_empty(), "empty node left in the trie");
            assert!(
                !(under_tracked && node.tracked.is_some()),
                "a tracked root inside another"
            );
            let write_here = matches!(node.lease, Some(LeaseState::Write));
            let mut below = Below::default();
            for child in node.children.values() {
                let under = walk(
                    child,
                    false,
                    under_tracked || node.tracked.is_some(),
                    under_write || write_here,
                );
                below.leases += under.leases + child.lease.is_some() as u32;
                below.writes +=
                    under.writes + matches!(child.lease, Some(LeaseState::Write)) as u32;
                below.waiting += under.waiting + child.waiting.len() as u32;
                below.tasks += under.tasks + child.task.is_some() as u32;
            }
            assert_eq!(node.below, below, "counters drifted");
            if node.task.is_some() {
                assert!(node.lease.is_none(), "a task parked at a leased node");
                assert!(
                    !under_write && below.writes == 0,
                    "a task parked around a held write"
                );
            }
            below
        }
        walk(&self.root, true, false, false);
    }
}

#[cfg(test)]
#[allow(unused_must_use)] // setups load roots that cannot nest
mod tests {
    use buck2_common::file_ops::metadata::FileMetadata;
    use buck2_common::file_ops::metadata::TrackedFileDigest;
    use buck2_execute::digest_config::DigestConfig;
    use buck2_execute::directory::ActionDirectoryBuilder;
    use buck2_execute::directory::ActionDirectoryMember;
    use buck2_execute::directory::INTERNER;
    use futures::FutureExt;

    use super::*;
    use crate::materializers::deferred::artifact_tree::artifact_metadata_matches_entry;

    /// The reply handle in these tests is a label the test picks for the request.
    type Tree = DiskTree<u32>;

    fn path(s: &str) -> ProjectRelativePathBuf {
        ProjectRelativePathBuf::unchecked_new(s.to_owned())
    }

    fn file(content: &str) -> ArtifactMetadata {
        DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
            digest: TrackedFileDigest::from_content(
                content.as_bytes(),
                DigestConfig::testing_default().cas_digest_config(),
            ),
            is_executable: false,
        }))
    }

    /// An exhaustive directory of `(relative path, content)` files.
    fn dir(files: &[(&str, &str)]) -> ArtifactMetadata {
        let mut builder = ActionDirectoryBuilder::empty_exhaustive();
        for (p, content) in files {
            builder
                .insert(
                    ForwardRelativePath::unchecked_new(p),
                    file(content).map_dir(|d| d.into_builder()),
                )
                .unwrap();
        }
        builder.mark_uniformly_exhaustive();
        DirectoryEntry::Dir(
            builder
                .fingerprint(DigestConfig::testing_default().as_directory_serializer())
                .shared(&*INTERNER),
        )
    }

    fn tracked(entry: ArtifactMetadata) -> Tracked {
        Tracked {
            entry: DiskEntry::from_action(&entry),
            last_access_time: Timestamp::UNIX_EPOCH,
            classification: ArtifactClassification::IntermediateOnly,
        }
    }

    fn roots<R>(tree: &DiskTree<R>) -> Vec<String> {
        let mut roots: Vec<String> = tree
            .iter()
            .into_iter()
            .map(|(p, _)| p.to_string())
            .collect();
        roots.sort();
        roots
    }

    fn paths(paths: &[ProjectRelativePathBuf]) -> Vec<String> {
        let mut out: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    fn added(change: &TreeChange) -> Vec<String> {
        paths(
            &change
                .added
                .iter()
                .map(|(p, _)| p.clone())
                .collect::<Vec<_>>(),
        )
    }

    fn removed(change: &TreeChange) -> Vec<String> {
        paths(
            &change
                .removed
                .iter()
                .map(|(p, _)| p.clone())
                .collect::<Vec<_>>(),
        )
    }

    /// A content want for the file that `present` puts at `p`: its own path.
    fn read(p: &str) -> Want {
        Want::Content(file(p))
    }

    /// The paths `present` and `read` agree on: each holds a file whose content is its path.
    const PRESENT: &[&str] = &[
        "a", "a/b", "a/b/c", "a/d", "a/x", "a/z", "b", "b/d", "c", "d", "e", "e/f", "g", "ab",
        "a-b", "x/y/z",
    ];

    /// A grant as the tests look at it; its hold stays with the harness.
    struct Granted {
        mode: Mode,
        exclusions: Vec<ProjectRelativePathBuf>,
        tasks: Vec<Task>,
        displaced: Option<DiskEntry>,
    }

    /// A tree with the holds of its requests, by label, so that a test releases and reports by
    /// label the way a holder would through its guard.
    struct Harness {
        tree: Tree,
        holds: BuckMutMap<u32, Hold>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                tree: Tree::new(),
                holds: BuckMutMap::default(),
            }
        }

        /// A tree holding `file(content)` at each of `present`.
        fn with(present: &[(&str, &str)]) -> Self {
            let mut h = Self::new();
            for (p, content) in present {
                h.tree.load(path(p), tracked(file(content)));
            }
            h
        }

        /// A tree holding, at each leaf of `PRESENT`, a file whose content is its path.
        fn present() -> Self {
            let mut h = Self::new();
            // Nested paths in `PRESENT` would nest roots; keep the leaves only.
            for p in PRESENT {
                if !PRESENT
                    .iter()
                    .any(|other| *other != *p && path(other).starts_with(path(p)))
                {
                    h.tree.load(path(p), tracked(file(p)));
                }
            }
            h
        }

        /// Keeps the holds of `effects` and returns their labels, in order.
        fn woken(&mut self, effects: Effects<u32>) -> Vec<u32> {
            let mut labels = Vec::new();
            for (label, grant) in effects.grants {
                assert!(
                    self.holds.insert(label, grant.hold).is_none(),
                    "request {label} granted twice"
                );
                labels.push(label);
            }
            labels
        }

        fn acquire(&mut self, label: u32, p: &str, want: Want) -> Vec<u32> {
            let effects = self
                .tree
                .acquire(path(p), want, RequestId(label as u64), label);
            self.woken(effects)
        }

        /// Asks for `want` at `p` as request `label`, expecting it granted at once, and nothing
        /// else.
        fn granted(&mut self, label: u32, p: &str, want: Want) -> Granted {
            let mut effects = self
                .tree
                .acquire(path(p), want, RequestId(label as u64), label);
            assert_eq!(
                effects.grants.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
                vec![label],
                "expected `{p}` granted at once"
            );
            let (_, grant) = effects.grants.pop().unwrap();
            let mode = match &grant.hold {
                Hold::Read(_) => Mode::Read,
                Hold::Write(_) => Mode::Write,
            };
            self.holds.insert(label, grant.hold);
            Granted {
                mode,
                exclusions: grant.exclusions,
                tasks: grant.tasks,
                displaced: grant.displaced,
            }
        }

        /// Asks for `want` at `p` as request `label`, expecting it to wait.
        fn queued(&mut self, label: u32, p: &str, want: Want) {
            let granted = self.acquire(label, p, want);
            assert_eq!(granted, Vec::<u32>::new(), "expected `{p}` to wait");
        }

        fn release(&mut self, label: u32) -> Vec<u32> {
            self.release_with(label, Vec::new())
        }

        fn release_with(&mut self, label: u32, tasks: Vec<Task>) -> Vec<u32> {
            let effects = match self.holds.remove(&label).expect("held") {
                Hold::Read(hold) => {
                    assert!(tasks.is_empty(), "a reader has no tasks");
                    self.tree.release_read(hold)
                }
                Hold::Write(hold) => self.tree.release_write(hold, tasks),
            };
            self.woken(effects)
        }

        fn report(&mut self, label: u32, entry: ArtifactMetadata) -> Vec<u32> {
            let Some(Hold::Write(hold)) = self.holds.remove(&label) else {
                panic!("request {label} does not hold a write")
            };
            match self.tree.report(hold, tracked(entry)) {
                Ok((hold, effects)) => {
                    self.holds.insert(label, Hold::Read(hold));
                    self.woken(effects)
                }
                Err((hold, disagreement)) => {
                    self.holds.insert(label, Hold::Write(hold));
                    panic!("report of {label} refused: {disagreement:?}")
                }
            }
        }

        /// Reports and expects the refusal, keeping the write hold.
        fn report_refused(&mut self, label: u32, entry: ArtifactMetadata) -> Disagreement {
            let Some(Hold::Write(hold)) = self.holds.remove(&label) else {
                panic!("request {label} does not hold a write")
            };
            match self.tree.report(hold, tracked(entry)) {
                Ok(_) => panic!("report of {label} was accepted"),
                Err((hold, disagreement)) => {
                    self.holds.insert(label, Hold::Write(hold));
                    disagreement
                }
            }
        }

        fn cancel(&mut self, label: u32, p: &str) -> Vec<u32> {
            let effects = self.tree.cancel(&path(p), RequestId(label as u64));
            self.woken(effects)
        }

        fn mode(&self, label: u32) -> Mode {
            match &self.holds[&label] {
                Hold::Read(_) => Mode::Read,
                Hold::Write(_) => Mode::Write,
            }
        }

        /// Releases everything, including what the releases grant, and checks that nothing is
        /// left.
        fn finish(mut self) {
            loop {
                let mut labels: Vec<u32> = self.holds.keys().copied().collect();
                if labels.is_empty() {
                    break;
                }
                labels.sort();
                for label in labels {
                    self.release(label);
                }
            }
            assert!(self.tree.is_idle(), "holds or waiters left");
            self.tree.check_invariants();
        }
    }

    // ---- tracked roots ----

    #[test]
    fn test_lookup_reads_inside_a_tracked_directory() {
        let mut tree = Tree::new();
        tree.load(path("d"), tracked(dir(&[("a", "A"), ("sub/b", "B")])));
        tree.load(path("f"), tracked(file("F")));
        assert!(matches!(tree.lookup(&path("d")), Lookup::Exact(_)));
        assert!(tree.matches(&path("d/a"), &file("A")));
        assert!(!tree.matches(&path("d/a"), &file("B")));
        assert!(tree.matches(&path("d/sub/b"), &file("B")));
        assert!(tree.matches(&path("d/sub"), &dir(&[("b", "B")])));
        assert!(matches!(
            tree.lookup(&path("d/missing")),
            Lookup::Inside { entry: None, .. }
        ));
        // Under a tracked file nothing can be, and the tree says so rather than looking inside.
        assert!(matches!(
            tree.lookup(&path("f/x")),
            Lookup::UnderLeaf { .. }
        ));
        assert!(!tree.matches(&path("f/x"), &file("X")));
        assert!(tree.roots_at_or_below(&path("f/x")).is_empty());
        assert!(matches!(tree.lookup(&path("e")), Lookup::Untracked));
        tree.check_invariants();
    }

    /// A root loaded from sqlite is a root like any other: granted for reading as the rows say,
    /// and left alone by a write over it that would match it.
    #[test]
    fn test_loaded_roots_are_trusted() {
        let mut h = Harness::new();
        h.tree.load(path("x"), tracked(file("x")));
        h.tree.load(path("d/a"), tracked(file("a")));

        let granted = h.granted(1, "x", Want::Content(file("x")));
        assert_eq!(granted.mode, Mode::Read);
        h.release(1);

        let granted = h.granted(2, "d", Want::Content(dir(&[("a", "a"), ("b", "b")])));
        assert_eq!(granted.mode, Mode::Write);
        assert_eq!(granted.exclusions, vec![path("d/a")]);
        assert_eq!(roots(&h.tree), vec!["d/a", "x"]);
        h.tree.check_invariants();
    }

    #[test]
    fn test_loading_nested_roots_is_refused() {
        let mut tree = Tree::new();
        assert!(tree.load(path("d"), tracked(dir(&[("a", "A")]))));
        assert!(!tree.load(path("d/a"), tracked(file("A"))));
        assert!(!tree.load(path("d"), tracked(file("A"))));
        assert!(tree.load(path("e"), tracked(file("E"))));
        assert!(!tree.load(path("e/f"), tracked(file("F"))));
        assert_eq!(roots(&tree), vec!["d", "e"]);
    }

    #[test]
    fn test_a_write_inside_a_directory_expands_it() {
        let mut h = Harness::new();
        h.tree
            .load(path("d"), tracked(dir(&[("a", "A"), ("b", "B")])));
        let effects = h.tree.acquire(path("d/a"), Want::Write, RequestId(0), 0);
        // `d/a` is added by the expansion and untracked in the same change.
        assert_eq!(removed(&effects.change), vec!["d", "d/a"]);
        assert_eq!(added(&effects.change), vec!["d/b"]);
        assert_eq!(roots(&h.tree), vec!["d/b"]);
        assert!(h.tree.matches(&path("d/b"), &file("B")));
        h.woken(effects);
        h.report(0, file("A2"));
        assert_eq!(roots(&h.tree), vec!["d/a", "d/b"]);
        assert!(h.tree.matches(&path("d/a"), &file("A2")));
        h.finish();
    }

    #[test]
    fn test_a_write_deep_inside_a_directory_expands_every_level() {
        let mut h = Harness::new();
        h.tree.load(
            path("d"),
            tracked(dir(&[
                ("a", "A"),
                ("s/t/x", "X"),
                ("s/t/y", "Y"),
                ("s/z", "Z"),
            ])),
        );
        h.granted(0, "d/s/t/x", Want::Write);
        assert_eq!(roots(&h.tree), vec!["d/a", "d/s/t/y", "d/s/z"]);
        h.report(0, file("X2"));
        assert_eq!(roots(&h.tree), vec!["d/a", "d/s/t/x", "d/s/t/y", "d/s/z"]);
        assert!(h.tree.matches(&path("d/s/t/x"), &file("X2")));
        assert!(h.tree.matches(&path("d/s/t/y"), &file("Y")));
        h.finish();
    }

    #[test]
    fn test_a_write_under_a_tracked_file_forgets_the_file() {
        let mut h = Harness::with(&[("a", "old"), ("b", "B")]);
        // A file where a directory has to go: whoever writes below it clears it from disk, so
        // the grant forgets it, and the report puts the new root in place.
        let effects = h.tree.acquire(path("a/b"), Want::Write, RequestId(0), 0);
        assert_eq!(removed(&effects.change), vec!["a"]);
        assert_eq!(roots(&h.tree), vec!["b"]);
        h.woken(effects);
        h.report(0, file("AB"));
        assert_eq!(roots(&h.tree), vec!["a/b", "b"]);
        h.finish();
        // A reader of the file blocks the write below it until it is done.
        let mut h = Harness::with(&[("a", "old")]);
        h.granted(1, "a", Want::Content(file("old")));
        h.queued(2, "a/b", Want::Write);
        assert_eq!(h.release(1), vec![2]);
        h.finish();
    }

    #[test]
    fn test_a_report_replaces_everything_below_it() {
        let mut h = Harness::with(&[("d/a", "A"), ("d/b/c", "C"), ("e", "E")]);
        h.granted(0, "d", Want::Write);
        assert_eq!(roots(&h.tree), vec!["e"]);
        h.report(0, dir(&[("x", "X")]));
        assert_eq!(roots(&h.tree), vec!["d", "e"]);
        h.finish();
    }

    #[test]
    fn test_a_write_grant_carries_what_it_displaced() {
        let mut h = Harness::with(&[("a", "a")]);
        h.tree
            .load(path("d"), tracked(dir(&[("x", "x"), ("y", "y")])));
        // A tracked root: its entry.
        let displaced = h.granted(0, "a", Want::Write).displaced;
        assert!(displaced.is_some_and(|entry| entry.matches(&file("a"))));
        // Nothing tracked: nothing.
        assert!(h.granted(1, "b", Want::Write).displaced.is_none());
        // Inside a tracked directory: what the directory held there, the directory expanded.
        let displaced = h.granted(2, "d/x", Want::Write).displaced;
        assert!(displaced.is_some_and(|entry| entry.matches(&file("x"))));
        h.release(2);
        // The directory, expanded into the roots below it: nothing at exactly the root.
        assert!(h.granted(3, "d", Want::Write).displaced.is_none());
    }

    #[test]
    fn test_touch_finds_the_containing_root() {
        let mut tree = Tree::new();
        tree.load(path("d"), tracked(dir(&[("a", "A")])));
        let now = Timestamp::from_second(1000).unwrap();
        assert_eq!(tree.touch(&path("d/a"), now), Some(path("d")));
        assert_eq!(tree.touch(&path("e"), now), None);
        let Lookup::Exact(root) = tree.lookup(&path("d")) else {
            panic!()
        };
        assert_eq!(root.last_access_time, now);
    }

    #[test]
    fn test_a_lease_inside_a_tracked_directory_survives_its_expansion() {
        let mut h = Harness::new();
        h.tree
            .load(path("d"), tracked(dir(&[("a", "A"), ("b", "B")])));
        h.granted(1, "d/a", Want::Content(file("A")));
        h.granted(2, "d/b", Want::Write);
        assert_eq!(roots(&h.tree), vec!["d/a"]);
        assert!(matches!(
            h.tree.root.get(path("d/a").iter()).unwrap().lease,
            Some(LeaseState::Read { holders: 1 })
        ));
        h.finish();
    }

    // ---- leases ----

    #[test]
    fn test_reads_never_conflict() {
        let mut h = Harness::present();
        for (label, p) in ["a/b/c", "a/b/c", "a/d", "a/x", "b/d", "c", "ab", "a-b"]
            .into_iter()
            .enumerate()
        {
            let grant = h.granted(label as u32, p, read(p));
            assert_eq!(grant.mode, Mode::Read);
            assert!(grant.exclusions.is_empty());
            assert!(grant.tasks.is_empty());
        }
        h.finish();
    }

    #[test]
    fn test_write_conflicts_with_reads_and_writes_in_both_directions() {
        // Against a held read at `a/b/c`: a sibling, and paths that merely share a prefix as
        // strings, are free; writes at, above and below it wait.
        let mut h = Harness::present();
        h.granted(0, "a/b/c", read("a/b/c"));
        h.granted(1, "a/d", Want::Write);
        h.granted(2, "ab", Want::Write);
        h.granted(3, "a-b", Want::Write);
        h.queued(4, "a/b/c", Want::Write);
        h.queued(5, "a/b", Want::Write);
        h.queued(6, "a", Want::Write);
        h.queued(7, "a/b/c/d", Want::Write);
        h.tree.check_invariants();
        for (label, p) in [(4, "a/b/c"), (5, "a/b"), (6, "a"), (7, "a/b/c/d")] {
            h.cancel(label, p);
        }
        h.finish();

        // Against a held write at `a/b/c`: everything at, above and below it waits.
        let mut h = Harness::present();
        h.granted(0, "a/b/c", Want::Write);
        h.granted(1, "a/d", read("a/d"));
        h.granted(2, "ab", read("ab"));
        h.queued(3, "a/b/c", read("a/b/c"));
        h.queued(4, "a/b/c/d", Want::Content(file("D")));
        h.queued(5, "a/b", Want::Content(dir(&[("c", "a/b/c")])));
        h.queued(6, "a/b/c", Want::Write);
        h.queued(7, "a", Want::Write);
        h.queued(8, "a/b/c/d", Want::Write);
        h.tree.check_invariants();
    }

    #[test]
    fn test_unrelated_subtrees_never_interact() {
        let mut h = Harness::present();
        h.granted(0, "a/b", Want::Write);
        h.granted(1, "x/y/z", Want::Write);
        h.granted(2, "c", Want::Write);
        h.granted(3, "e/f", read("e/f"));
        h.finish();
    }

    #[test]
    fn test_a_content_want_the_tree_lacks_is_a_write() {
        let mut h = Harness::present();
        // Present with other content, and absent altogether.
        let grant = h.granted(0, "a/b/c", Want::Content(file("other")));
        assert_eq!(grant.mode, Mode::Write);
        assert!(
            !h.tree.matches(&path("a/b/c"), &file("a/b/c")),
            "untracked by the grant"
        );
        let grant = h.granted(1, "new", Want::Content(file("new")));
        assert_eq!(grant.mode, Mode::Write);
        // A write want is a write even over matching content, and untracks it.
        let grant = h.granted(2, "c", Want::Write);
        assert_eq!(grant.mode, Mode::Write);
        assert!(!h.tree.matches(&path("c"), &file("c")));
        h.finish();
    }

    #[test]
    fn test_a_waiting_request_takes_the_mode_the_tree_has_when_it_is_examined() {
        let mut h = Harness::new();
        h.granted(0, "p", Want::Content(file("P")));
        h.queued(1, "p", Want::Content(file("P")));
        // The first reports: the second finds its content there and reads it.
        assert_eq!(h.report(0, file("P")), vec![1]);
        assert_eq!(h.mode(1), Mode::Read);
        h.finish();
        // The first releases without reporting: the second has to write it itself.
        let mut h = Harness::new();
        h.granted(0, "p", Want::Content(file("P")));
        h.queued(1, "p", Want::Content(file("P")));
        assert_eq!(h.release(0), vec![1]);
        assert_eq!(h.mode(1), Mode::Write);
        h.finish();
    }

    #[test]
    fn test_a_write_excludes_and_keeps_the_matching_roots_below_it() {
        let mut h = Harness::with(&[("d/a", "A"), ("d/b", "old"), ("d/sub/c", "C"), ("d/x", "X")]);
        let want = dir(&[("a", "A"), ("b", "B"), ("sub/c", "C")]);
        let grant = h.granted(0, "d", Want::Content(want.dupe()));
        assert_eq!(grant.mode, Mode::Write);
        assert_eq!(paths(&grant.exclusions), vec!["d/a", "d/sub/c"]);
        // What matched stays tracked, what did not is gone, and readers of what stays are fine.
        assert_eq!(roots(&h.tree), vec!["d/a", "d/sub/c"]);
        assert_eq!(
            h.granted(1, "d/a", Want::Content(file("A"))).mode,
            Mode::Read
        );
        h.queued(2, "d/b", Want::Content(file("B")));
        // Reporting the whole directory replaces the excluded roots by the one root, and the
        // reader inside is granted on it.
        assert_eq!(h.report(0, want), vec![2]);
        assert_eq!(roots(&h.tree), vec!["d"]);
        h.finish();
    }

    #[test]
    fn test_a_produce_want_takes_the_whole_root_when_the_tree_lacks_it() {
        let mut h = Harness::with(&[("d/a", "A"), ("d/b", "old")]);
        let want = dir(&[("a", "A"), ("b", "B")]);
        let grant = h.granted(0, "d", Want::Produce(want.dupe()));
        assert_eq!(grant.mode, Mode::Write);
        assert!(grant.exclusions.is_empty(), "the holder writes all of it");
        assert_eq!(roots(&h.tree), Vec::<String>::new());
        h.report(0, want.dupe());
        assert_eq!(roots(&h.tree), vec!["d"]);
        // Found from then on.
        assert_eq!(h.granted(1, "d", Want::Produce(want)).mode, Mode::Read);
        h.finish();
    }

    #[test]
    fn test_a_report_that_disagrees_with_what_it_excluded_is_refused() {
        let mut h = Harness::with(&[("d/a", "A"), ("d/b", "B")]);
        h.granted(0, "d", Want::Content(dir(&[("a", "A"), ("b", "B2")])));
        assert_eq!(roots(&h.tree), vec!["d/a"]);
        // The holder promised to leave `d/a` alone; a report that says something else is there,
        // or that `d` is a file, cannot be right.
        assert_eq!(
            h.report_refused(0, dir(&[("a", "A3"), ("b", "B2")])).root,
            path("d/a")
        );
        assert_eq!(h.report_refused(0, file("D")).root, path("d/a"));
        assert_eq!(roots(&h.tree), vec!["d/a"]);
        assert_eq!(h.mode(0), Mode::Write);
        h.report(0, dir(&[("a", "A"), ("b", "B2")]));
        assert_eq!(roots(&h.tree), vec!["d"]);
        h.finish();
    }

    #[test]
    fn test_a_read_held_root_outside_the_exclusions_blocks_the_write() {
        let mut h = Harness::with(&[("d/a", "A"), ("d/b", "old")]);
        h.granted(0, "d/b", Want::Content(file("old")));
        // `d/b` does not match the want, so the write cannot exclude it, and it is read-held.
        h.queued(1, "d", Want::Content(dir(&[("a", "A"), ("b", "B")])));
        assert_eq!(h.release(0), vec![1]);
        h.finish();
    }

    #[test]
    fn test_a_queued_write_lets_readers_of_what_it_excludes_through() {
        let mut h = Harness::with(&[("d/a", "A"), ("d/b", "old")]);
        h.granted(0, "d/b", Want::Content(file("old")));
        h.queued(1, "d", Want::Content(dir(&[("a", "A"), ("b", "B")])));
        // `d/a` is excluded by the waiting write: reading it does not queue behind the write.
        assert_eq!(
            h.granted(2, "d/a", Want::Content(file("A"))).mode,
            Mode::Read
        );
        // `d/b` is not: a later reader of it queues behind the write, and writes anywhere under
        // `d` do too.
        h.queued(3, "d/b", Want::Content(file("old")));
        h.queued(4, "d/new", Want::Write);
        h.tree.check_invariants();
    }

    #[test]
    fn test_waiting_writer_blocks_later_readers() {
        let mut h = Harness::present();
        h.granted(0, "a/b/c", read("a/b/c"));
        h.queued(1, "a/b", Want::Write);
        // Reads under the waiting write wait behind it; unrelated reads do not.
        h.queued(2, "a/b/c", read("a/b/c"));
        h.granted(3, "a/d", read("a/d"));
        h.granted(4, "ab", read("ab"));
        // Once the reader goes, the writer is served first and the reader stays behind it.
        assert_eq!(h.release(0), vec![1]);
        assert_eq!(h.tree.waiting(), 1);
        h.tree.check_invariants();
    }

    #[test]
    fn test_a_report_wakes_readers_but_not_writers() {
        let mut h = Harness::new();
        h.granted(0, "p", Want::Content(file("P")));
        h.queued(1, "p", Want::Content(file("P")));
        h.queued(2, "p", Want::Write);
        h.queued(3, "p", Want::Content(file("P")));
        // The report turns the holder into a reader. The first waiting reader shares it; the
        // writer behind it still waits, and so does the reader behind the writer.
        assert_eq!(h.report(0, file("P")), vec![1]);
        assert_eq!(h.tree.waiting(), 2);
        // Only once both readers have released is the writer granted, then the last reader
        // after the writer has reported.
        assert_eq!(h.release(0), Vec::<u32>::new());
        assert_eq!(h.release(1), vec![2]);
        assert_eq!(h.report(2, file("P")), vec![3]);
        h.finish();
    }

    #[test]
    fn test_release_wakes_exactly_the_newly_grantable_requests() {
        let mut h = Harness::present();
        h.granted(0, "a/b", Want::Write);
        h.granted(1, "a/d", Want::Write);
        h.queued(2, "a", Want::Write);
        h.queued(3, "a/b/c", read("a/b/c"));
        h.queued(4, "a/x", read("a/x"));
        // `a/b` released: the write on `a` still waits for `a/d`, and blocks the readers behind
        // it, so nothing is granted.
        assert_eq!(h.release(0), Vec::<u32>::new());
        // `a/d` released: the write on `a` goes, and only it.
        assert_eq!(h.release(1), vec![2]);
        assert_eq!(h.tree.waiting(), 2);
        h.tree.check_invariants();
    }

    #[test]
    fn test_cancel_removes_a_waiting_request_and_unblocks_the_ones_behind_it() {
        let mut h = Harness::present();
        h.granted(0, "a/b/c", read("a/b/c"));
        h.queued(1, "a", Want::Write);
        h.queued(2, "a/x", read("a/x"));
        h.queued(3, "a/b/c", read("a/b/c"));
        // The writer withdraws: the readers behind it are free, in order.
        assert_eq!(h.cancel(1, "a"), vec![2, 3]);
        assert_eq!(h.tree.waiting(), 0);
        // Cancelling what is not waiting is harmless.
        assert_eq!(h.cancel(1, "a"), Vec::<u32>::new());
        assert_eq!(h.cancel(9, "nowhere"), Vec::<u32>::new());
        h.finish();
    }

    #[test]
    fn test_everything_released_leaves_nothing_behind() {
        let mut h = Harness::present();
        h.granted(0, "a/b/c", read("a/b/c"));
        h.granted(1, "a/b/c", read("a/b/c"));
        h.granted(2, "x/y/z", Want::Write);
        h.queued(3, "a", Want::Write);
        h.report(2, file("Z"));
        h.release(2);
        h.release(0);
        assert_eq!(h.release(1), vec![3]);
        h.finish();
    }

    #[test]
    fn test_the_cleaner_takes_only_what_it_saw_and_nobody_uses() {
        let t0 = Timestamp::UNIX_EPOCH;
        let mut h = Harness::with(&[("d/a", "A"), ("d/b", "B"), ("e", "E")]);
        // A root as the scan saw it, unheld: taken, and untracked by the take.
        let (grant, change) = h
            .tree
            .take_for_cleaning(
                path("d/a"),
                Expect::Tracked {
                    last_access_time: t0,
                },
            )
            .expect("free and as expected");
        assert!(matches!(grant.hold, Hold::Write(_)));
        assert_eq!(removed(&change), vec!["d/a"]);
        assert_eq!(roots(&h.tree), vec!["d/b", "e"]);
        h.holds.insert(0, grant.hold);
        // A root someone read since the scan has another access time: refused.
        h.granted(1, "d/b", Want::Content(file("B")));
        h.release(1);
        assert!(
            h.tree
                .take_for_cleaning(
                    path("d/b"),
                    Expect::Tracked {
                        last_access_time: t0,
                    },
                )
                .is_none()
        );
        // A root somebody holds is refused; an untracked path with roots below is not
        // untracked; an untracked path with nothing below is taken; a waiting writer above is
        // ahead of the cleaner.
        h.granted(2, "e", Want::Content(file("E")));
        assert!(
            h.tree
                .take_for_cleaning(
                    path("e"),
                    Expect::Tracked {
                        last_access_time: t0,
                    },
                )
                .is_none()
        );
        assert!(
            h.tree
                .take_for_cleaning(path("d"), Expect::Untracked)
                .is_none()
        );
        let (grant, _) = h
            .tree
            .take_for_cleaning(path("x/y"), Expect::Untracked)
            .expect("nothing there");
        h.holds.insert(3, grant.hold);
        h.queued(4, "x", Want::Write);
        assert!(
            h.tree
                .take_for_cleaning(path("x/z"), Expect::Untracked)
                .is_none()
        );
        h.release(3);
        h.finish();
    }

    #[test]
    fn test_the_cleaner_reads_without_touching() {
        let t0 = Timestamp::UNIX_EPOCH;
        let mut h = Harness::with(&[("p", "P")]);
        let hold = h
            .tree
            .hold_for_reading(path("p"), Some(t0))
            .expect("free and as expected");
        // Readers share it and touch it; a writer waits for it.
        assert_eq!(h.granted(0, "p", Want::Content(file("P"))).mode, Mode::Read);
        h.queued(1, "p", Want::Write);
        let Lookup::Exact(root) = h.tree.lookup(&path("p")) else {
            panic!()
        };
        assert_ne!(root.last_access_time, t0, "the reader touched it");
        h.release(0);
        let effects = h.tree.release_read(hold);
        assert_eq!(h.woken(effects), vec![1]);
        assert!(
            h.tree
                .hold_for_reading(path("p"), Some(Timestamp::UNIX_EPOCH))
                .is_none(),
            "held for writing"
        );
        h.finish();
        // Without an expectation, any tracked content will do, inside a directory included, and
        // the hold is still no use of it.
        let mut h = Harness::new();
        h.tree.load(path("d"), tracked(dir(&[("a", "A")])));
        let hold = h
            .tree
            .hold_for_reading(path("d/a"), None)
            .expect("tracked inside `d`");
        let Lookup::Exact(root) = h.tree.lookup(&path("d")) else {
            panic!()
        };
        assert_eq!(root.last_access_time, Timestamp::UNIX_EPOCH);
        assert!(h.tree.hold_for_reading(path("d/x"), None).is_none());
        assert!(h.tree.hold_for_reading(path("e"), None).is_none());
        h.tree.release_read(hold);
        h.finish();
    }

    fn pending_task(p: &str) -> Task {
        Task::spawn(path(p), file("T"), |_| futures::future::pending().boxed())
    }

    fn task_roots(tasks: &[Task]) -> Vec<String> {
        let mut out: Vec<String> = tasks.iter().map(|t| t.root().to_string()).collect();
        out.sort();
        out
    }

    #[tokio::test]
    async fn test_tasks_handed_back_go_to_the_next_writer_over_their_root() {
        // A writer granted in the same step as the release gets the task uncancelled.
        let mut h = Harness::new();
        h.granted(0, "p", Want::Write);
        h.queued(1, "p", Want::Write);
        let Hold::Write(hold) = h.holds.remove(&0).unwrap() else {
            panic!()
        };
        let mut effects = h.tree.release_write(hold, vec![pending_task("p")]);
        let (label, grant) = effects.grants.pop().unwrap();
        assert!(effects.grants.is_empty());
        assert_eq!(label, 1);
        assert_eq!(task_roots(&grant.tasks), vec!["p"]);
        assert!(!grant.tasks[0].is_cancelled());
        h.holds.insert(1, grant.hold);
        h.finish();

        // Nobody waiting: the task is cancelled and parked at its root, and the next write at,
        // above or below the root takes it along.
        let mut h = Harness::new();
        h.granted(0, "d/p", Want::Write);
        assert_eq!(
            h.release_with(0, vec![pending_task("d/p")]),
            Vec::<u32>::new()
        );
        h.tree.check_invariants();
        assert!(!h.tree.is_idle(), "the task is parked");
        assert!(h.granted(1, "d/q", Want::Write).tasks.is_empty());
        h.release(1);
        let grant = h.granted(2, "d", Want::Write);
        assert_eq!(task_roots(&grant.tasks), vec!["d/p"]);
        assert!(grant.tasks[0].is_cancelled());
        h.finish();

        // A reader does not take tasks; a writer below a parked task's root does.
        let mut h = Harness::with(&[("d/x", "X")]);
        h.granted(0, "d/p", Want::Write);
        h.release_with(0, vec![pending_task("d/p")]);
        assert!(
            h.granted(1, "d/x", Want::Content(file("X")))
                .tasks
                .is_empty()
        );
        assert_eq!(
            task_roots(&h.granted(2, "d/p/inner", Want::Write).tasks),
            vec!["d/p"]
        );
        h.finish();
    }

    /// A brute-force model of the tree's contract on leases: a request is granted when it
    /// conflicts with nothing granted and with no request that arrived before it and is still
    /// waiting, every request resolved afresh against the tracked roots each time it is examined,
    /// and a write grant untracking what it does not exclude.
    ///
    /// The model shares `conflicts` and `find_in` with the tree and restates the resolution and
    /// arrival-order rules over flat lists, so the comparison checks the trie, its counters, the
    /// exclusion walk and the per-node queues. It is not an independent oracle for the rules
    /// themselves; those are pinned by the tests above. Tracked roots stay at leaf paths here, so
    /// expansion never enters; it has its own tests.
    #[derive(Default)]
    struct Model {
        tracked: Vec<(ProjectRelativePathBuf, ArtifactMetadata)>,
        granted: Vec<(u32, Root)>,
        waiting: Vec<(u32, ProjectRelativePathBuf, Want)>,
    }

    impl Model {
        fn entry_at(&self, path: &ProjectRelativePath) -> Option<ArtifactMetadata> {
            self.tracked
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, e)| e.dupe())
        }

        fn resolve(&self, path: &ProjectRelativePath, want: &Want) -> Root {
            match want {
                Want::Write => Root {
                    path: path.to_buf(),
                    mode: Mode::Write,
                    exclusions: Vec::new(),
                },
                Want::Produce(entry) => {
                    let matches = self
                        .entry_at(path)
                        .is_some_and(|e| artifact_metadata_matches_entry(&e, entry));
                    Root {
                        path: path.to_buf(),
                        mode: if matches { Mode::Read } else { Mode::Write },
                        exclusions: Vec::new(),
                    }
                }
                Want::Content(entry) => {
                    let matches = self
                        .entry_at(path)
                        .is_some_and(|e| artifact_metadata_matches_entry(&e, entry));
                    if matches {
                        Root {
                            path: path.to_buf(),
                            mode: Mode::Read,
                            exclusions: Vec::new(),
                        }
                    } else {
                        let exclusions = match entry {
                            DirectoryEntry::Dir(dir) => self
                                .tracked
                                .iter()
                                .filter(|(p, e)| {
                                    p != path
                                        && p.starts_with(path)
                                        && find_in(dir, p.strip_prefix(path).unwrap())
                                            .is_some_and(|w| artifact_metadata_matches_entry(e, &w))
                                })
                                .map(|(p, _)| p.clone())
                                .collect(),
                            DirectoryEntry::Leaf(_) => Vec::new(),
                        };
                        Root {
                            path: path.to_buf(),
                            mode: Mode::Write,
                            exclusions,
                        }
                    }
                }
            }
        }

        fn grantable(&self, root: &Root, before: usize) -> bool {
            !self.granted.iter().any(|(_, held)| conflicts(held, root))
                && !self.waiting[..before]
                    .iter()
                    .any(|(_, path, want)| conflicts(&self.resolve(path, want), root))
        }

        fn grant(&mut self, label: u32, root: Root) {
            if root.mode == Mode::Write {
                self.tracked.retain(|(p, _)| {
                    !p.starts_with(&root.path) || root.excludes(p) && p != &root.path
                });
            }
            self.granted.push((label, root));
        }

        fn request(&mut self, label: u32, path: ProjectRelativePathBuf, want: Want) -> bool {
            let root = self.resolve(&path, &want);
            if self.grantable(&root, self.waiting.len()) {
                self.grant(label, root);
                true
            } else {
                self.waiting.push((label, path, want));
                false
            }
        }

        fn pump(&mut self) -> Vec<u32> {
            let mut woken = Vec::new();
            let mut i = 0;
            while i < self.waiting.len() {
                let (_, path, want) = &self.waiting[i];
                let root = self.resolve(path, want);
                if self.grantable(&root, i) {
                    let (label, _, _) = self.waiting.remove(i);
                    self.grant(label, root);
                    woken.push(label);
                } else {
                    i += 1;
                }
            }
            woken
        }

        fn release(&mut self, label: u32) -> Vec<u32> {
            self.granted.retain(|(granted, _)| *granted != label);
            self.pump()
        }

        /// Reports `entry` at a write root and downgrades it.
        fn report(&mut self, label: u32, entry: ArtifactMetadata) -> Vec<u32> {
            let (_, root) = self.granted.iter_mut().find(|(g, _)| *g == label).unwrap();
            let path = root.path.clone();
            root.mode = Mode::Read;
            root.exclusions.clear();
            self.tracked.retain(|(p, _)| *p != path);
            self.tracked.push((path, entry));
            self.pump()
        }

        fn cancel(&mut self, label: u32) -> Vec<u32> {
            self.waiting.retain(|(waiting, _, _)| *waiting != label);
            self.pump()
        }

        fn tracked_paths(&self) -> Vec<String> {
            let mut v: Vec<String> = self.tracked.iter().map(|(p, _)| p.to_string()).collect();
            v.sort();
            v
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }

        fn below(&mut self, n: usize) -> usize {
            self.next() as usize % n
        }
    }

    #[test]
    fn test_matches_the_brute_force_model() {
        const LEAVES: &[&str] = &["a/b", "a/c", "a/d/e", "e/f", "g"];
        const DIRS: &[&str] = &["a", "a/d", "e"];
        const LABELS: &[&str] = &["x", "y"];
        let mut rng = Lcg(0x5eed);
        let mut h = Harness::new();
        let mut model = Model::default();
        let mut next_label = 0u32;
        // The labels of the requests that hold something, and of those that wait, with their
        // paths.
        let mut held: Vec<u32> = Vec::new();
        let mut waiting: Vec<(u32, ProjectRelativePathBuf)> = Vec::new();

        // A directory want: the leaves under `dir`, each with a random label.
        let dir_want = |rng: &mut Lcg, d: &str| -> ArtifactMetadata {
            let files: Vec<(String, &str)> = LEAVES
                .iter()
                .filter(|leaf| path(leaf).starts_with(path(d)))
                .map(|leaf| {
                    (
                        path(leaf).strip_prefix(path(d)).unwrap().to_string(),
                        LABELS[rng.below(LABELS.len())],
                    )
                })
                .collect();
            let files: Vec<(&str, &str)> = files.iter().map(|(p, l)| (p.as_str(), *l)).collect();
            dir(&files)
        };

        fn check_grant(grant: &Grant, path: &ProjectRelativePath, expected: &Root) {
            assert_eq!(grant.hold.path(), path);
            let mode = match grant.hold {
                Hold::Read(_) => Mode::Read,
                Hold::Write(_) => Mode::Write,
            };
            assert_eq!(mode, expected.mode, "mode at {path}");
            assert_eq!(
                paths(&grant.exclusions),
                paths(&expected.exclusions),
                "exclusions at {path}"
            );
        }

        // Checks the grants in `effects` against the model and moves their requests from
        // `waiting` to `held`.
        fn woken(
            h: &mut Harness,
            effects: Effects<u32>,
            model: &Model,
            waiting: &mut Vec<(u32, ProjectRelativePathBuf)>,
            held: &mut Vec<u32>,
        ) -> Vec<u32> {
            let mut labels = Vec::new();
            for (label, grant) in &effects.grants {
                let position = waiting
                    .iter()
                    .position(|(l, _)| l == label)
                    .expect("woken request was waiting");
                let (_, path) = waiting.remove(position);
                let (_, expected) = model
                    .granted
                    .iter()
                    .find(|(l, _)| l == label)
                    .expect("the model granted it too");
                check_grant(grant, &path, expected);
                held.push(*label);
                labels.push(*label);
            }
            h.woken(effects);
            labels
        }

        for _ in 0..4000 {
            match rng.below(100) {
                0..=44 => {
                    let (p, want) = match rng.below(4) {
                        0 => (path(LEAVES[rng.below(LEAVES.len())]), Want::Write),
                        1 => {
                            let d = DIRS[rng.below(DIRS.len())];
                            (path(d), Want::Content(dir_want(&mut rng, d)))
                        }
                        _ => (
                            path(LEAVES[rng.below(LEAVES.len())]),
                            Want::Content(file(LABELS[rng.below(LABELS.len())])),
                        ),
                    };
                    let label = next_label;
                    next_label += 1;
                    let model_granted = model.request(label, p.clone(), want.clone());
                    let effects = h
                        .tree
                        .acquire(p.clone(), want, RequestId(label as u64), label);
                    if model_granted {
                        assert_eq!(
                            effects.grants.len(),
                            1,
                            "tree queued what the model granted"
                        );
                        let (_, expected) = model.granted.last().unwrap();
                        check_grant(&effects.grants[0].1, &p, expected);
                        held.push(label);
                    } else {
                        assert!(
                            effects.grants.is_empty(),
                            "tree granted what the model queued"
                        );
                        waiting.push((label, p));
                    }
                    h.woken(effects);
                }
                45..=64 if !held.is_empty() => {
                    let label = held.remove(rng.below(held.len()));
                    let expected = model.release(label);
                    let effects = match h.holds.remove(&label).unwrap() {
                        Hold::Read(hold) => h.tree.release_read(hold),
                        Hold::Write(hold) => h.tree.release_write(hold, Vec::new()),
                    };
                    assert_eq!(
                        woken(&mut h, effects, &model, &mut waiting, &mut held),
                        expected
                    );
                }
                65..=89 if !held.is_empty() => {
                    let index = rng.below(held.len());
                    let label = held[index];
                    if h.mode(label) != Mode::Write {
                        continue;
                    }
                    let Hold::Write(hold) = h.holds.remove(&label).unwrap() else {
                        unreachable!()
                    };
                    let p = hold.path().to_buf();
                    if LEAVES.contains(&p.as_str()) {
                        // Leaves get reported with some content and held on in read mode.
                        let entry = file(LABELS[rng.below(LABELS.len())]);
                        let expected = model.report(label, entry.dupe());
                        let (read, effects) = h
                            .tree
                            .report(hold, tracked(entry))
                            .unwrap_or_else(|(_, d)| panic!("refused: {d:?}"));
                        h.holds.insert(label, Hold::Read(read));
                        assert_eq!(
                            woken(&mut h, effects, &model, &mut waiting, &mut held),
                            expected
                        );
                    } else {
                        // A directory write gives up without a report, so that tracked roots
                        // stay at leaf paths.
                        held.remove(index);
                        let expected = model.release(label);
                        let effects = h.tree.release_write(hold, Vec::new());
                        assert_eq!(
                            woken(&mut h, effects, &model, &mut waiting, &mut held),
                            expected
                        );
                    }
                }
                90..=99 if !waiting.is_empty() => {
                    let (label, p) = waiting.remove(rng.below(waiting.len()));
                    let expected = model.cancel(label);
                    let effects = h.tree.cancel(&p, RequestId(label as u64));
                    assert_eq!(
                        woken(&mut h, effects, &model, &mut waiting, &mut held),
                        expected
                    );
                }
                _ => {}
            }
            h.tree.check_invariants();
            assert_eq!(h.tree.waiting(), model.waiting.len());
            assert_eq!(roots(&h.tree), model.tracked_paths());
        }
        h.finish();
    }
}
