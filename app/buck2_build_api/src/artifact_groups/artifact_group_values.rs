/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_execute::artifact::group::artifact_group_values_dyn::ArtifactGroupValuesDyn;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::directory::INTERNER;
use buck2_execute::directory::LazyActionDirectoryBuilder;
use buck2_execute::directory::insert_artifact_lazy;
use buck2_hash::BuckMutSet;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use smallvec::SmallVec;
use smallvec::smallvec;

/// The [`ArtifactValue`]s for an [`crate::artifact_groups::ArtifactGroup`].
#[derive(Clone, Dupe, Allocative, Pagable)]
pub struct ArtifactGroupValues(pub(super) Arc<ArtifactGroupValuesData>);

impl ArtifactGroupValues {
    /// Create a new instance of ArtifactGroupValues for a TransitiveSetProjection. This expects
    /// that all the children *will* have a Directory.
    pub fn new(
        values: SmallVec<[(Artifact, ArtifactValue); 1]>,
        children: Vec<Self>,
        artifact_fs: &ArtifactFs,
        digest_config: DigestConfig,
    ) -> buck2_error::Result<Self> {
        let mut builder = LazyActionDirectoryBuilder::empty();

        for (artifact, value) in values.iter() {
            if artifact.path_resolution_requires_artifact_value() {
                let path = artifact
                    .resolve_path(artifact_fs, Some(&value.content_based_path_hash()))
                    .buck_error_context("Invalid artifact")?;
                insert_artifact_lazy(&mut builder, path, value)?;
            } else {
                let path = artifact
                    .resolve_path(artifact_fs, None)
                    .buck_error_context("Invalid artifact")?;
                insert_artifact_lazy(&mut builder, path, value)?;
            }
        }

        for child in children.iter() {
            // NOTE: Technically, we could fall back to iterating the artifacts in the
            // ArtifactGroupValues here, but we *do* rely on the fact that TransitiveSetProjections
            // produce intermediate directories, so if they don't, it is preferable to report it.
            let child_dir =
                child.0.directory.as_ref().ok_or_else(|| {
                    internal_error!("TransitiveSetProjection was missing directory!")
                })?;

            builder
                .merge(child_dir.dupe())
                .buck_error_context("Merge failed")?;
        }

        let directory = builder
            .finalize()?
            .fingerprint(digest_config.as_directory_serializer())
            .shared(&*INTERNER);

        Ok(Self(Arc::new(ArtifactGroupValuesData {
            values,
            children: children.into(),
            directory: Some(directory),
        })))
    }

    pub fn from_artifact(artifact: Artifact, value: ArtifactValue) -> Self {
        Self(Arc::new(ArtifactGroupValuesData {
            values: smallvec![(artifact, value)],
            children: MiniBoxSlice::new(),
            directory: None,
        }))
    }

    pub fn add_to_directory(
        &self,
        builder: &mut LazyActionDirectoryBuilder,
        artifact_fs: &ArtifactFs,
    ) -> buck2_error::Result<()> {
        if let Some(d) = self.0.directory.as_ref() {
            builder.merge(d.dupe())?;
            return Ok(());
        }

        for (artifact, value) in self.iter() {
            let projrel_path = artifact.resolve_path(
                artifact_fs,
                if artifact.path_resolution_requires_artifact_value() {
                    Some(value.content_based_path_hash())
                } else {
                    None
                }
                .as_ref(),
            )?;
            insert_artifact_lazy(builder, projrel_path, value)?;
        }

        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Artifact, ArtifactValue)> {
        TransitiveSetIterator::new(self)
    }

    pub fn iter_many<'a>(
        values: impl IntoIterator<Item = &'a Self>,
    ) -> impl Iterator<Item = &'a (Artifact, ArtifactValue)> {
        TransitiveSetIterator::new_many(values)
    }

    pub fn shallow_equals(&self, other: &Self) -> bool {
        let this = &self.0;
        let other = &other.0;

        this.values == other.values
            && this
                .children
                .iter()
                .eq_by(&other.children, |x, y| Arc::ptr_eq(&x.0, &y.0))
    }
}

#[derive(Allocative, Pagable)]
pub struct ArtifactGroupValuesData {
    pub(super) values: SmallVec<[(Artifact, ArtifactValue); 1]>,
    pub(super) children: MiniBoxSlice<ArtifactGroupValues>,
    /// If set, a precomputed directory represented the union of all values in this
    /// ArtifactGroupValuesData.
    pub(super) directory: Option<ActionSharedDirectory>,
}

/// An opaque identifier for the identity of a ArtifactGroupValue. There is no operation on this
/// that makes sense except for comparison.
#[derive(Hash, Eq, PartialEq)]
pub struct ArtifactValueIdentity(usize);

impl TransitiveSetContainer for ArtifactGroupValues {
    type Value = (Artifact, ArtifactValue);
    type Identity = ArtifactValueIdentity;

    fn values(&self) -> &[Self::Value] {
        &self.0.values
    }

    fn children(&self) -> &[Self] {
        &self.0.children
    }

    fn identity(&self) -> Self::Identity {
        ArtifactValueIdentity(Arc::as_ptr(&self.0) as usize)
    }
}

trait TransitiveSetContainer: Sized {
    type Value: Sized;
    type Identity: Hash + Eq + PartialEq;

    fn values(&self) -> &[Self::Value];

    fn children(&self) -> &[Self];

    fn identity(&self) -> Self::Identity;
}

struct TransitiveSetIterator<'a, C, V, I> {
    values: &'a [V],
    queue: Vec<&'a C>,
    seen: BuckMutSet<I>,
}

