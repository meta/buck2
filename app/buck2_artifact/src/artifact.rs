/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! An 'Artifact' represents a File to be used as part of the build. It can be either a file in the
//! source tree, or a file to be generated as part of the build.
//!
//! The existence of an Artifact does not mean the actual file exists. Artifacts needs to be
//! 'made available' before its existence on the filesystem is guaranteed.
//!
//! An 'Artifact' is first "declared" by rule implementation as a 'DeclaredArtifact'. The artifact
//! will need to be "bound" to an 'Action' through being used as an 'OutputArtifact'. Once bound,
//! it becomes a 'BuildArtifact' that can be available.

pub mod artifact_dump;
pub mod artifact_dyn;
pub mod artifact_path;
pub mod artifact_type;
pub mod build_artifact;
pub mod output_type;
pub mod source_artifact;
