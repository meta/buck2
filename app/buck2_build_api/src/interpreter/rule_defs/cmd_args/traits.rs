/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::DeclaredArtifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_interpreter::types::cell_root::CellRoot;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_interpreter::types::project_root::StarlarkProjectRoot;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use starlark::any::ProvidesStaticType;
use starlark::typing::Ty;
use starlark::values::StarlarkValue;
use starlark::values::string::StarlarkStr;

use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::ArtifactGroupValues;
use crate::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use crate::interpreter::rule_defs::cmd_args::builder::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;
use crate::interpreter::rule_defs::resolved_macro::ResolvedMacro;

/// How an input reaches the action. Leaves report the default; each enclosing
/// `cmd_args` scope rewrites it on the way up to the action's visitor.
///
/// A visitor that branches on the properties must destructure it without `..` and match
/// `requires` exhaustively, so that a new property or need is a compile error at every
/// site that decides on it. Wrappers that only rewrite one property forward the rest
/// with `..props`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputProperties {
    /// Reached through some `cmd_args`'s `hidden` list; nothing renders it.
    pub hidden: bool,
    pub requires: Requires,
}

/// What the command requires of an input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Requires {
    /// The contents: a regular input, built and materialized.
    #[default]
    Contents,
    /// Only the path, as under `ignore_artifacts`: built when the path embeds the
    /// content hash and is rendered, never materialized. Hidden, nothing renders it,
    /// so it is not an input at all.
    Path,
    /// Only the content hash: built, folded into the run action's salt, never
    /// materialized.
    Digest,
}

pub trait CommandLineArtifactVisitor<'v> {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        props: InputProperties,
        tags: Vec<&ArtifactTag>,
    );

    fn visit_declared_output(&mut self, artifact: OutputArtifact<'v>, tags: Vec<&ArtifactTag>);

    fn visit_frozen_output(&mut self, artifact: Artifact, tags: Vec<&ArtifactTag>);

    /// Those two functions can be used to keep track of recursion when visiting artifacts.
    fn push_frame(&mut self) -> buck2_error::Result<()> {
        Ok(())
    }

    fn pop_frame(&mut self) {}

    fn visit_declared_artifact(
        &mut self,
        declared_artifact: DeclaredArtifact<'v>,
        props: InputProperties,
        tags: Vec<&ArtifactTag>,
    ) -> buck2_error::Result<()> {
        self.visit_input(
            ArtifactGroup::Artifact(declared_artifact.ensure_bound()?.into_artifact()),
            props,
            tags,
        );
        Ok(())
    }

    fn skip_hidden(&self) -> bool {
        self.inputs_for_path_resolution()
    }

    /// Whether to collect inputs for rendering paths rather than executing the command.
    /// These visitors skip hidden arguments by default.
    fn inputs_for_path_resolution(&self) -> bool {
        false
    }
}

/// A CommandLineArtifactVisitor that gathers inputs and outputs.
pub struct SimpleCommandLineArtifactVisitor<'v> {
    pub inputs: BuckMutInsertionSet<ArtifactGroup>,
    pub declared_outputs: BuckMutInsertionSet<OutputArtifact<'v>>,
    pub frozen_outputs: BuckMutInsertionSet<Artifact>,
}

impl SimpleCommandLineArtifactVisitor<'_> {
    pub fn new() -> Self {
        Self {
            inputs: BuckMutInsertionSet::default(),
            declared_outputs: BuckMutInsertionSet::default(),
            frozen_outputs: BuckMutInsertionSet::default(),
        }
    }

    pub fn inputs(&self) -> impl Iterator<Item = &ArtifactGroup> {
        self.inputs.iter()
    }
}

impl<'v> CommandLineArtifactVisitor<'v> for SimpleCommandLineArtifactVisitor<'v> {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        props: InputProperties,
        _tags: Vec<&ArtifactTag>,
    ) {
        let InputProperties { hidden, requires } = props;
        match requires {
            Requires::Contents => {}
            Requires::Path if hidden => return,
            Requires::Path => {}
            Requires::Digest => {}
        }
        self.inputs.insert(input);
    }

    fn visit_declared_output(&mut self, artifact: OutputArtifact<'v>, _tags: Vec<&ArtifactTag>) {
        self.declared_outputs.insert(artifact);
    }

    fn visit_frozen_output(&mut self, artifact: Artifact, _tags: Vec<&ArtifactTag>) {
        self.frozen_outputs.insert(artifact);
    }
}

