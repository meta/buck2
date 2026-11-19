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
use buck2_artifact::artifact::artifact_type::OutputArtifact;

use super::ArtifactTag;
use crate::artifact_groups::ArtifactGroup;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::InputProperties;

/// Wrap an existing CommandLineArtifactVisitor into one that adds an ArtifactTag.
pub struct TaggedVisitor<'a, 'b, 'v> {
    inner: &'b mut dyn CommandLineArtifactVisitor<'v>,
    tag: &'a ArtifactTag,
    inputs_only: bool,
}

impl<'a, 'b, 'v> TaggedVisitor<'a, 'b, 'v> {
    pub fn wrap(
        tag: &'a ArtifactTag,
        inputs_only: bool,
        inner: &'b mut dyn CommandLineArtifactVisitor<'v>,
    ) -> Self {
        Self {
            inner,
            tag,
            inputs_only,
        }
    }
}

impl<'a, 'b, 'v> CommandLineArtifactVisitor<'v> for TaggedVisitor<'a, 'b, 'v> {
    /// Ignore the inner tag, set our own. Nesting input groups generally isn't a great idea, but
    /// we can't statically prevent it.
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        props: InputProperties,
        existing_tags: Vec<&ArtifactTag>,
    ) {
        let mut tags = vec![self.tag];
        tags.extend(existing_tags);
        self.inner.visit_input(input, props, tags)
    }

    fn visit_declared_output(
        &mut self,
        artifact: OutputArtifact<'v>,
        existing_tags: Vec<&ArtifactTag>,
    ) {
        let tags = if self.inputs_only {
            existing_tags
        } else {
            let mut tags = vec![self.tag];
            tags.extend(existing_tags);
            tags
        };
        self.inner.visit_declared_output(artifact, tags)
    }

    fn visit_frozen_output(&mut self, artifact: Artifact, existing_tags: Vec<&ArtifactTag>) {
        let tags = if self.inputs_only {
            existing_tags
        } else {
            let mut tags = vec![self.tag];
            tags.extend(existing_tags);
            tags
        };
        self.inner.visit_frozen_output(artifact, tags)
    }

    fn skip_hidden(&self) -> bool {
        self.inner.skip_hidden()
    }

    fn inputs_for_path_resolution(&self) -> bool {
        self.inner.inputs_for_path_resolution()
    }
}
