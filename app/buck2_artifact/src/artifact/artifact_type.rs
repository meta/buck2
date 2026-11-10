/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Ref;
use std::cell::RefCell;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_util::arc_str::ThinArcS;
use buck2_util::size_assert;
use derivative::Derivative;
use derive_more::Display;
use derive_more::From;
use dupe::Dupe;
use either::Either;
use pagable::Pagable;
use starlark::StarlarkPagablePanic;
use starlark::values::Heap;
use starlark::values::ProvidesStaticType;
use starlark::values::Trace;
use starlark::values::ValueTyped;
use starlark::values::any_complex::StarlarkAnyComplex;
use starlark_map::Hashed;

use crate::actions::key::ActionKey;
use crate::artifact::artifact_dyn::ArtifactDyn;
use crate::artifact::artifact_path::ArtifactPath;
use crate::artifact::artifact_path::BuildArtifactPathRef;
use crate::artifact::build_artifact::BuildArtifact;
use crate::artifact::output_type::OutputType;
use crate::artifact::source_artifact::SourceArtifact;

/// An 'Artifact' that can be materialized at its path. The underlying data is not very large here,
/// but we do store many copies of it, which is why we store this as an Arc.
#[derive(
    Clone,
    Debug,
    Display,
    Dupe,
    Allocative,
    Derivative,
    PartialEq,
    Eq,
    Hash,
    strong_hash::StrongHash,
    Pagable,
    starlark::values::StarlarkPagableViaPagable
)]
pub struct Artifact(Arc<ArtifactData>);

#[derive(
    Clone,
    Debug,
    Display,
    Dupe,
    Allocative,
    Hash,
    Eq,
    PartialEq,
    strong_hash::StrongHash,
    Pagable
)]
#[display("{}", data)]
struct ArtifactData {
    data: Hashed<ArtifactKind>,

    /// The number of components at the prefix of that path that are internal details to the rule,
    /// not returned by `.short_path`.
    hidden_components_count: usize,
}

size_assert::words_of_type!(ArtifactData, 9);

impl Artifact {
    pub fn new(
        artifact: impl Into<BaseArtifactKind>,
        projected_path: ThinArcS<ForwardRelativePath>,
        hidden_components_count: usize,
    ) -> Self {
        let artifact = ArtifactKind {
            base: artifact.into(),
            path: projected_path,
        };
        Self(Arc::new(ArtifactData {
            data: Hashed::new(artifact),
            hidden_components_count,
        }))
    }

