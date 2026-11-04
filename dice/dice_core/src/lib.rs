/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The core state of an incremental computation engine: sound reuse of computed values across
//! versions, branches of versions, and concurrent access to any of them.
//!
//! This crate is the data structure specified in `dice/docs/incrementality.md`; section
//! references throughout the code point there. It knows nothing about values, execution or
//! scheduling: keys and revisions are opaque identities, and an execution environment (dice, or
//! anything else) computes values, mints revisions for them, and reports what it computed as
//! [`Cert`]ificates. In return, [`CoreState::lookup`] tells the environment which revision a key
//! has at a version, or that it does not know.
//!
//! The environment plugs in through the [`Env`] trait, which names the data it wants the core to
//! carry: the dependency payload of a certificate, data per claim, and data per assertion.
//! The core never interprets any of them.

pub mod arc;
mod branch;
mod cert;
mod collections;
mod commit;
mod env;
mod fork;
mod history;
mod ids;
mod introspect;
mod invariants;
mod resolve;
mod slot;
mod state;
mod write;

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod oracle;
#[cfg(test)]
mod tests;

pub use crate::cert::Cert;
pub use crate::collections::KeyMap;
pub use crate::collections::KeySet;
pub use crate::commit::Change;
pub use crate::env::Env;
pub use crate::env::Premise;
pub use crate::env::Premises;
pub use crate::history::Entry;
pub use crate::history::History;
pub use crate::ids::BranchId;
pub use crate::ids::EpsilonToken;
pub use crate::ids::Key;
pub use crate::ids::Revision;
pub use crate::ids::Seq;
pub use crate::ids::Version;
pub use crate::introspect::KeyIntrospection;
pub use crate::slot::Claim;
pub use crate::slot::Slot;
pub use crate::slot::Window;
pub use crate::state::CoreState;
pub use crate::state::Lookup;
pub use crate::write::WriteOutcome;