impl<'a, C>
    TransitiveSetIterator<
        'a,
        C,
        <C as TransitiveSetContainer>::Value,
        <C as TransitiveSetContainer>::Identity,
    >
where
    C: TransitiveSetContainer,
{
    fn new(container: &'a C) -> Self {
        let mut ret = Self {
            values: container.values(),
            queue: Vec::new(),
            seen: BuckMutSet::default(),
        };
        ret.enqueue_children(container.children());
        ret
    }

    fn new_many(containers: impl IntoIterator<Item = &'a C>) -> Self {
        let mut ret = Self {
            values: &[],
            queue: Vec::new(),
            seen: BuckMutSet::default(),
        };
        ret.enqueue_roots(containers);
        ret
    }

    fn enqueue_roots(&mut self, roots: impl IntoIterator<Item = &'a C>) {
        let roots = roots.into_iter().collect::<Vec<_>>();
        for t in roots.into_iter().rev() {
            if self.seen.insert(t.identity()) {
                self.queue.push(t);
            }
        }
    }

    fn enqueue_children(&mut self, transitive: &'a [C]) {
        for t in transitive.iter().rev() {
            if self.seen.insert(t.identity()) {
                self.queue.push(t);
            }
        }
    }
}

impl<'a, C> Iterator
    for TransitiveSetIterator<
        'a,
        C,
        <C as TransitiveSetContainer>::Value,
        <C as TransitiveSetContainer>::Identity,
    >
where
    C: TransitiveSetContainer,
{
    type Item = &'a <C as TransitiveSetContainer>::Value;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((v, rest)) = self.values.split_first() {
                self.values = rest;
                return Some(v);
            }

            let next = self.queue.pop()?;
            self.values = next.values();
            self.enqueue_children(next.children());
        }
    }
}

impl ArtifactGroupValuesDyn for ArtifactGroupValues {
    fn iter(&self) -> Box<dyn Iterator<Item = (&dyn ArtifactDyn, &ArtifactValue)> + '_> {
        Box::new(
            self.iter()
                .map(|(artifact, value)| (artifact as &dyn ArtifactDyn, value)),
        )
    }

    fn add_to_directory(
        &self,
        builder: &mut LazyActionDirectoryBuilder,
        artifact_fs: &ArtifactFs,
    ) -> buck2_error::Result<()> {
        self.add_to_directory(builder, artifact_fs)
    }
}

#[cfg(test)]
mod tests {
    use buck2_artifact::actions::key::ActionIndex;
    use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
    use buck2_artifact::artifact::build_artifact::BuildArtifact;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;

    use super::*;

    fn artifact(name: &str) -> (Artifact, ArtifactValue) {
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());

        let artifact = BuildArtifact::testing_new(target.dupe(), name, ActionIndex::new(0));

        let value = ArtifactValue::file(DigestConfig::testing_default().empty_file());

        (Artifact::from(artifact), value)
    }

    impl ArtifactGroupValuesData {
        fn value(mut self, v: &(Artifact, ArtifactValue)) -> Self {
            self.values.push((v.0.dupe(), v.1.dupe()));
            self
        }

        fn chain(mut self, child: &ArtifactGroupValues) -> Self {
            let mut children = std::mem::take(&mut self.children).into_vec();
            children.push(child.dupe());
            self.children = children.into();
            self
        }

        fn build(self) -> ArtifactGroupValues {
            ArtifactGroupValues(Arc::new(self))
        }
    }

    fn builder() -> ArtifactGroupValuesData {
        ArtifactGroupValuesData {
            values: Default::default(),
            children: Default::default(),
            directory: None,
        }
    }

    #[test]
    fn test_iter() {
        let a1 = artifact("a1");
        let a2 = artifact("a1");
        let a3 = artifact("a1");

        let v2 = builder().value(&a2).build();
        let v3 = builder().value(&a3).build();
        let values = builder().value(&a1).chain(&v2).chain(&v3).build();

        let mut iter = values.iter();
        assert_eq!(iter.next(), Some(&a1));
        assert_eq!(iter.next(), Some(&a2));
        assert_eq!(iter.next(), Some(&a3));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_shallow_eq() {
        let a1 = artifact("a1");
        let a2 = artifact("a1");
        let a3 = artifact("a1");

        let v2 = builder().value(&a2).build();
        let v3 = builder().value(&a3).build();

        {
            let s1 = builder().value(&a1).chain(&v2).chain(&v3).build();
            let s2 = builder().value(&a1).chain(&v2).chain(&v3).build();
            assert!(s1.shallow_equals(&s2));
        }

        {
            // Different artifacts
            let s1 = builder().value(&a1).chain(&v2).chain(&v3).build();
            let s2 = builder().chain(&v2).chain(&v3).build();
            assert!(!s1.shallow_equals(&s2));
        }

        {
            // Different children
            let s1 = builder().value(&a1).chain(&v2).chain(&v3).build();
            let s2 = builder().value(&a1).chain(&v2).build();
            assert!(!s1.shallow_equals(&s2));
        }
    }

    #[test]
    fn test_iter_many_dedups_shared_children() {
        let a1 = artifact("a1");
        let a2 = artifact("a2");
        let a3 = artifact("a3");

        let shared = builder().value(&a2).build();
        let r1 = builder().value(&a1).chain(&shared).build();
        let r2 = builder().value(&a3).chain(&shared).build();

        let all = ArtifactGroupValues::iter_many([&r1, &r2]).collect::<Vec<_>>();
        assert_eq!(all, vec![&a1, &a2, &a3]);
    }
}
