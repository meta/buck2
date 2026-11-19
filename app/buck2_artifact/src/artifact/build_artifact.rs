/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_data::ToProtoMessage;
use buck2_error::internal_error;
use buck2_util::size_assert;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;

use crate::actions::key::ActionKey;
use crate::artifact::output_type::OutputType;

/// An artifact that is built by the build system
#[derive(
    Clone,
    PartialEq,
    Eq,
    Hash,
    Debug,
    Dupe,
    Display,
    Derivative,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
#[display("`{}`, action: {}", path, key)]
pub struct BuildArtifact {
    path: BuildArtifactPath,
    key: ActionKey,
    output_type: OutputType,
}

size_assert::words_of_type!(BuildArtifact, 6);

impl BuildArtifact {
    pub fn new(
        path: BuildArtifactPath,
        key: ActionKey,
        output_type: OutputType,
    ) -> buck2_error::Result<Self> {
        if !key.holder_key().starts_with(path.owner()) {
            return Err(internal_error!(
                "BaseDeferredKey mismatch: in action key: {}, in path: {}",
                key.holder_key(),
                path.owner(),
            ));
        }
        Ok(BuildArtifact {
            path,
            key,
            output_type,
        })
    }

    pub fn get_path(&self) -> &BuildArtifactPath {
        &self.path
    }

    pub fn key(&self) -> &ActionKey {
        &self.key
    }

    pub fn output_type(&self) -> OutputType {
        self.output_type
    }
}

impl ToProtoMessage for BuildArtifact {
    type Message = buck2_data::BuildArtifact;

    fn as_proto(&self) -> Self::Message {
        buck2_data::BuildArtifact {
            key: Some(self.key().as_proto()),
            path: self.get_path().path().to_string(),
        }
    }
}
