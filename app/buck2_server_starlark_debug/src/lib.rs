/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(rustdoc::private_intra_doc_links)]

//! Provides the (daemon-side) support for buck2's starlark debugging.
//!
//! Components of the debugger:
//!
//! [BuckStarlarkDebuggerServer] is the main way that the core of buck2 integrates
//! the starlark debugger. This provides the hooks to wrap a Starlark evaluation and
//! enable the debugger (and handle communication between that starlark evaluation
//! and the debugger server/state)
//!
//! [BuckStarlarkDebuggerHandle] is a "handle" the to the debugger server. One of these
//! will be created for each buck2 command and put in the dice per-transaction data. Code
//! that needs to do starlark evaluation can then use this to setup their Evaluator
//! appropriately (though this is really just an implementation detail hidden in the
//! helper [buck2_interpreter::factory::StarlarkEvaluatorProvider]).
//!
//! [CURRENT_DEBUGGER] holds a global reference for the currently attached debugger and
//! vends out handles (through [create_debugger_handle]).
//!
//! ServerState the internal implementation of the starlark debugger server sends all
//! requests and starlark evaluation events to the single-threaded ServerState.
//!
//! Information about the debugger behavior:
//!
//! The debugger server exposes all current starlark evaluations as separate threads to
//! the DAP client. Then, one of it's main responsibility is as a mux/demux for requests/events/etc
//! flowing between the DAP client and the starlark evaluation.
//!
//! The server also tracks the state of all evaluations so that it can send regularly
//! debugger snapshots to all current buck commands. Those commands in turn use those snapshots
//! to render a debugger superconsole component so that users aren't left confused about
//! why their commands are seemingly hanging.
//!
//! The server handles a variety of other small adapter functionality. For example,
//! the DAP client may send source file references as absolute paths, but our starlark
//! evaluators will only understand them as the project-relative paths that buck
//! uses and so the server must convert them from/to the client/starlark.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use buck2_core::fs::project::ProjectRoot;
use buck2_events::dispatch::EventDispatcher;
use buck2_interpreter::starlark_debug::StarlarkDebugController;
use buck2_interpreter::starlark_debug::StarlarkDebuggerHandle;
use derive_more::Display;
use dupe::Dupe;
use tokio::sync::mpsc;

use crate::error::StarlarkDebuggerError;
use crate::run::ToClientMessage;
use crate::server::BuckStarlarkDebuggerServer;

mod controller;
mod dap_api;
mod error;
pub mod run;
mod server;
mod variable_known_paths;

/// A handle to the debugger server.
#[derive(Debug, Clone, Dupe)]
pub struct BuckStarlarkDebuggerHandle(Arc<HandleData>);

#[derive(Debug)]
pub struct HandleData {
    id: HandleId,
    server: Arc<BuckStarlarkDebuggerServer>,
}

impl Drop for HandleData {
    fn drop(&mut self) {
        self.server.drop_handle(self.id)
    }
}

#[async_trait]
impl StarlarkDebuggerHandle for BuckStarlarkDebuggerHandle {
    async fn start_eval(
        &self,
        description: &str,
    ) -> buck2_error::Result<Box<dyn StarlarkDebugController>> {
        self.0.server.start_eval(self, description).await
    }
}

#[derive(Debug, Clone, Dupe, Copy, Eq, PartialEq, Hash)]
pub(crate) struct HandleId(u32);

#[derive(Display, Debug, Clone, Dupe, Copy, Eq, PartialEq, Hash)]
pub(crate) struct HookId(u32);

/// We allow only a single debugger to be attached at a time. While it's attached, this will hold the server instance.
static CURRENT_DEBUGGER: Mutex<Option<Arc<BuckStarlarkDebuggerServer>>> = Mutex::new(None);

/// Used by each command to get a handle to the current debugger. The debugger server will capture the
/// event dispatcher to send back debugger state snapshots (which indicate that the debugger is attached
/// and which, if any, threads are paused) while the command is running.
pub fn create_debugger_handle(events: EventDispatcher) -> Option<BuckStarlarkDebuggerHandle> {
    CURRENT_DEBUGGER
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|v| v.new_handle(events))
}

/// Manages setting/unsetting the CURRENT_DEBUGGER while the starlark debug-attach command is running.
struct ServerConnection(Arc<BuckStarlarkDebuggerServer>);

impl ServerConnection {
    fn new(
        to_client_send: mpsc::UnboundedSender<ToClientMessage>,
        project_root: ProjectRoot,
    ) -> buck2_error::Result<Self> {
        let mut locked = CURRENT_DEBUGGER.lock().unwrap();
        if locked.is_some() {
            return Err(StarlarkDebuggerError::DebuggerAlreadyAttached.into());
        }

        let server = Arc::new(BuckStarlarkDebuggerServer::new(
            to_client_send,
            project_root,
        ));
        *locked = Some(server.dupe());
        Ok(Self(server))
    }
}

impl Drop for ServerConnection {
    fn drop(&mut self) {
        *CURRENT_DEBUGGER.lock().unwrap() = None;
        let _ignored = self.0.detach();
    }
}