    /// Allocates a new `OutputArtifact` for the given artifact.
    ///
    /// This is almost always wrong to call - `Artifact`s are generally already bound, making an
    /// output artifact for them makes little sense.
    ///
    /// Returns `None` if this is not a build artifact
    pub fn allocate_new_output_artifact_for<'v>(
        &self,
        heap: Heap<'v>,
    ) -> Option<OutputArtifact<'v>> {
        let key = self.0.data.key();
        match &key.base {
            BaseArtifactKind::Source(_) => None,
            BaseArtifactKind::Build(artifact) => Some({
                let artifact = StarlarkAnyComplex::new(DeclaredArtifactCell(RefCell::new(
                    DeclaredArtifactKind::Bound(artifact.dupe()),
                )));
                DeclaredArtifact {
                    artifact: StarlarkAnyComplex::alloc_no_freeze(artifact, heap),
                    projected_path: key.path.dupe(),
                    hidden_components_count: self.0.hidden_components_count,
                }
                .into()
            }),
        }
    }

    pub fn data(&self) -> &ArtifactKind {
        self.0.data.key()
    }

    pub fn is_source(&self) -> bool {
        match self.as_parts().0 {
            BaseArtifactKind::Source(_) => true,
            BaseArtifactKind::Build(_) => false,
        }
    }

    pub fn get_source(&self) -> Option<SourceArtifact> {
        match self.as_parts().0 {
            BaseArtifactKind::Source(x) => Some(x.dupe()),
            BaseArtifactKind::Build(_) => None,
        }
    }

    /// The callsite that declared this artifact, any.
    pub fn owner(&self) -> Option<&BaseDeferredKey> {
        match self.as_parts().0 {
            BaseArtifactKind::Source(_) => None,
            BaseArtifactKind::Build(b) => Some(b.get_path().owner().owner()),
        }
    }

    /// The action that would produce this artifact, if any.
    pub fn action_key(&self) -> Option<&ActionKey> {
        match self.as_parts().0 {
            BaseArtifactKind::Source(_) => None,
            BaseArtifactKind::Build(b) => Some(b.key()),
        }
    }

    pub fn as_parts(&self) -> (&BaseArtifactKind, &ForwardRelativePath) {
        let key = self.0.data.key();
        (&key.base, &key.path)
    }

    pub fn get_path(&self) -> ArtifactPath<'_> {
        let (base, projected_path) = self.as_parts();

        let base_path = match base {
            BaseArtifactKind::Build(b) => Either::Left(BuildArtifactPathRef::Direct(b.get_path())),
            BaseArtifactKind::Source(s) => Either::Right(s.get_path()),
        };

        ArtifactPath {
            base_path,
            projected_path,
            hidden_components_count: self.0.hidden_components_count,
        }
    }

    pub fn project(&self, path: &ForwardRelativePath, hide_prefix: bool) -> Artifact {
        if path.is_empty() {
            return self.dupe();
        }

        let hidden_components_count = self.0.hidden_components_count
            + if hide_prefix {
                self.get_path().with_short_path(|p| p.iter().count())
            } else {
                0
            };

        let (base, already_projected) = self.as_parts();

        let projected = already_projected.join(path);

        Self::new(
            base.dupe(),
            ThinArcS::from(projected.as_ref()),
            hidden_components_count,
        )
    }

    pub fn path_resolution_requires_artifact_value(&self) -> bool {
        match self.as_parts().0 {
            BaseArtifactKind::Source(_) => false,
            BaseArtifactKind::Build(b) => b.get_path().is_content_based_path(),
        }
    }

    pub fn has_configuration_based_path(&self) -> bool {
        match self.as_parts().0 {
            BaseArtifactKind::Source(_) => false,
            BaseArtifactKind::Build(b) => b.get_path().is_configuration_based_path(),
        }
    }
}

impl ArtifactDyn for Artifact {
    fn resolve_path(
        &self,
        fs: &ArtifactFs,
        content_hash: Option<&ContentBasedPathHash>,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        self.get_path().resolve(fs, content_hash)
    }

    fn resolve_configuration_hash_path(
        &self,
        fs: &ArtifactFs,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        self.get_path().resolve_configuration_hash_path(fs)
    }

    fn requires_materialization(&self, fs: &ArtifactFs) -> bool {
        let Some(source_artifact) = self.get_source() else {
            return true;
        };
        let path = source_artifact.get_path();
        fs.cell_resolver()
            .get(path.package().cell_name())
            .unwrap()
            .external()
            .is_some()
    }

    fn has_content_based_path(&self) -> bool {
        self.path_resolution_requires_artifact_value()
    }

    fn is_projected(&self) -> bool {
        !self.as_parts().1.is_empty()
    }
}

#[derive(
    Clone,
    Debug,
    Display,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    From,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub enum BaseArtifactKind {
    Source(SourceArtifact),
    Build(BuildArtifact),
}

size_assert::words_of_type!(BaseArtifactKind, 6);

#[derive(
    Clone,
    Debug,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct ArtifactKind {
    pub base: BaseArtifactKind,
    /// When non-empty, the artifact is considered "projected".
    pub path: ThinArcS<ForwardRelativePath>,
}

impl Display for ArtifactKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.base)
        } else {
            write!(f, "{}/{}", self.base, self.path)
        }
    }
}

size_assert::words_of_type!(ArtifactKind, 7);

