/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_core::configuration::transition::id::TransitionId;
use starlark::starlark_predicate_type;

/// Implemented by starlark transition objects.
pub trait TransitionValue {
    fn transition_id(&self) -> buck2_error::Result<Arc<TransitionId>>;
}

// There is no global of this name, and so no `.bzl` file can write the type: transition objects
// are meant to go away, and a name that files write would have to outlive them. Signatures and
// error messages print the name all the same.
starlark_predicate_type! {
    /// A transition: what `transition(..)` returns, a change of the configuration that a
    /// dependency or a target is built in.
    pub StarlarkTransition<'v>;
    name "Transition";
    get &'v dyn TransitionValue;
}
