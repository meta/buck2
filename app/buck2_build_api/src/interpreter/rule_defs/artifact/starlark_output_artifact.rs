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
use std::fmt::Debug;
use std::fmt::Display;

use allocative::Allocative;
use buck2_artifact::artifact::artifact_path::ArtifactPath;
use buck2_artifact::artifact::artifact_type::BaseArtifactKind;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use dupe::Dupe;
use either::Either;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::AllocFrozenValue;
use starlark::values::AllocValue;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::FrozenHeap;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark_map::StarlarkHasher;

use crate::interpreter::rule_defs::artifact::methods::any_artifact_methods;
use crate::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::ArtifactFingerprint;
use crate::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactLike;
use crate::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;

/// Thin wrapper around `OutputArtifact`.
///
/// Allows actions to distinguish between inputs and outputs, and can validate whether the
/// underlying artifact is bound or not yet.
#[derive(Debug, Clone, Dupe, ProvidesStaticType, Trace, NoSerialize, Allocative)]
pub struct StarlarkOutputArtifact<'v> {
    declared_artifact: ValueTyped<'v, StarlarkDeclaredArtifact<'v>>,
}

#[derive(
    Debug,
    Clone,
    Dupe,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
pub struct FrozenStarlarkOutputArtifact<'v> {
    artifact: ValueTyped<'v, StarlarkArtifact>,
}

impl<'v> Freeze<'v> for StarlarkOutputArtifact<'v> {
    type Frozen<'fv> = FrozenStarlarkOutputArtifact<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(FrozenStarlarkOutputArtifact {
            artifact: Freeze::freeze(self.declared_artifact, freezer)?,
        })
    }
}

impl<'v> AllocValue<'v> for StarlarkOutputArtifact<'v> {
    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

impl<'fv> AllocFrozenValue<'fv> for FrozenStarlarkOutputArtifact<'fv> {
    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        heap.alloc_simple_typed(self).to_value()
    }
}

/// An `OutputArtifact` in either of its representations: the receiver of the type's methods,
/// and the binding for a native parameter of type `OutputArtifact`.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum StarlarkOutputArtifactView<'v> {
    Unfrozen(ValueTyped<'v, StarlarkOutputArtifact<'v>>),
    Frozen(ValueTyped<'v, FrozenStarlarkOutputArtifact<'v>>),
}

impl<'v> StarlarkOutputArtifactView<'v> {
    pub(crate) fn as_artifact_like(self) -> &'v dyn StarlarkArtifactLike<'v> {
        match self {
            StarlarkOutputArtifactView::Unfrozen(x) => x.as_ref(),
            StarlarkOutputArtifactView::Frozen(x) => x.as_ref(),
        }
    }
}

impl<'v> Display for StarlarkOutputArtifact<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<output artifact for {}>", self.get_path())
    }
}

impl<'v> Display for FrozenStarlarkOutputArtifact<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<output artifact for {}>", self.get_path())
    }
}

impl<'v> StarlarkOutputArtifact<'v> {
    pub fn new(v: ValueTyped<'v, StarlarkDeclaredArtifact<'v>>) -> Self {
        Self {
            declared_artifact: v,
        }
    }

    pub(crate) fn inner(&self) -> ValueTyped<'v, StarlarkDeclaredArtifact<'v>> {
        self.declared_artifact
    }

    pub fn artifact(&self) -> OutputArtifact<'v> {
        self.inner().output_artifact()
    }

    fn get_path(&self) -> ArtifactPath<'v> {
        self.declared_artifact.as_ref().artifact.get_path()
    }
}

impl<'v> FrozenStarlarkOutputArtifact<'v> {
    pub fn inner(&self) -> ValueTyped<'v, StarlarkArtifact> {
        self.artifact
    }

    pub fn as_build_artifact(&self) -> &'v BuildArtifact {
        match self.artifact.as_ref().artifact.as_parts().0 {
            BaseArtifactKind::Build(build) => build,
            BaseArtifactKind::Source(_) => unreachable!("checked at construction time"),
        }
    }

    fn get_path(&self) -> ArtifactPath<'v> {
        self.artifact.as_ref().artifact.get_path()
    }
}

impl<'v> StarlarkArtifactLike<'v> for StarlarkOutputArtifact<'v> {
    fn with_filename(
        &self,
        f: &dyn for<'b> Fn(&'b FileName) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        self.get_path().with_filename(f)
    }

    fn is_source(&'v self) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn owner(&'v self) -> buck2_error::Result<Option<BaseDeferredKey>> {
        Ok(self.declared_artifact.as_ref().artifact.owner())
    }

    fn with_short_path(
        &self,
        f: &dyn for<'b> Fn(&'b ForwardRelativePath) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        Ok(self.get_path().with_short_path(f))
    }

    fn fingerprint<'s>(&'s self) -> ArtifactFingerprint<'s>
    where
        'v: 's,
    {
        ArtifactFingerprint::Normal {
            path: self.get_path(),
            associated_artifacts: None,
            is_output: true,
        }
    }
}