impl From<SourceArtifact> for Artifact {
    fn from(a: SourceArtifact) -> Self {
        Self::new(a, ThinArcS::from(ForwardRelativePath::empty()), 0)
    }
}

impl From<BuildArtifact> for Artifact {
    fn from(a: BuildArtifact) -> Self {
        Self::new(a, ThinArcS::from(ForwardRelativePath::empty()), 0)
    }
}

/// An intermediate struct to respond to calls to `ensure_bound`.
#[derive(Clone, Dupe, Debug, Display, Allocative, Hash, Eq, PartialEq, Trace)]
#[display("{}", self.get_path())]
pub struct BoundBuildArtifact {
    artifact: BuildArtifact,
    projected_path: ThinArcS<ForwardRelativePath>,
    hidden_components_count: usize,
}

impl BoundBuildArtifact {
    pub fn into_artifact(self) -> Artifact {
        Artifact::new(
            self.artifact,
            self.projected_path,
            self.hidden_components_count,
        )
    }

    pub fn as_base_artifact(&self) -> &BuildArtifact {
        &self.artifact
    }

    pub fn action_key(&self) -> &ActionKey {
        self.artifact.key()
    }

    pub fn get_path(&self) -> ArtifactPath<'_> {
        ArtifactPath {
            base_path: Either::Left(BuildArtifactPathRef::Direct(self.artifact.get_path())),
            projected_path: &self.projected_path,
            hidden_components_count: self.hidden_components_count,
        }
    }
}

/// An artifact that is "declared" by a rule implementation, which will be an artifact that can be
/// made available by running some action created by that rule implementation.
///
/// This type should only exist within the concept of a rule implementation.
///
/// The 'DeclaredArtifact' can be either already "bound" if there is already an action attached,
/// in which case it is underneath a 'BuildArtifact', or be unbound if there is no action attached
/// to it yet, in which case it is a 'UnboundArtifact' underneath.
///
/// All 'DeclaredArtifact's are forced to be bound at the end of the analysis phase.
#[derive(Clone, Debug, Dupe, Display, Trace, Allocative)]
#[display("{}", self.get_path())]
pub struct DeclaredArtifact<'v> {
    /// Allocation here is not optimization: `DeclaredArtifactKind` is a shared mutable state.
    artifact: ValueTyped<'v, StarlarkAnyComplex<DeclaredArtifactCell>>,
    projected_path: ThinArcS<ForwardRelativePath>,
    hidden_components_count: usize,
}

