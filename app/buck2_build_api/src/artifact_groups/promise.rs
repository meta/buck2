/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::OnceLock;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark::codemap::FileSpan;
use starlark::values::Trace;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum PromiseArtifactResolveError {
    #[error("Resolved promise of the artifact promise was not an artifact (was `{0}`)")]
    NotAnArtifact(String),
    #[error("Artifact promise {1} {} wasn't resolved", maybe_declared_at(_0))]
    PromiseNotResolved(Option<FileSpan>, String),
    #[error("Artifact promise was resolved multiple times")]
    AlreadyResolved,
    #[error(
        "Artifact promise is not yet resolved, this indicates it was used as an artifact during the analysis where it was declared"
    )]
    PromiseNotYetResolved,
    #[error("Artifact promise resolved to artifact with associated artifacts, this isn't allowed")]
    HasAssociatedArtifacts,
    #[error(
        "Artifact promise resolved to a source artifact, this isn't allowed: an artifact promise must resolve to a build artifact"
    )]
    SourceArtifact,
    #[error(
        "assert_short_path() was called with `short_path = {0}`, but it did not match the artifact's actual short path: `{1}`"
    )]
    ShortPathMismatch(ForwardRelativePathBuf, String),
    #[error("Internal error: analysis result did not contain promise with ID ({0})")]
    NotFoundInAnalysis(PromiseArtifactId),
    #[error(
        "Internal error: promise artifact (id: {0}) owner is ({1}), which is not an anon target"
    )]
    OwnerIsNotAnonTarget(PromiseArtifactId, BaseDeferredKey),
    #[error(
        "Artifact promise resolved to artifact that uses content based paths. Call `actions.assert_has_content_based_path` on the promised artifact to assert that.\n  Promise ID: {0}\n  Artifact: {1}"
    )]
    UsesContentBasedPath(PromiseArtifactId, String),
    #[error(
        "Artifact promise resolved to artifact that does not use content based paths. Remove the `actions.assert_has_content_based_path` on the promised artifact.\n  Promise ID: {0}\n  Artifact: {1}"
    )]
    DoesNotUseContentBasedPath(PromiseArtifactId, String),
}

fn maybe_declared_at(location: &Option<FileSpan>) -> String {
    match location {
        Some(v) => format!(" (declared at {v})"),
        None => String::new(),
    }
}

/// A PromiseArtifact is used to assert that a starlark promise will resolve to an artifact. While the analysis
/// that declares the promise is running, the promise may not yet be resolved. In that case, operations that require
/// the underlying artifact will fail. Once that analysis is complete, all promises will be resolved and this will
/// have a reference to the fully resolved artifact.
#[derive(Clone, Debug, Dupe, Allocative, Pagable, Trace)]
pub struct PromiseArtifact {
    artifact: Arc<PromiseArtifactLock>,
    pub id: PromiseArtifactId,
}

#[derive(Debug, Allocative)]
pub struct PromiseArtifactLock(OnceLock<Artifact>);

impl From<Artifact> for PromiseArtifactLock {
    fn from(artifact: Artifact) -> Self {
        Self(artifact.into())
    }
}

impl PromiseArtifactLock {
    pub fn new() -> Self {
        Self(OnceLock::new())
    }
}

impl PagableSerialize for PromiseArtifactLock {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        // Currently page-out only runs when DICE is idle, at which point all promises should be resolved.
        self.0
            .get()
            .ok_or_else(|| pagable::Error::msg("Promise artifact not yet resolved"))?
            .pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for PromiseArtifactLock {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let artifact = Artifact::pagable_deserialize(deserializer)?;
        Ok(Self(artifact.into()))
    }
}

#[derive(
    Clone,
    Debug,
    Dupe,
    Allocative,
    Hash,
    Eq,
    PartialEq,
    PartialOrd,
    Ord,
    strong_hash::StrongHash,
    Pagable
)]
pub struct PromiseArtifactId {
    owner: BaseDeferredKey,
    id: usize,
}