pub trait WriteToFileMacroVisitor {
    fn visit_write_to_file_macro(
        &mut self,
        m: &ResolvedMacro,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()>;

    fn set_current_relative_to_path(&mut self, p: ProjectRelativePathBuf);

    fn fs(&self) -> Option<&ExecutorFs<'_>>;
}

/// Used to provide a mapping from artifacts to the content-based hash that should be used when
/// resolving their path.
///
/// In general, for artifacts that have been built (e.g. inputs to an action), we use a hash based
/// on the content of the artifact. For some other situations where we do not yet have the content
/// hash available, this mapper may choose to resolve artifacts to a different "hash". For example,
/// prior to being built, output artifacts always resolve to a known constant value.  
///
/// Only artifacts that are specified to use content-based paths actually use the value that is
/// returned here. Any other artifacts will continue to resolve their path using the configuration
/// hash.
pub trait ArtifactPathMapper {
    fn get(&self, artifact: &Artifact) -> Option<&ContentBasedPathHash>;
}

impl ArtifactPathMapper for BuckMutMap<&Artifact, ContentBasedPathHash> {
    fn get(&self, artifact: &Artifact) -> Option<&ContentBasedPathHash> {
        self.get(artifact)
    }
}

pub struct ArtifactPathMapperImpl<'a> {
    pub map: BuckMutMap<&'a Artifact, ContentBasedPathHash>,
}

impl<'a> From<&'a Vec<(ArtifactGroup, ArtifactGroupValues)>> for ArtifactPathMapperImpl<'a> {
    fn from(ensured_inputs: &'a Vec<(ArtifactGroup, ArtifactGroupValues)>) -> Self {
        Self {
            map: ensured_inputs
                .iter()
                .flat_map(|(_, v)| v.iter())
                .map(|(a, v)| (a, v.content_based_path_hash()))
                .collect(),
        }
    }
}

impl ArtifactPathMapper for ArtifactPathMapperImpl<'_> {
    fn get(&self, artifact: &Artifact) -> Option<&ContentBasedPathHash> {
        self.map.get(artifact)
    }
}

/// Implemented by anything that can show up in a command line
pub trait CommandLineArgLike<'v> {
    /// Call `command_line_arg_like_impl!` to register the type with the interpreter typechecker.
    fn register_me(&self);

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()>;

    fn visit_artifacts(
        &self,
        _visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Allows to query if object contains a value resolved from `attrs.arg()`
    fn contains_arg_attr(&self) -> bool;

    fn visit_write_to_file_macros(
        &self,
        visitor: &mut dyn WriteToFileMacroVisitor,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()>;
}

unsafe impl<'v> ProvidesStaticType<'v> for &'v dyn CommandLineArgLike<'v> {
    type StaticType = &'static dyn CommandLineArgLike<'static>;
}

impl<'v> CommandLineArgLike<'v> for &str {
    fn register_me(&self) {
        command_line_arg_like_impl!(Ty::string());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_str(self);
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

impl<'v> CommandLineArgLike<'v> for StarlarkStr {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkStr::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_str(self.as_str());
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

impl<'v> CommandLineArgLike<'v> for StarlarkTargetLabel {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkTargetLabel::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_string(self.to_string());
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

impl<'v> CommandLineArgLike<'v> for StarlarkConfiguredProvidersLabel {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkConfiguredProvidersLabel::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_string(self.to_string());
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

impl<'v> CommandLineArgLike<'v> for CellRoot {
    fn register_me(&self) {
        command_line_arg_like_impl!(CellRoot::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_cell_path(self.cell_path())?;
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

impl<'v> CommandLineArgLike<'v> for StarlarkProjectRoot {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkProjectRoot::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        fmt.push_project_path(ProjectRelativePath::empty().to_owned())?;
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