impl<'v> DeclaredArtifact<'v> {
    pub fn new(
        path: BuildArtifactPath,
        output_type: OutputType,
        hidden_components_count: usize,
        heap: Heap<'v>,
    ) -> DeclaredArtifact<'v> {
        let artifact = StarlarkAnyComplex::new(DeclaredArtifactCell(RefCell::new(
            DeclaredArtifactKind::Unbound(UnboundArtifact(path, output_type)),
        )));
        DeclaredArtifact {
            artifact: StarlarkAnyComplex::alloc_no_freeze(artifact, heap),
            projected_path: ThinArcS::from(ForwardRelativePath::empty()),
            hidden_components_count,
        }
    }

    fn artifact(&self) -> &'v RefCell<DeclaredArtifactKind> {
        &self.artifact.as_ref().value.0
    }

    pub fn project(&self, path: &ForwardRelativePath, hide_prefix: bool) -> Self {
        if path.is_empty() {
            return self.dupe();
        }

        let hidden_components_count = self.hidden_components_count
            + if hide_prefix {
                self.get_path().with_short_path(|p| p.iter().count())
            } else {
                0
            };

        Self {
            artifact: self.artifact,
            projected_path: ThinArcS::from(self.projected_path.join(path).as_ref()),
            hidden_components_count,
        }
    }

    pub fn as_output(&self) -> OutputArtifact<'v> {
        OutputArtifact(self.dupe())
    }

    pub fn get_path(&self) -> ArtifactPath<'_> {
        let borrow = self.artifact().borrow();

        let projected_path = &self.projected_path;

        let base_path = Ref::map(borrow, |a| match &a {
            DeclaredArtifactKind::Bound(a) => a.get_path(),
            DeclaredArtifactKind::Unbound(a) => &a.0,
        });

        ArtifactPath {
            base_path: Either::Left(BuildArtifactPathRef::RefCell(base_path)),
            projected_path,
            hidden_components_count: self.hidden_components_count,
        }
    }

    pub fn output_type(&self) -> OutputType {
        match &*self.artifact().borrow() {
            DeclaredArtifactKind::Bound(x) => x.output_type(),
            DeclaredArtifactKind::Unbound(x) => x.1,
        }
    }

    /// Ensure that the artifact is bound.
    ///
    /// This is called before we freeze the artifacts by the artifact registry.
    /// When we freeze `DeclaredArtifact`, we then just call `get_bound_artifact()`, and `expect`
    /// it to be valid. We have the `ensure_bound` method to make sure that we can return
    /// a friendlier message to users because `freeze()` does not return error messages
    pub fn ensure_bound(self) -> buck2_error::Result<BoundBuildArtifact> {
        let borrow = self.artifact().borrow();

        let artifact = match &*borrow {
            DeclaredArtifactKind::Bound(built) => built.dupe(),
            DeclaredArtifactKind::Unbound(unbound) => {
                return Err(ArtifactErrors::UnboundArtifact(unbound.dupe()).into());
            }
        };

        Ok(BoundBuildArtifact {
            artifact,
            projected_path: self.projected_path,
            hidden_components_count: self.hidden_components_count,
        })
    }

    pub fn is_bound(&self) -> bool {
        self.artifact().borrow().is_bound()
    }

    pub fn owner(&self) -> Option<BaseDeferredKey> {
        match &*self.artifact().borrow() {
            DeclaredArtifactKind::Bound(b) => Some(b.get_path().owner().owner().dupe()),
            DeclaredArtifactKind::Unbound(_) => None,
        }
    }

    pub fn has_content_based_path(&self) -> bool {
        match &*self.artifact().borrow() {
            DeclaredArtifactKind::Bound(b) => b.get_path().is_content_based_path(),
            DeclaredArtifactKind::Unbound(b) => b.0.is_content_based_path(),
        }
    }

    pub fn has_configuration_based_path(&self) -> bool {
        match &*self.artifact().borrow() {
            DeclaredArtifactKind::Bound(b) => b.get_path().is_configuration_based_path(),
            DeclaredArtifactKind::Unbound(b) => b.0.is_configuration_based_path(),
        }
    }
}

impl Hash for DeclaredArtifact<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get_path().hash(state)
    }
}

impl PartialEq for DeclaredArtifact<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.get_path() == other.get_path()
    }
}

impl Eq for DeclaredArtifact<'_> {}

/// A 'DeclaredArtifact' can be either "bound" to an 'Action', or "unbound"
#[derive(Debug, Display, Allocative, ProvidesStaticType, Trace)]
enum DeclaredArtifactKind {
    Bound(BuildArtifact),
    Unbound(UnboundArtifact),
}

/// Local newtype wrapper so `StarlarkAnyComplex<_>` has a local payload type
/// we can register a typing vtable entry for (the orphan rule forbids
/// implementing the payload marker for `RefCell<LocalT>` directly).
#[derive(Debug, Allocative, ProvidesStaticType, Trace, StarlarkPagablePanic)]
#[repr(transparent)]
pub struct DeclaredArtifactCell(RefCell<DeclaredArtifactKind>);

starlark::register_starlark_any_complex!(DeclaredArtifactCell);

impl DeclaredArtifactKind {
    pub fn is_bound(&self) -> bool {
        match self {
            DeclaredArtifactKind::Bound(_) => true,
            DeclaredArtifactKind::Unbound(_) => false,
        }
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
pub enum ArtifactErrors {
    #[error("Attempted to bind an artifact which was already bound\n  Artifact: {0}")]
    DuplicateBind(Artifact),
    #[error(
        "Artifact must be bound by now. If you are intending to use this artifact as the output of `run`, are you missing an `.as_output()` call?\n  Artifact: {0}"
    )]
    UnboundArtifact(UnboundArtifact),
}

