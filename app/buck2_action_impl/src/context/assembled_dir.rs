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
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use derive_more::Display;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::starlark_value;

/// One entry of `ctx.actions.assembled_dir`: a bound input artifact plus how
/// it is materialized inside the assembled directory (real bytes vs symlink).
/// Constructed with `assembled_dir.copy(artifact)` /
/// `assembled_dir.symlink(artifact)`; opaque to Starlark otherwise.
#[derive(
    Debug,
    Clone,
    Trace,
    Freeze,
    Display,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[derive(NoSerialize)] // TODO make artifacts serializable
#[display("AssembledDirEntry(copy = {}, {})", copy, artifact)]
pub struct StarlarkAssembledDirEntry<'v> {
    /// `true`: lay the artifact out as real bytes; `false`: symlink to it.
    pub(crate) copy: bool,
    /// The input artifact; validated as artifact-like at construction.
    pub(crate) artifact: Value<'v>,
}

starlark_complex_value!(pub StarlarkAssembledDirEntry);

#[starlark_value(type = "AssembledDirEntry")]
impl<'v> StarlarkValue<'v> for StarlarkAssembledDirEntry<'v> {}

/// Entry constructors for `ctx.actions.assembled_dir`.
#[starlark_module]
fn assembled_dir_members(globals: &mut GlobalsBuilder) {
    /// An entry materialized as the artifact's real bytes. Use for inputs
    /// that must be physically present in the assembled directory -- e.g. an
    /// executable whose `$ORIGIN`-relative RPATH or `current_exe`-relative
    /// lookup must resolve inside it (a symlinked executable resolves those
    /// against its realpath, outside the directory).
    fn copy<'v>(
        #[starlark(require = pos)] artifact: ValueOf<'v, StarlarkArtifactView<'v>>,
    ) -> starlark::Result<StarlarkAssembledDirEntry<'v>> {
        Ok(StarlarkAssembledDirEntry {
            copy: true,
            artifact: artifact.to_value(),
        })
    }

    /// An entry materialized as a symlink pointing at the artifact. Use for
    /// inputs that are pure content -- especially large ones already living
    /// at an immutable (content-based) path -- to avoid duplicating bytes.
    fn symlink<'v>(
        #[starlark(require = pos)] artifact: ValueOf<'v, StarlarkArtifactView<'v>>,
    ) -> starlark::Result<StarlarkAssembledDirEntry<'v>> {
        Ok(StarlarkAssembledDirEntry {
            copy: false,
            artifact: artifact.to_value(),
        })
    }
}

#[starlark_module]
#[starlark_types(StarlarkAssembledDirEntry<'_> as AssembledDirEntry)]
fn register_assembled_dir_entry_type(globals: &mut GlobalsBuilder) {}

pub(crate) fn register_assembled_dir(globals: &mut GlobalsBuilder) {
    register_assembled_dir_entry_type(globals);
    globals.namespace("assembled_dir", assembled_dir_members);
}