impl PromiseArtifactId {
    pub fn new(owner: BaseDeferredKey, id: usize) -> PromiseArtifactId {
        Self { owner, id }
    }

    pub fn owner(&self) -> &BaseDeferredKey {
        &self.owner
    }
}

impl Display for PromiseArtifactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.owner, self.id)
    }
}

impl PromiseArtifact {
    pub fn new(artifact: Arc<PromiseArtifactLock>, id: PromiseArtifactId) -> Self {
        Self { artifact, id }
    }

    pub fn get_err(&self) -> buck2_error::Result<&Artifact> {
        match self.artifact.0.get() {
            Some(v) => Ok(v),
            None => Err(PromiseArtifactResolveError::PromiseNotYetResolved.into()),
        }
    }

    pub fn unwrap(&self) -> &Artifact {
        self.get_err().unwrap()
    }

    pub fn get(&self) -> Option<&Artifact> {
        self.artifact.0.get()
    }

    pub fn resolve(
        &self,
        artifact: Artifact,
        expected_short_path: &Option<ForwardRelativePathBuf>,
        promise_has_content_based_path: bool,
    ) -> buck2_error::Result<()> {
        let bound = artifact;
        if bound.is_source() {
            return Err(PromiseArtifactResolveError::SourceArtifact.into());
        }

        let artifact_has_content_based_path = bound.path_resolution_requires_artifact_value();
        if artifact_has_content_based_path && !promise_has_content_based_path {
            return Err(PromiseArtifactResolveError::UsesContentBasedPath(
                self.id.clone(),
                format!("{}", bound),
            )
            .into());
        } else if !artifact_has_content_based_path && promise_has_content_based_path {
            return Err(PromiseArtifactResolveError::DoesNotUseContentBasedPath(
                self.id.clone(),
                format!("{}", bound),
            )
            .into());
        }

        if let Some(expected_short_path) = expected_short_path {
            bound.get_path().with_short_path(|artifact_short_path| {
                if artifact_short_path != expected_short_path {
                    Err(buck2_error::Error::from(
                        PromiseArtifactResolveError::ShortPathMismatch(
                            expected_short_path.clone(),
                            artifact_short_path.to_string(),
                        ),
                    ))
                } else {
                    Ok(())
                }
            })?;
        }
        match self.artifact.0.set(bound) {
            Ok(_) => Ok(()),
            Err(_) => Err(PromiseArtifactResolveError::AlreadyResolved.into()),
        }
    }

    pub fn id(&self) -> &PromiseArtifactId {
        &self.id
    }

    pub fn owner(&self) -> &BaseDeferredKey {
        &self.id.owner
    }
}

impl Display for PromiseArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(artifact) = self.artifact.0.get() {
            write!(f, "PromiseArtifact(Resolved({artifact}))")
        } else {
            write!(
                f,
                "PromiseArtifact(Unresolved({}, {}))",
                self.id.owner, self.id.id
            )
        }
    }
}

impl Hash for PromiseArtifact {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialEq for PromiseArtifact {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for PromiseArtifact {}

// When passing promise artifacts into anon targets, we will coerce them into this type.
// During resolve, we look up the analysis of the target that produced the promise artifact,
// assert short paths, and produce a new `StarlarkPromiseArtifact` with the `OnceLock` resolved.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct PromiseArtifactAttr {
    pub id: PromiseArtifactId,
    pub short_path: Option<ForwardRelativePathBuf>,
    pub has_content_based_path: bool,
}

impl fmt::Display for PromiseArtifactAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO(@wendyy) - we should figure out what to do about the declaration location.
        // It's possible that 2 targets produce the same promise artifact and try to pass
        // it into a downstream target, so then there would be 2 declaration locations.
        write!(
            f,
            "<promise artifact attr (id = {}, has_content_based_path = {})",
            self.id, self.has_content_based_path
        )?;
        if let Some(short_path) = &self.short_path {
            write!(f, " with short_path `{short_path}`")?;
        }
        write!(f, ">")?;
        Ok(())
    }
}
