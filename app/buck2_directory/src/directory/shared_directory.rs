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
use std::sync::Weak;

use allocative::Allocative;
use buck2_core::directory_digest::DirectoryDigest;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use derivative::Derivative;
use derive_more::Display;
use dupe::Clone_;
use dupe::Dupe;
use dupe::Dupe_;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use pagable::arc_erase::ArcErase;
use pagable::arc_erase::ArcEraseDyn;
use pagable::arc_erase::ArcEraseType;
use pagable::arc_erase::ArcSerializeOutcome;
use pagable::arc_erase::StdArcEraseType;
use pagable::arc_erase::WeakErase;
use pagable::arc_erase::deserialize_arc;

use crate::directory::builder::DirectoryBuilder;
use crate::directory::dashmap_directory_interner::DashMapDirectoryInterner;
use crate::directory::directory::Directory;
use crate::directory::directory_data::DirectoryData;
use crate::directory::directory_ref::DirectoryRef;
use crate::directory::directory_ref::FingerprintedDirectoryRef;
use crate::directory::entry::DirectoryEntry;
use crate::directory::exhaustiveness::ExhaustivenessHash;
use crate::directory::fingerprinted_directory::FingerprintedDirectory;
use crate::directory::immutable_directory::ImmutableDirectory;
use crate::directory::macros::impl_fingerprinted_directory;

pub type SharedDirectoryData<L, H> = DirectoryData<SharedDirectory<L, H>, L, H>;

#[derive(Derivative, Display, Allocative)]
#[derivative(Debug(bound = "L: ::std::fmt::Debug"))]
#[display("{}", self.data)]
pub struct SharedDirectoryInner<L, H>
where
    H: DirectoryDigest,
{
    pub(super) data: SharedDirectoryData<L, H>,

    #[derivative(Debug = "ignore")]
    pub(super) interner: DashMapDirectoryInterner<L, H>,
}

pub trait SharedDirectoryInternable<H: DirectoryDigest>: Sized + 'static {
    fn interner() -> DashMapDirectoryInterner<Self, H>;
}

impl<L, H> Drop for SharedDirectoryInner<L, H>
where
    H: DirectoryDigest,
{
    fn drop(&mut self) {
        self.interner.dropped(&self.data)
    }
}

#[derive(Derivative, Clone_, Dupe_, Display, Allocative)]
#[derivative(Debug(bound = "L: ::std::fmt::Debug"))]
#[display("{}", self.inner)]
pub struct SharedDirectory<L, H>
where
    H: DirectoryDigest,
{
    pub(super) inner: Arc<SharedDirectoryInner<L, H>>,
}

/// Weak reference to a [`SharedDirectory`].
pub struct SharedDirectoryWeak<L, H>
where
    H: DirectoryDigest,
{
    inner: Weak<SharedDirectoryInner<L, H>>,
}

impl<L, H: Pagable + DirectoryDigest> WeakErase for SharedDirectoryWeak<L, H>
where
    L: Pagable + SharedDirectoryInternable<H>,
{
    fn is_expired(&self) -> bool {
        self.inner.strong_count() == 0
    }

    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>> {
        self.inner
            .upgrade()
            .map(|inner| Box::new(SharedDirectory { inner }) as _)
    }
}