/// An artifact that is marked as the output of a particular 'Action'.
#[derive(Clone, Debug, Display, Dupe, Hash, PartialEq, Eq, Allocative, Trace)]
pub struct OutputArtifact<'v>(DeclaredArtifact<'v>);

impl<'v> From<DeclaredArtifact<'v>> for OutputArtifact<'v> {
    fn from(artifact: DeclaredArtifact<'v>) -> Self {
        Self(artifact)
    }
}

impl<'v> OutputArtifact<'v> {
    pub fn bind(&self, key: ActionKey) -> buck2_error::Result<BoundBuildArtifact> {
        match &mut *self.0.artifact().borrow_mut() {
            DeclaredArtifactKind::Bound(a) => {
                // NOTE: If the artifact was already bound to the same action, we leave it alone.
                // This can happen when we have projected artifacts used in a command: we'll visit
                // the projected artifacts and then try to bind each of them, but the same
                // underlying artifact is the one that gets bound.
                if *a.key() != key {
                    return Err(ArtifactErrors::DuplicateBind(a.dupe().into()).into());
                }
            }
            a => take_mut::take(a, |artifact| match artifact {
                DeclaredArtifactKind::Unbound(unbound) => {
                    DeclaredArtifactKind::Bound(unbound.bind(key).unwrap())
                }
                DeclaredArtifactKind::Bound(_) => {
                    unreachable!("should already be verified to be unbound")
                }
            }),
        };

        let artifact = match &*self.0.artifact().borrow() {
            DeclaredArtifactKind::Bound(b) => b.dupe(),
            _ => unreachable!("should already be bound"),
        };

        Ok(BoundBuildArtifact {
            artifact,
            projected_path: self.0.projected_path.dupe(),
            hidden_components_count: self.0.hidden_components_count,
        })
    }

    pub fn ensure_output_type(&self, output_type: OutputType) -> buck2_error::Result<()> {
        output_type.check_path(self, self.0.output_type())
    }

    pub fn path_resolution_method(&self) -> BuckOutPathKind {
        match &*self.0.artifact().borrow() {
            DeclaredArtifactKind::Bound(b) => b.get_path().path_resolution_method(),
            DeclaredArtifactKind::Unbound(u) => u.0.path_resolution_method(),
        }
    }
}

impl<'v> Deref for OutputArtifact<'v> {
    type Target = DeclaredArtifact<'v>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Dupe, Debug, Display, Allocative)]
#[display("{}", self.0)]
pub struct UnboundArtifact(BuildArtifactPath, OutputType);

impl UnboundArtifact {
    fn bind(self, key: ActionKey) -> buck2_error::Result<BuildArtifact> {
        BuildArtifact::new(self.0, key, self.1)
    }
}

pub mod testing {
    use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
    use buck2_core::deferred::key::DeferredHolderKey;
    use buck2_core::fs::buck_out_path::BuckOutPathKind;
    use buck2_core::fs::buck_out_path::BuildArtifactPath;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    use dupe::Dupe;

    use crate::actions::key::ActionIndex;
    use crate::actions::key::ActionKey;
    use crate::artifact::artifact_type::DeclaredArtifact;
    use crate::artifact::artifact_type::DeclaredArtifactKind;
    use crate::artifact::build_artifact::BuildArtifact;
    use crate::artifact::output_type::OutputType;

    pub trait ArtifactTestingExt {
        fn testing_is_bound(&self) -> bool;

        fn testing_action_key(&self) -> Option<ActionKey>;
    }

    impl ArtifactTestingExt for DeclaredArtifact<'_> {
        fn testing_is_bound(&self) -> bool {
            match &*self.artifact().borrow() {
                DeclaredArtifactKind::Bound(_) => true,
                DeclaredArtifactKind::Unbound(_) => false,
            }
        }