impl<'v> StarlarkArtifactLike<'v> for FrozenStarlarkOutputArtifact<'v> {
    fn with_filename(
        &self,
        f: &dyn for<'b> Fn(&'b FileName) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        self.get_path().with_filename(f)
    }

    fn is_source(&'v self) -> buck2_error::Result<bool> {
        Ok(false)
    }

    fn owner(&'v self) -> buck2_error::Result<Option<BaseDeferredKey>> {
        Ok(self.artifact.as_ref().artifact.owner().cloned())
    }

    fn with_short_path(
        &self,
        f: &dyn for<'b> Fn(&'b ForwardRelativePath) -> StringValue<'v>,
    ) -> buck2_error::Result<StringValue<'v>> {
        Ok(self.get_path().with_short_path(f))
    }

    fn fingerprint<'s>(&'s self) -> ArtifactFingerprint<'s>
    where
        'v: 's,
    {
        ArtifactFingerprint::Normal {
            path: self.get_path(),
            associated_artifacts: None,
            is_output: true,
        }
    }
}

starlark::methods_static!(
    OUTPUT_ARTIFACT_METHODS = |b| {
        any_artifact_methods(b);
        output_artifact_methods(b);
    }
);

#[starlark_value(type = "OutputArtifact")]
impl<'v> StarlarkType<'v> for StarlarkOutputArtifactView<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(OUTPUT_ARTIFACT_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        StarlarkArtifactLike::equals(self.as_artifact_like(), other)
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        StarlarkArtifactLike::write_hash(self.as_artifact_like(), hasher)
    }
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for StarlarkOutputArtifact<'v> {
    type Type = StarlarkOutputArtifactView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike<'v>>(self);
    }
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenStarlarkOutputArtifact<'v> {
    type Type = StarlarkOutputArtifactView<'v>;

    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike<'v>>(self);
    }
}

/// An artifact marked as an output of an action.
///
/// When you create custom actions with `ctx.actions.run()`, you need to tell Buck2 which artifacts
/// the action will produce. `OutputArtifact` is how you mark a declared artifact as an output.
///
/// ### Common Usage
///
/// ```python
/// def _impl(ctx):
///     # Declare what file will be produced
///     out = ctx.actions.declare_output("output.txt")
///
///     # Run an action that produces it
///     # The artifact must be marked as output using .as_output()
///     ctx.actions.run(
///         cmd_args(["my_tool", "--output", out.as_output()]),
///         category = "process",
///     )
///
///     return [DefaultInfo(default_output = out)]
/// ```
///
/// ### When is `.as_output()` needed?
///
/// - **Required**: When passing declared artifacts to `ctx.actions.run()` - the action needs to know
///   which artifacts it's responsible for producing
/// - **Not needed**: When using `ctx.actions.write()`, `ctx.actions.copy_file()`, etc. - these
///   methods handle the output declaration automatically
///
/// ### Key Rules
///
/// - Every action must have at least one output
/// - Each declared artifact can only be bound to one action
/// - All declared artifacts must be bound before the rule finishes
/// - If you forget to bind a declared artifact, Buck2 will raise an error
#[starlark_module]
fn output_artifact_methods(builder: &mut MethodsBuilder) {
    /// Returns the input artifact from which this output artifact was constructed
    fn as_input<'v>(
        this: StarlarkOutputArtifactView<'v>,
    ) -> starlark::Result<
        Either<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>, ValueTyped<'v, StarlarkArtifact>>,
    > {
        Ok(match this {
            StarlarkOutputArtifactView::Unfrozen(v) => Either::Left(v.inner()),
            StarlarkOutputArtifactView::Frozen(v) => Either::Right(v.inner()),
        })
    }
}

impl<'v> CommandLineArgLike<'v> for StarlarkOutputArtifact<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkOutputArtifact::starlark_type_repr());
    }

    fn add_to_command_line(
        &self,
        _fmt: &mut CommandLineBuilder<'v, '_>,
    ) -> buck2_error::Result<()> {
        Err(buck2_error::internal_error!(
            "Cannot add an unfrozen output artifact to a command line. \
                 Output artifacts must be declared and bound to an action \
                 before they can be used in command lines"
        ))
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visitor.visit_declared_output(self.declared_artifact.as_ref().output_artifact(), vec![]);
        Ok(())
    }

    fn contains_arg_attr(&self) -> bool {
        false
    }

    fn visit_write_to_file_macros(
        &self,
        _visitor: &mut dyn WriteToFileMacroVisitor,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        Ok(())
    }
}

impl<'v> CommandLineArgLike<'v> for FrozenStarlarkOutputArtifact<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(FrozenStarlarkOutputArtifact::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        // We do not need to use the ArtifactPathMapper here as output artifacts are always
        // resolved to a known path since their content hash is not yet available.
        fmt.push_output_artifact(&self.artifact.as_ref().artifact)?;
        Ok(())
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        visitor.visit_frozen_output(self.artifact.as_ref().artifact(), vec![]);
        Ok(())
    }

    fn contains_arg_attr(&self) -> bool {
        false
    }

    fn visit_write_to_file_macros(
        &self,
        _visitor: &mut dyn WriteToFileMacroVisitor,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        Ok(())
    }
}

/// The result of calling [`Artifact.as_output()`](../Artifact/#artifactas_output).
#[starlark_module]
#[starlark_types(
    StarlarkOutputArtifact<'_> as OutputArtifact
)]
pub(crate) fn register_output_artifact(globals: &mut GlobalsBuilder) {}