impl<L, H: Pagable + DirectoryDigest> PagableSerialize for SharedDirectory<L, H>
where
    L: Pagable + SharedDirectoryInternable<H>,
{
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de, L, H: Pagable + DirectoryDigest> PagableDeserialize<'de> for SharedDirectory<L, H>
where
    L: Pagable + SharedDirectoryInternable<H>,
{
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl<L, H: Pagable + DirectoryDigest> ArcErase for SharedDirectory<L, H>
where
    L: Pagable + SharedDirectoryInternable<H>,
{
    type Weak = SharedDirectoryWeak<L, H>;

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        Some(SharedDirectoryWeak {
            inner: Arc::downgrade(&self.inner),
        })
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<ArcSerializeOutcome> {
        self.inner.data.pagable_serialize(ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: pagable::PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> pagable::Result<Self> {
        let data = SharedDirectoryData::pagable_deserialize(deser)?;
        Ok(L::interner().intern(data))
    }
}

impl<L, H> SharedDirectory<L, H>
where
    H: DirectoryDigest,
{
    pub fn as_immutable(self) -> ImmutableDirectory<L, H> {
        ImmutableDirectory::Shared(self)
    }

    pub fn entries(
        &self,
    ) -> impl IntoIterator<Item = (&FileNameBuf, &DirectoryEntry<SharedDirectory<L, H>, L>)> + '_
    {
        &self.inner.data.entries
    }

    pub fn get<'a>(
        &'a self,
        needle: &'_ FileName,
    ) -> Option<DirectoryEntry<&'a SharedDirectory<L, H>, &'a L>> {
        self.inner
            .data
            .entries
            .get(needle)
            .as_ref()
            .map(|v| v.as_ref())
    }

    pub fn fingerprint(&self) -> &H {
        self.inner.data.fingerprint()
    }

    pub fn exhaustiveness_hash(&self) -> ExhaustivenessHash {
        self.inner.data.exhaustiveness_hash
    }

    pub fn size(&self) -> u64 {
        self.inner.data.size
    }

    /// The allocation behind this handle, for identity comparisons and ordering.
    pub fn allocation_ptr(&self) -> *const () {
        Arc::as_ptr(&self.inner) as *const ()
    }

    pub fn into_builder(self) -> DirectoryBuilder<L, H> {
        DirectoryBuilder::Immutable(self.as_immutable())
    }

    pub fn ptr_eq(&self, other: &SharedDirectory<L, H>) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<L, H> SharedDirectory<L, H>
where
    L: Clone,
    H: DirectoryDigest,
{
    pub fn collect_entries<C>(self) -> C
    where
        C: FromIterator<(FileNameBuf, DirectoryEntry<DirectoryBuilder<L, H>, L>)>,
    {
        self.entries()
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone().map_dir(|v| v.into_builder())))
            .collect()
    }

    pub fn into_entries(
        self,
    ) -> impl Iterator<Item = (FileNameBuf, DirectoryEntry<DirectoryBuilder<L, H>, L>)> {
        // Hard to convince the borrow checker that this is safe, so write this using indexing in
        // this slightly awkward way
        (0..self.inner.data.entries.len()).map(move |i| {
            let (k, v) = self.inner.data.entries.get_index(i).unwrap();
            (k.clone(), v.clone().map_dir(|v| v.into_builder()))
        })
    }
}

pub struct SharedDirectoryEntries<'a, L, H>(
    crate::directory::sorted_slice_map::Iter<
        'a,
        FileNameBuf,
        DirectoryEntry<SharedDirectory<L, H>, L>,
    >,
)
where
    H: DirectoryDigest;

impl<'a, L, H> Iterator for SharedDirectoryEntries<'a, L, H>
where
    H: DirectoryDigest,
{
    type Item = (
        &'a FileName,
        DirectoryEntry<&'a SharedDirectory<L, H>, &'a L>,
    );

    fn next(&mut self) -> Option<Self::Item> {
        let (name, entry) = self.0.next()?;
        Some((name, entry.as_ref()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a, L, H> DirectoryRef<'a> for &'a SharedDirectory<L, H>
where
    H: DirectoryDigest,
{
    type Leaf = L;
    type DirectoryDigest = H;
    type Entries = SharedDirectoryEntries<'a, L, H>;

    fn get(self, name: &FileName) -> Option<DirectoryEntry<Self, &'a Self::Leaf>> {
        self.get(name)
    }

    fn entries(self) -> Self::Entries {
        SharedDirectoryEntries(self.inner.data.entries.iter())
    }

    fn as_dyn(self) -> &'a dyn Directory<Self::Leaf, Self::DirectoryDigest> {
        self
    }
}

impl<'a, L, H> FingerprintedDirectoryRef<'a> for &'a SharedDirectory<L, H>
where
    H: DirectoryDigest,
{
    fn as_fingerprinted_dyn(
        self,
    ) -> &'a dyn FingerprintedDirectory<Self::Leaf, Self::DirectoryDigest> {
        self
    }
}

impl<L, H> Directory<L, H> for SharedDirectory<L, H>
where
    H: DirectoryDigest,
{
    type DirectoryRef<'a>
        = &'a SharedDirectory<L, H>
    where
        Self: Sized + 'a;

    fn as_ref<'a>(&'a self) -> Self::DirectoryRef<'a>
    where
        Self: Sized + 'a,
    {
        self
    }

    fn to_builder(&self) -> DirectoryBuilder<L, H>
    where
        L: Clone,
    {
        self.clone().into_builder()
    }
}

impl_fingerprinted_directory!(SharedDirectory);

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use dupe::Dupe;
    use pagable::PagableDeserialize;
    use pagable::PagableSerialize;
    use pagable::arc_erase::ArcErase;
    use pagable::arc_erase::WeakErase;
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;

    use crate::directory::entry::DirectoryEntry;
    use crate::directory::shared_directory::SharedDirectory;
    use crate::directory::shared_directory::SharedDirectoryInternable;
    use crate::directory::test::NopEntry;
    use crate::directory::test::TestDigest;
    use crate::directory::test::TestDirectoryBuilder;
    use crate::directory::test::TestHasher;
    use crate::directory::test::path;

    // These tests share the static NopEntry interner, so run them serially.
    static PAGABLE_SHARED_DIRECTORY_TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn pagable_preserves_shared_directory_identity() -> pagable::Result<()> {
        let _guard = PAGABLE_SHARED_DIRECTORY_TEST_MUTEX
            .lock()
            .expect("pagable shared directory test mutex should not be poisoned");

        let mut builder = TestDirectoryBuilder::empty_non_exhaustive();
        builder
            .insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))
            .expect("test path should insert");
        let interner = <NopEntry as SharedDirectoryInternable<TestDigest>>::interner();
        let dir = builder.fingerprint(&TestHasher).shared(&interner);

        let pair = (dir.dupe(), dir.dupe());
        let mut serializer = TestingSerializer::new();
        pair.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: (
            SharedDirectory<NopEntry, TestDigest>,
            SharedDirectory<NopEntry, TestDigest>,
        ) = PagableDeserialize::pagable_deserialize(&mut deserializer)?;

        assert!(restored.0.ptr_eq(&restored.1));
        Ok(())
    }

    #[test]
    fn pagable_shared_directory_weak_upgrades_while_alive() -> buck2_error::Result<()> {
        let _guard = PAGABLE_SHARED_DIRECTORY_TEST_MUTEX
            .lock()
            .expect("pagable shared directory test mutex should not be poisoned");

        let mut builder = TestDirectoryBuilder::empty_non_exhaustive();
        builder.insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))?;
        let interner = <NopEntry as SharedDirectoryInternable<TestDigest>>::interner();
        let dir = builder.fingerprint(&TestHasher).shared(&interner);

        let weak = ArcErase::downgrade(&dir).expect("shared directory should support downgrade");
        let upgraded = WeakErase::upgrade_weak(&weak)
            .expect("live shared directory should upgrade")
            .as_arc_any()
            .downcast_ref::<SharedDirectory<NopEntry, TestDigest>>()
            .expect("upgraded weak should be a shared directory")
            .dupe();

        assert!(dir.ptr_eq(&upgraded));
        assert!(!WeakErase::is_expired(&weak));

        drop(dir);
        drop(upgraded);

        assert!(WeakErase::is_expired(&weak));
        assert!(WeakErase::upgrade_weak(&weak).is_none());
        Ok(())
    }
}
