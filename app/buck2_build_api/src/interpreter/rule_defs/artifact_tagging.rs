/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub(crate) mod artifact_tag;
mod tagged_command_line;
mod tagged_value;
mod tagged_visitor;

pub use artifact_tag::ArtifactTag;
pub use tagged_command_line::StarlarkTaggedCommandLine;
pub(crate) use tagged_command_line::register_tagged_command_line;
pub use tagged_value::StarlarkTaggedValue;
pub(crate) use tagged_value::register_tagged_value;
pub use tagged_visitor::TaggedVisitor;