        fn testing_action_key(&self) -> Option<ActionKey> {
            match &*self.artifact().borrow() {
                DeclaredArtifactKind::Bound(built) => Some(built.key().dupe()),
                DeclaredArtifactKind::Unbound(_) => None,
            }
        }
    }

    impl ArtifactTestingExt for BuildArtifact {
        fn testing_is_bound(&self) -> bool {
            true
        }

        fn testing_action_key(&self) -> Option<ActionKey> {
            Some(self.key().dupe())
        }
    }

    pub trait BuildArtifactTestingExt {
        fn testing_new(target: ConfiguredTargetLabel, path: &str, id: ActionIndex)
        -> BuildArtifact;
    }

    impl BuildArtifactTestingExt for BuildArtifact {
        fn testing_new(
            target: ConfiguredTargetLabel,
            path: &str,
            id: ActionIndex,
        ) -> BuildArtifact {
            BuildArtifact::new(
                BuildArtifactPath::new(
                    BaseDeferredKey::TargetLabel(target.dupe()),
                    ForwardRelativePath::new(path).unwrap().to_buf(),
                    BuckOutPathKind::default(),
                ),
                ActionKey::new(
                    DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target)),
                    id,
                ),
                OutputType::File,
            )
            .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
    use buck2_core::deferred::key::DeferredHolderKey;
    use buck2_core::fs::artifact_path_resolver::ArtifactFs;
    use buck2_core::fs::buck_out_path::BuckOutPathKind;
    use buck2_core::fs::buck_out_path::BuckOutPathResolver;
    use buck2_core::fs::buck_out_path::BuildArtifactPath;
    use buck2_core::fs::project::ProjectRoot;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_core::package::source_path::SourcePath;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_fs::fs_util::uncategorized as fs_util;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    use buck2_util::arc_str::ThinArcS;
    use dupe::Dupe;
    use starlark::values::Heap;

    use crate::actions::key::ActionIndex;
    use crate::actions::key::ActionKey;
    use crate::artifact::artifact_type::Artifact;
    use crate::artifact::artifact_type::DeclaredArtifact;
    use crate::artifact::artifact_type::DeclaredArtifactKind;
    use crate::artifact::artifact_type::testing::BuildArtifactTestingExt;
    use crate::artifact::build_artifact::BuildArtifact;
    use crate::artifact::output_type::OutputType;
    use crate::artifact::source_artifact::SourceArtifact;

    #[test]
    fn artifact_binding() -> buck2_error::Result<()> {
        Heap::temp(|heap| {
            let target = ConfiguredTargetLabel::testing_parse(
                "cell//pkg:foo",
                ConfigurationData::testing_new(),
            );
            let declared = DeclaredArtifact::new(
                BuildArtifactPath::new(
                    BaseDeferredKey::TargetLabel(target.dupe()),
                    ForwardRelativePathBuf::unchecked_new("bar.out".into()),
                    BuckOutPathKind::default(),
                ),
                OutputType::File,
                0,
                heap,
            );
            let key = ActionKey::new(
                DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target.dupe())),
                ActionIndex::new(0),
            );

            let out = declared.as_output();
            let bound = out.bind(key.dupe())?;

            assert_eq!(*bound.as_base_artifact().key(), key);
            assert_eq!(bound.get_path(), declared.get_path());

            match &*declared.artifact().borrow() {
                DeclaredArtifactKind::Bound(b) => {
                    assert_eq!(b, bound.as_base_artifact());
                }
                _ => panic!("should be bound"),
            };

            // Binding again to the same key should succeed
            out.bind(key)?;

            // Binding again to a different key should fail
            let other_key = ActionKey::new(
                DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target)),
                ActionIndex::new(1),
            );

            assert_matches!(out.bind(other_key), Err(..));

            Ok(())
        })
    }

    #[test]
    fn resolve_artifact() -> buck2_error::Result<()> {
        let source = SourceArtifact::new(SourcePath::testing_new("cell//pkg", "src.cpp"));

        let project_fs =
            ProjectRoot::new(AbsNormPathBuf::try_from(std::env::current_dir().unwrap()).unwrap())
                .unwrap();
        let fs = ArtifactFs::new(
            CellResolver::testing_with_name_and_path(
                CellName::testing_new("cell"),
                CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("cell_path".into())),
            ),
            BuckOutPathResolver::new(ProjectRelativePathBuf::unchecked_new("buck_out".into())),
            project_fs,
        );

        assert_eq!(
            &*Artifact::from(source).get_path().resolve(&fs, None)?,
            ProjectRelativePath::unchecked_new("cell_path/pkg/src.cpp")
        );

        Ok(())
    }

    #[test]
    fn writes_files() -> buck2_error::Result<()> {
        let project_root = ProjectRootTemp::new().unwrap();
        let project_fs = project_root.path();

        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());

        let artifact1 =
            BuildArtifact::testing_new(target.dupe(), "foo/bar.cpp", ActionIndex::new(0));
        let artifact2 = BuildArtifact::testing_new(target.dupe(), "foo/bar.h", ActionIndex::new(0));
        let artifact3 =
            BuildArtifact::testing_new(target, "foo/bar.cpp/invalid_file.txt", ActionIndex::new(1));

        let fs = ArtifactFs::new(
            CellResolver::testing_with_name_and_path(
                CellName::testing_new("cell"),
                CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("cell_path".into())),
            ),
            BuckOutPathResolver::new(ProjectRelativePathBuf::unchecked_new("buck_out".into())),
            project_fs.dupe(),
        );
        let expected_path1 = project_fs.resolve(fs.resolve_build(artifact1.get_path(), None)?);
        let expected_path2 = project_fs.resolve(fs.resolve_build(artifact2.get_path(), None)?);

        let dest_path = fs.resolve_build(artifact1.get_path(), None)?;
        fs.fs().write_file(&dest_path, "artifact1", false)?;

        assert_eq!("artifact1", fs_util::read_to_string(&expected_path1)?);

        let dest_path = fs.resolve_build(artifact2.get_path(), None)?;
        fs.fs().write_file(&dest_path, "artifact2", true)?;

        assert_eq!("artifact2", fs_util::read_to_string(&expected_path2)?);

        let dest_path = fs.resolve_build(artifact3.get_path(), None)?;
        fs.fs()
            .write_file(&dest_path, "artifact3", false)
            .expect_err("should fail because bar.cpp is a file");

        #[cfg(unix)]
        {
            // Check executable bit on unix
            use std::os::unix::fs::PermissionsExt;
            // Unix permission bits
            let artifact1_executable =
                fs_util::metadata(expected_path1)?.permissions().mode() & 0o100 != 0;
            let artifact2_executable =
                fs_util::metadata(expected_path2)?.permissions().mode() & 0o100 != 0;

            assert!(!artifact1_executable);
            assert!(artifact2_executable);
        }

        Ok(())
    }

    #[test]
    fn test_short_path() -> buck2_error::Result<()> {
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:foo", ConfigurationData::testing_new());

        let artifact =
            BuildArtifact::testing_new(target.dupe(), "foo/bar.cpp", ActionIndex::new(0));

        let full = Artifact::new(
            artifact.clone(),
            ThinArcS::from(ForwardRelativePath::empty()),
            0,
        );
        let hidden = Artifact::new(artifact, ThinArcS::from(ForwardRelativePath::empty()), 1);

        full.get_path()
            .with_full_path(|p| assert_eq!(p.as_str(), "foo/bar.cpp"));

        full.get_path()
            .with_short_path(|p| assert_eq!(p.as_str(), "foo/bar.cpp"));

        hidden
            .get_path()
            .with_full_path(|p| assert_eq!(p.as_str(), "foo/bar.cpp"));

        hidden
            .get_path()
            .with_short_path(|p| assert_eq!(p.as_str(), "bar.cpp"));

        Ok(())
    }
}
