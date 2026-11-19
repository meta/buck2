/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_events::dispatch::with_dispatcher_opt_async;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_hash::BuckMutMap;
use buck2_hash::IntentionallyStdHashMap;
use buck2_interpreter::starlark_debug::StarlarkDebugController;
use debugserver_types as dap;
use dupe::Dupe;
use futures::StreamExt;
use itertools::Itertools;
use starlark::debug::DapAdapter;
use starlark::debug::DapAdapterClient;
use starlark::debug::DapAdapterEvalHook;
use starlark::debug::ResolvedBreakpoints;
use starlark::debug::StepKind;
use starlark::debug::VariablePath;
use starlark::debug::prepare_dap_adapter;
use starlark::debug::resolve_breakpoints;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;
use starlark::syntax::DialectTypes;
use tokio::select;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::debug;

use crate::BuckStarlarkDebuggerHandle;
use crate::HandleData;
use crate::HandleId;
use crate::HookId;
use crate::controller::BuckStarlarkDebugController;
use crate::dap_api::ContinueArguments;
use crate::dap_api::DebugServer;
use crate::dap_api::dap_event;
use crate::dap_api::dispatch;
use crate::dap_api::err_response;
use crate::error::StarlarkDebuggerError;
use crate::run::ToClientMessage;
use crate::variable_known_paths::VariablesKnownPaths;

/// The DAP capabilities that our debugserver supports.
///
/// See https://microsoft.github.io/debug-adapter-protocol/specification#Types_Capabilities
fn capabilities() -> serde_json::Value {
    // debugserver_types is out of date and missing fields on Capabilities and so we just construct
    // a little json map explicitly ourselves.
    serde_json::json!({
        "supportsConfigurationDoneRequest": true,
        "supportsEvaluateForHovers": true,
        "supportsSetVariable": true,
        "supportsStepInTargetsRequest": true,
        "supportsConditionalBreakpoints": true,
        // note that some capabilities have the word "support" and some "supports" this seems to be according to the spec
        "supportTerminateDebuggee": false,
        "supportSuspendDebuggee": false,
        // This is different from starlark's `dap_capabilities`. The buck starlark debugger treats
        // each ongoing starlark Evaluation as a separate thread and handles requests appropriately.
        "supportsSingleThreadExecutionRequests": true,

    })
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum DebuggerError {
    #[error("SetBreakpointsArguments invalid: {0:?}")]
    InvalidSetBreakpoints(dap::SetBreakpointsArguments),
}

/// The buck starlark debugger server. Most of the work is managed by the single-threaded server state.
///
/// There will be several references to the BuckStarlarkDebuggerServer instance and it will forward messages
/// along to the state.
#[derive(Debug)]
pub(crate) struct BuckStarlarkDebuggerServer {
    to_state: mpsc::UnboundedSender<ServerMessage>,
    next_handle_id: AtomicU32,
    /// When debugging a starlark evaluation, we wrap it in tokio::task::block_in_place (so that when it is paused
    /// it doesn't block a tokio worker thread), but we still want to ensure that we aren't over-saturating the
    /// local resources so we use this semaphore to limit how many evaluation can currently run.
    /// TODO(cjhopman): It probably actually makes sense for this to be a more general mechanism in buck so that
    /// long-running things aren't holding tokio workers busy without yielding. That'd then also better integrate
    /// with user-requested resource limits (e.g. `-j 4`).
    eval_semaphore: Arc<Semaphore>,
}

impl BuckStarlarkDebuggerServer {
    pub(crate) fn new(
        to_client: mpsc::UnboundedSender<ToClientMessage>,
        project_root: ProjectRoot,
    ) -> Self {
        let (to_state, state_recv) = mpsc::unbounded_channel();
        let dispatcher = get_dispatcher_opt();
        tokio::task::spawn(with_dispatcher_opt_async(dispatcher, async move {
            let mut server = ServerState::new(to_client, project_root);
            let res = server.run(state_recv).await;
            // We always send the ::Shutdown message when the state thread finishes. It may be normal
            // shutdown (on detach()) or indicate an internal state error or that the client has already
            // exited (and dropped its side of the channel).
            let _ignored = server.to_client.send(ToClientMessage::Shutdown(res));
        }));
        // TODO(cjhopman): figure out a better value for the limit on evaluations. This should probably be
        // passed in as we should at the least respect any `-j` flag.
        Self {
            to_state,
            eval_semaphore: Arc::new(Semaphore::new(buck2_util::threads::available_parallelism())),
            next_handle_id: AtomicU32::new(0),
        }
    }

    /// Send a message to the ServerState.
    ///
    /// Errors from the send are ignored, callers should assume that the state could encounter an
    /// error and shutdown early (but will not otherwise lose messages).
    ///
    /// Even if we were to return an error from this, that could only be used as an indication that the
    /// state thread has finished but the `Ok()` would not necessarily mean that the state thread would
    /// ever see the sent message.
    fn maybe_to_state(&self, msg: ServerMessage) {
        let _ignored = self.to_state.send(msg);
    }

    pub(crate) fn new_handle(
        self: &Arc<Self>,
        events: EventDispatcher,
    ) -> Option<BuckStarlarkDebuggerHandle> {
        let handle_id = HandleId(self.next_handle_id.fetch_add(1, Ordering::Relaxed));
        self.maybe_to_state(ServerMessage::NewHandle {
            id: handle_id,
            events,
        });
        Some(BuckStarlarkDebuggerHandle(Arc::new(HandleData {
            id: handle_id,
            server: self.clone(),
        })))
    }

    /// Acquires a StarlarkDebugController for starlark evaluation.
    ///
    /// The debugger server limits how many concurrent evaluations there can be. This is important as
    /// `with_starlark_eval_provider` will send starlark evaluations to tokio blocking threads when a
    /// debugger is attached and so we need to otherwise limit the concurrent ones.
    pub(crate) async fn start_eval(
        self: &Arc<Self>,
        handle: &BuckStarlarkDebuggerHandle,
        description: &str,
    ) -> buck2_error::Result<Box<dyn StarlarkDebugController>> {
        debug!("starting debug-hooked eval {}", description);
        let permit = self.eval_semaphore.dupe().acquire_owned().await?;
        let (send, recv) = oneshot::channel();

        // Here we actually need a response from the state. We still ignore any send
        // errors, but if the state thread has hit some internal error and is shutting down
        // we'll notice when we poll the recv channel.
        self.maybe_to_state(ServerMessage::NewHook {
            handle: handle.dupe(),
            description: description.to_owned(),
            response_channel: send,
        });

        let (hook_id, eval_wrapper) = match recv.await {
            Ok(v) => v,
            Err(..) => {
                // This indicates the state thread is shutting down (or hit an internal error).
                // That could be due to the debugger detaching from buck2. This does not indicate
                // an error for other on-going buck commands, and so we'll allow starlark execution
                // to continue as normal. In this case, the hook_id doesn't matter.
                (HookId(u32::MAX), None)
            }
        };
        Ok(Box::new(BuckStarlarkDebugController::new(
            eval_wrapper,
            hook_id,
            description,
            self,
            permit,
        )))
    }

    /// Indicates that a hook has been dropped and so an evaluation has finished.
    pub(crate) fn drop_hook(&self, hook_id: HookId) {
        self.maybe_to_state(ServerMessage::DropHook { id: hook_id });
    }

    /// Indicates that a handle has been dropped and so a command (and its associated
    /// dice computation) has finished.
    pub(crate) fn drop_handle(&self, handle_id: HandleId) {
        self.maybe_to_state(ServerMessage::DropHandle { id: handle_id });
    }

    /// Called when a starlark evaluation is paused (e.g. at a breakpoint).
    pub(crate) fn event_stopped(&self, hook_id: HookId) {
        self.maybe_to_state(ServerMessage::EvalStopped { hook_id });
    }

    /// Called to forward along requests from the DAP client.
    pub(crate) fn send_request(&self, req: dap::Request) -> buck2_error::Result<()> {
        // If the state encountered an error or is shutting down, it may never see this
        // request. But that's okay since in that case it will send back the Shutdown message
        // and that'll make its way back to the client.
        self.maybe_to_state(ServerMessage::DapRequest { req });
        Ok(())
    }

    /// Called when the DAP client has disconnected.
    pub(crate) fn detach(&self) -> buck2_error::Result<()> {
        self.maybe_to_state(ServerMessage::Detach);
        Ok(())
    }
}

/// Messages to the debugger server state
enum ServerMessage {
    NewHook {
        handle: BuckStarlarkDebuggerHandle,
        description: String,
        response_channel: oneshot::Sender<(HookId, Option<Box<dyn DapAdapterEvalHook>>)>,
    },
    NewHandle {
        id: HandleId,
        events: EventDispatcher,
    },
    DropHook {
        id: HookId,
    },
    DropHandle {
        id: HandleId,
    },
    DapRequest {
        req: dap::Request,
    },
    EvalStopped {
        hook_id: HookId,
    },
    Detach,
}

/// The ServerState is the main thing implementing the debug adapter protocol.
///
/// It runs on a single thread to more easily handle the concurrent requests and events
/// from the DAP client and buck's multithreaded starlark evaluation.
#[derive(Debug)]
struct ServerState {
    /// Sends messages back to the DAP client. Errors on this channel indicate the
    /// `starlark debug-attach` command has ended and so we treat them as state
    /// errors to stop the state thread (similar to a detach() call).
    to_client: mpsc::UnboundedSender<ToClientMessage>,

    /// The currently set breakpoints. New hooks will be initialized with these.
    set_breakpoints: BuckMutMap<String, ResolvedBreakpoints>,

    /// The project root is used to get the current source code to resolve breakpoints.
    project_root: ProjectRoot,

    /// Currently executing buck commands, this is primarily used to send debugger snapshots.
    current_commands: BuckMutMap<HandleId, CommandState>,

    /// Current starlark evaluation hooks.
    current_hooks: BuckMutMap<HookId, HookState>,

    /// HookIds are simply incrementing.
    next_hook_id: HookId,

    /// We keep a pool of "pseudo" thread ids to appear to use only as many threads as we ever
    /// have concurrent starlark evaluations. Some DAP clients may provide slightly nicer ui
    /// when we do this instead of providing the real thread ids (which might range over some
    /// 100s of ids)
    free_pseudo_threads: BTreeSet<u32>,
    next_pseudo_thread: u32,

    /// This data structure keeps track of destructured local variables obtained by debugger at breakpoint
    /// this is required to satify incremental nature of VariablesRequeste
    /// variables are lazily fetched from starlark evaluator and cached by thread id
    variables_by_thread: BuckMutMap<u32, VariablesKnownPaths>,
}

/// This type is using bitmasking to pack "frame_id, thread_id, variable_id" into an integer value
/// Since this is a JSON serialized number it's safer to only rely on 53 bits
/// thread_id is u32 but we're putting a limit of 20 bits on it for now to simplify DAP implementation
/// frame_id supports up to 127 frames (0 = top frame).
///
/// Bit 47 (`DAP_SENTINEL_GUARD`) is always set so the encoded value is never 0,
/// which DAP reserves as the `variables_reference` "no children" sentinel.
///
/// Layout (48 bits used):
///   [DAP sentinel guard: 1 bit (47)]
///   [frame_id: 7 bits (40-46)]
///   [thread_id: 20 bits (20-39)]
///   [variable_id: 20 bits (0-19)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct VariableId(i64);

impl VariableId {
    const MASK_53_BITS: i64 = (1 << 53) - 1;
    const DAP_SENTINEL_GUARD: i64 = 1 << 47;
    const MAX_THREAD_ID: u32 = 0xFFFFF; // 20 bits
    const MAX_VARIABLE_ID: u32 = 0xFFFFF; // 20 bits
    const MAX_FRAME_ID: u32 = 0x7F; // 7 bits

    pub fn new(frame_id: u32, thread_id: u32, variable_id: u32) -> buck2_error::Result<Self> {
        if thread_id > Self::MAX_THREAD_ID {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::StarlarkServer,
                "Thread ID exceeds 20-bit limit: max is {}, received {}",
                Self::MAX_THREAD_ID,
                thread_id
            ));
        }
        if variable_id > Self::MAX_VARIABLE_ID {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::StarlarkServer,
                "Variable ID exceeds 20-bit limit: max is {}, received {}",
                Self::MAX_VARIABLE_ID,
                variable_id
            ));
        }
        if frame_id > Self::MAX_FRAME_ID {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::StarlarkServer,
                "Frame ID exceeds 7-bit limit: max is {}, received {}",
                Self::MAX_FRAME_ID,
                frame_id
            ));
        }
        let frame_id_part = (frame_id as i64) << 40;
        let thread_id_part = (thread_id as i64) << 20;
        Ok(Self(
            Self::DAP_SENTINEL_GUARD | frame_id_part | thread_id_part | variable_id as i64,
        ))
    }

    pub fn frame_id(self) -> u32 {
        ((self.0 >> 40) & 0x7F) as u32
    }

    pub fn thread_id(self) -> u32 {
        ((self.0 >> 20) & 0xFFFFF) as u32
    }

    pub fn variable_id(self) -> u32 {
        (self.0 & 0xFFFFF) as u32
    }

    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for VariableId {
    type Error = buck2_error::Error;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value & VariableId::MASK_53_BITS == value {
            Ok(Self(value))
        } else {
            Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Input,
                "value exceeds 53-bit limit. value: {}",
                value
            ))
        }
    }
}

impl From<VariableId> for i64 {
    fn from(value: VariableId) -> Self {
        value.0
    }
}

impl DebugServer for ServerState {
    fn initialize(
        &mut self,
        _x: dap::InitializeRequestArguments,
    ) -> buck2_error::Result<Option<serde_json::Value>> {
        Ok(Some(capabilities()))
    }

    fn set_breakpoints(
        &mut self,
        mut x: dap::SetBreakpointsArguments,
    ) -> buck2_error::Result<dap::SetBreakpointsResponseBody> {
        // buck will use the project-relative paths when parsing asts with the starlark interpreter. We need to match that.
        let source = x
            .source
            .path
            .take()
            .ok_or_else(|| DebuggerError::InvalidSetBreakpoints(x.clone()))?;
        // vscode sends absolute paths, so need to relativize them
        let project_relative = self.project_root.relativize(AbsNormPath::new(&source)?)?;
        let source = project_relative.to_string();
        x.source.path = Some(source.clone());

        // We currently just resolve new breakpoints against the current state of the file. This isn't quite correct, but oh well.
        let resolved = resolve_breakpoints(&x, &self.get_ast(&project_relative)?)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        for hook_state in self.current_hooks.values() {
            hook_state
                .adapter
                .set_breakpoints(&source, &resolved)
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        }
        let response = resolved.to_response();
        self.set_breakpoints.insert(source, resolved);
        Ok(response)
    }

    fn set_exception_breakpoints(
        &mut self,
        _x: dap::SetExceptionBreakpointsArguments,
    ) -> buck2_error::Result<()> {
        // TODO(cjhopman): This may not make sense in starlark and could be a more informative error. Or possibly we
        // could use it to break on `fail()`.
        Err(StarlarkDebuggerError::Unimplemented.into())
    }

    fn attach(&mut self, _x: dap::AttachRequestArguments) -> buck2_error::Result<()> {
        Ok(())
    }

    fn threads(&mut self) -> buck2_error::Result<dap::ThreadsResponseBody> {
        let mut threads = Vec::with_capacity(self.current_hooks.len());
        for hook_state in self
            .current_hooks
            .values()
            .sorted_unstable_by_key(|v| v.pseudo_thread_id)
        {
            threads.push(dap::Thread {
                id: hook_state.pseudo_thread_id as i64,
                name: hook_state.pseudo_thread_name.clone(),
            });
        }
        Ok(dap::ThreadsResponseBody { threads })
    }

    fn configuration_done(&mut self) -> buck2_error::Result<()> {
        Ok(())
    }

    fn stack_trace(
        &mut self,
        x: dap::StackTraceArguments,
    ) -> buck2_error::Result<dap::StackTraceResponseBody> {
        let hook = self.find_hook_by_pseudo_thread(x.thread_id)?;
        let mut trace_response = hook
            .adapter
            .stack_trace(x)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        for frame in &mut trace_response.stack_frames {
            // rewrite the sources to be absolute (like vscode sent us)
            if let Some(source) = &mut frame.source {
                if let Some(path) = &mut source.path {
                    let abs_path = self.project_root.resolve(ProjectRelativePath::new(path)?);
                    *path = abs_path.to_string();
                }
            }

            // rewrite the frame ids to be {tid}{frame_id}. starlark doesn't write frame ids > 10000, our thread ids should be <1000.
            frame.id |= (hook.pseudo_thread_id as i64) << 16;
        }
        Ok(trace_response)
    }

    fn scopes(&mut self, x: dap::ScopesArguments) -> buck2_error::Result<dap::ScopesResponseBody> {
        let thread_id = x.frame_id >> 16;
        let frame_id = (x.frame_id & 0xFFFF) as u32;

        let hook = self.find_hook_by_pseudo_thread(thread_id)?;
        let scopes_info = hook
            .adapter
            .scopes(frame_id as usize)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        Ok(dap::ScopesResponseBody {
            scopes: vec![dap::Scope {
                name: "Locals".to_owned(),
                named_variables: Some(scopes_info.num_locals as i64),
                variables_reference: VariableId::new(frame_id, thread_id as u32, 0)?.as_i64(),
                expensive: false,
                column: None,
                end_column: None,
                end_line: None,
                indexed_variables: None,
                line: None,
                source: None,
            }],
        })
    }

    fn variables(
        &mut self,
        x: dap::VariablesArguments,
    ) -> buck2_error::Result<dap::VariablesResponseBody> {
        let encoded_variable_id = VariableId::try_from(x.variables_reference)?;
        let thread_id = encoded_variable_id.thread_id();
        let frame_id = encoded_variable_id.frame_id();

        let mut result = Vec::new();

        if encoded_variable_id.variable_id() == 0 {
            let hook = self.find_hook_by_pseudo_thread(thread_id.into())?;
            let vars_info = hook
                .adapter
                .variables(frame_id as usize)
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
            let known_variables = self.variables_by_thread.entry(thread_id).or_default();

            for v in vars_info.locals {
                let has_children = v.has_children;
                let var_path = VariablePath::new_local(v.name.to_string());
                let mut dap_message = v.to_dap();
                if has_children {
                    let var_id = known_variables.insert(var_path);
                    dap_message.variables_reference =
                        VariableId::new(frame_id, thread_id, var_id)?.into();
                }
                result.push(dap_message);
            }
        } else {
            let path = self
                .variables_by_thread
                .get(&thread_id)
                .and_then(|x| x.get(encoded_variable_id.variable_id()))
                .map(ToOwned::to_owned);

            if let Some(path) = path {
                let hook = self.find_hook_by_pseudo_thread(thread_id.into())?;
                let inspect_result = hook
                    .adapter
                    .inspect_variable(frame_id as usize, path.to_owned())
                    .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
                let current_frame_vars =
                    self.variables_by_thread
                        .get_mut(&thread_id)
                        .ok_or_else(|| {
                            internal_error!("variables cache must exist in this codepath")
                        })?;

                for child in inspect_result.sub_values {
                    let child_path = path.make_child(child.name.clone());
                    let has_children = child.has_children;
                    let mut dap_result = child.to_dap();
                    if has_children {
                        let reference_id = VariableId::new(
                            frame_id,
                            thread_id,
                            current_frame_vars.insert(child_path),
                        )?;
                        dap_result.variables_reference = reference_id.into();
                    }
                    result.push(dap_result);
                }
            }
        };

        Ok(dap::VariablesResponseBody { variables: result })
    }

    fn source(&mut self, _x: dap::SourceArguments) -> buck2_error::Result<dap::SourceResponseBody> {
        Err(StarlarkDebuggerError::Unimplemented.into())
    }

    fn continue_(
        &mut self,
        x: ContinueArguments,
    ) -> buck2_error::Result<dap::ContinueResponseBody> {
        let hook = self.find_hook_by_pseudo_thread_mut(x.thread_id)?;
        hook.adapter
            .continue_()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        hook.stopped_at = None;

        Ok(dap::ContinueResponseBody {
            all_threads_continued: Some(false),
        })
    }

    fn next(&mut self, x: dap::NextArguments) -> buck2_error::Result<()> {
        let hook = self.find_hook_by_pseudo_thread_mut(x.thread_id)?;
        hook.adapter
            .step(StepKind::Over)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        hook.stopped_at = None;
        Ok(())
    }

    fn step_in(&mut self, x: dap::StepInArguments) -> buck2_error::Result<()> {
        let hook = self.find_hook_by_pseudo_thread_mut(x.thread_id)?;
        hook.adapter
            .step(StepKind::Into)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        hook.stopped_at = None;
        Ok(())
    }

    fn step_out(&mut self, x: dap::StepOutArguments) -> buck2_error::Result<()> {
        let hook = self.find_hook_by_pseudo_thread_mut(x.thread_id)?;
        hook.adapter
            .step(StepKind::Out)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        hook.stopped_at = None;
        Ok(())
    }

    fn evaluate(
        &mut self,
        x: dap::EvaluateArguments,
    ) -> buck2_error::Result<dap::EvaluateResponseBody> {
        let frame_id = match x.frame_id {
            Some(v) => v,
            None => {
                return Err(StarlarkDebuggerError::Unimplemented.into());
            }
        };
        let thread_id = frame_id >> 16;
        let frame_id = frame_id & 0xFFFF;
        if frame_id != 0 {
            return Err(StarlarkDebuggerError::Unimplemented.into());
        }

        let hook = self.find_hook_by_pseudo_thread(thread_id)?;
        match hook.adapter.evaluate(&x.expression) {
            Ok(v) if v.has_children => {
                let mut variable_id = 0;

                self.variables_by_thread
                    .entry(thread_id as u32)
                    .and_modify(|path| {
                        variable_id = path.insert(VariablePath::new_expression(&x.expression))
                    })
                    .or_insert_with(|| {
                        let mut result = VariablesKnownPaths::default();
                        variable_id = result.insert(VariablePath::new_expression(&x.expression));
                        result
                    });

                Ok(dap::EvaluateResponseBody {
                    indexed_variables: None,
                    named_variables: None,
                    presentation_hint: None,
                    result: v.result,
                    type_: Some(v.type_),
                    variables_reference: VariableId::new(0, thread_id as u32, variable_id)?.as_i64()
                        as f64,
                })
            }
            Ok(v) => Ok(dap::EvaluateResponseBody {
                indexed_variables: None,
                named_variables: None,
                presentation_hint: None,
                result: v.result,
                type_: Some(v.type_),
                variables_reference: 0.0,
            }),
            Err(er) => Ok(dap::EvaluateResponseBody {
                indexed_variables: None,
                named_variables: None,
                presentation_hint: None,
                result: format!("{er:#}"),
                type_: None,
                variables_reference: 0.0,
            }),
        }
    }

    fn disconnect(&mut self, _x: dap::DisconnectArguments) -> buck2_error::Result<()> {
        Ok(())
    }
}

impl ServerState {
    fn new(to_client: mpsc::UnboundedSender<ToClientMessage>, project_root: ProjectRoot) -> Self {
        Self {
            to_client,
            project_root,
            current_commands: BuckMutMap::default(),
            current_hooks: BuckMutMap::default(),
            free_pseudo_threads: BTreeSet::new(),
            next_pseudo_thread: 0,
            next_hook_id: HookId(0),
            set_breakpoints: BuckMutMap::default(),
            variables_by_thread: BuckMutMap::default(),
        }
    }

    // The main run loop for the ServerState.
    async fn run(
        &mut self,
        recv: mpsc::UnboundedReceiver<ServerMessage>,
    ) -> buck2_error::Result<()> {
        let mut recv = UnboundedReceiverStream::new(recv);

        self.to_client
            .send(ToClientMessage::Event(dap_event::<()>("initialized", None)))?;

        // We send regular debugger snapshots to all current commands. When we aren't
        // receiving events we send them only once a minute, if there are events they
        // will be reflected in a snapshot within 1sec.
        // As new commands requesting handles are also events, a new command should get
        // its first debugger snapshot within 1s of starting (and can then show UI
        // indicating that a debugger is attached).
        let mut next_snapshot = Instant::now() + Duration::from_secs(1);

        // This loop continues until we encounter an error or get a detach() message.
        loop {
            let timer = tokio::time::sleep_until(next_snapshot);
            select! {
                _ = timer => {
                    let mut snapshot = self.get_snapshot();
                    for (id, state) in &self.current_commands {
                        snapshot.this_handle = id.0;
                        state.events.instant_event(snapshot.clone());
                    }
                    next_snapshot = Instant::now() + Duration::from_mins(1);
                }
                msg = recv.next() => {
                    match msg {
                        None => break,
                        Some(msg) => {
                            if !self.handle_message(msg)? {
                                break;
                            }
                        }
                    }
                    next_snapshot = std::cmp::min(next_snapshot, Instant::now() + Duration::from_secs(1));
                }
            }
        }
        Ok(())
    }

    /// Gets a snapshot with information about all currently stopped starlark evaluations.
    ///
    /// This is sent even when nothing is currently stopped but won't ever be sent if a
    /// debugger isn't attached, and so when a command receives the snapshot it can know
    /// that a debugger is attached to the buck daemon.
    fn get_snapshot(&self) -> buck2_data::DebugAdapterSnapshot {
        let mut current_handles = IntentionallyStdHashMap::new();

        for hook_state in self.current_hooks.values() {
            if let Some(v) = &hook_state.stopped_at {
                let handle_snapshot = current_handles
                    .entry(hook_state.handle_id.0)
                    .or_insert_with(|| buck2_data::DebugAdapterCommandSnapshot {
                        stopped_evals: Vec::new(),
                    });
                handle_snapshot
                    .stopped_evals
                    .push(buck2_data::DebugAdapterStoppedEval {
                        description: hook_state.pseudo_thread_name.clone(),
                        stopped_at: v.clone(),
                    })
            }
        }

        buck2_data::DebugAdapterSnapshot {
            // this will be changed as appropriate before sending the snapshot.
            this_handle: 0,
            current_handles,
        }
    }

    /// Returns `false` on detach to indicate the state thread should stop running.
    fn handle_message(&mut self, msg: ServerMessage) -> buck2_error::Result<bool> {
        match msg {
            ServerMessage::NewHook {
                handle,
                description,
                response_channel,
            } => {
                let resp = self.new_hook(handle, description)?;
                response_channel.send(resp).map_err(|_| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::StarlarkServer,
                        "channel closed"
                    )
                })?;
            }
            ServerMessage::NewHandle { id, events } => self.new_handle(id, events),
            ServerMessage::DropHook { id } => self.drop_hook(id)?,
            ServerMessage::DropHandle { id } => self.drop_handle(id),
            ServerMessage::DapRequest { req } => {
                let response = match dispatch(self, &req) {
                    Ok(resp) => resp,
                    Err(err) => err_response(&req, &err),
                };
                self.to_client.send(ToClientMessage::Response(response))?;
            }
            ServerMessage::EvalStopped { hook_id } => self.eval_stopped(hook_id)?,
            ServerMessage::Detach => {
                self.detach();
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn next_hook_id(&mut self) -> (HookId, u32) {
        let next = self.next_hook_id;
        self.next_hook_id = HookId(self.next_hook_id.0 + 1);
        let pseudo_thread_id = match self.free_pseudo_threads.pop_first() {
            Some(v) => v,
            None => {
                // if only u32 had fetch_add.
                self.next_pseudo_thread += 1;
                self.next_pseudo_thread - 1
            }
        };

        (next, pseudo_thread_id)
    }

    fn new_hook(
        &mut self,
        handle: BuckStarlarkDebuggerHandle,
        description: String,
    ) -> buck2_error::Result<(HookId, Option<Box<dyn DapAdapterEvalHook>>)> {
        let (hook_id, pseudo_thread_id) = self.next_hook_id();

        let client = Box::new(BuckStarlarkDapAdapterClient {
            handle: handle.dupe(),
            hook_id,
        });
        let (hook, eval_wrapper) = prepare_dap_adapter(client);
        let hook_state = HookState {
            adapter: Box::new(hook),
            pseudo_thread_id,
            pseudo_thread_name: description,
            stopped_at: None,
            handle_id: handle.0.id,
        };

        for (source, breakpoints) in &self.set_breakpoints {
            hook_state
                .adapter
                .set_breakpoints(source, breakpoints)
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::StarlarkServer))?;
        }
        self.current_hooks.insert(hook_id, hook_state);

        self.to_client.send(ToClientMessage::Event(dap_event(
            "thread",
            Some(&dap::ThreadEventBody {
                reason: "started".to_owned(),
                thread_id: pseudo_thread_id as i64,
            }),
        )))?;

        Ok((hook_id, Some(Box::new(eval_wrapper))))
    }

    fn new_handle(&mut self, id: HandleId, events: EventDispatcher) {
        self.current_commands.insert(id, CommandState { events });
    }

    fn drop_hook(&mut self, hook_id: HookId) -> buck2_error::Result<()> {
        if let Some(state) = self.current_hooks.remove(&hook_id) {
            self.to_client.send(ToClientMessage::Event(dap_event(
                "thread",
                Some(&dap::ThreadEventBody {
                    reason: "exited".to_owned(),
                    thread_id: state.pseudo_thread_id as i64,
                }),
            )))?;
            self.free_pseudo_threads.insert(state.pseudo_thread_id);
        }
        Ok(())
    }

    fn drop_handle(&mut self, handle_id: HandleId) {
        self.current_commands.remove(&handle_id);
    }

    fn eval_stopped(&mut self, hook_id: HookId) -> buck2_error::Result<()> {
        debug!("eval stopped {}", hook_id);
        let state = self.current_hooks.get_mut(&hook_id).unwrap();
        let top_frame = state.adapter.top_frame();
        let description = match top_frame {
            Ok(Some(v)) => describe_frame(v),
            _ => "???".to_owned(),
        };
        state.stopped_at = Some(description);
        let thread_id = state.pseudo_thread_id;
        self.variables_by_thread.remove(&thread_id);

        let msg = dap::StoppedEventBody {
            reason: "breakpoint".to_owned(),
            thread_id: Some(thread_id as i64),
            description: Some("Hello".to_owned()),
            all_threads_stopped: Some(false),
            preserve_focus_hint: None,
            text: None,
        };

        self.to_client
            .send(ToClientMessage::Event(dap_event("stopped", Some(&msg))))?;
        Ok(())
    }

    fn detach(&mut self) {
        // Dropping the DapAdapter should make any hooked Evaluator continue freely.
        self.current_hooks.clear();
    }

    /// The requests that act on an evaluation are only valid while it is stopped. Injecting one
    /// into a running evaluation blocks this state until the evaluation next stops, and the
    /// evaluation then consumes the stale command and runs through its breakpoint.
    fn hook_id_by_pseudo_thread(&self, thread_id: i64) -> buck2_error::Result<HookId> {
        let thread_id = thread_id as u32;
        for (hook_id, hook_state) in &self.current_hooks {
            if hook_state.pseudo_thread_id == thread_id {
                if hook_state.stopped_at.is_none() {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Thread `{}` is not stopped",
                        thread_id
                    ));
                }
                return Ok(*hook_id);
            }
        }
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::StarlarkServer,
            "can't find evaluator thread"
        ))
    }

    fn find_hook_by_pseudo_thread(&self, thread_id: i64) -> buck2_error::Result<&HookState> {
        let hook_id = self.hook_id_by_pseudo_thread(thread_id)?;
        self.current_hooks
            .get(&hook_id)
            .ok_or_else(|| internal_error!("hook `{}` was just found", hook_id))
    }

    fn find_hook_by_pseudo_thread_mut(
        &mut self,
        thread_id: i64,
    ) -> buck2_error::Result<&mut HookState> {
        let hook_id = self.hook_id_by_pseudo_thread(thread_id)?;
        self.current_hooks
            .get_mut(&hook_id)
            .ok_or_else(|| internal_error!("hook `{}` was just found", hook_id))
    }

    fn get_ast(&self, source: &ProjectRelativePath) -> buck2_error::Result<AstModule> {
        debug!("tried to get ast `{}`", source);
        let abs_path = self.project_root.resolve(source);
        let content = fs_util::read_to_string_if_exists(abs_path)?.ok_or_else(|| {
            buck2_error::buck2_error!(
                buck2_error::ErrorTag::StarlarkServer,
                "file not found: {}",
                source
            )
        })?;
        match AstModule::parse(
            source.as_ref(),
            content,
            &Dialect {
                enable_def: true,
                enable_lambda: true,
                enable_load: true,
                enable_keyword_only_arguments: true,
                enable_types: DialectTypes::ParseOnly,
                enable_load_reexport: false,
                enable_top_level_stmt: true,
                ..Dialect::Standard
            },
        ) {
            Ok(v) => Ok(v),
            Err(e) => Err(e.into()),
        }
    }
}

/// Our implementation of starlark's DapAdapterClient. It basically just needs to
/// forward along events to the server with the hook_id (so the server can tell
/// which evaluation the event came from).
#[derive(Debug)]
struct BuckStarlarkDapAdapterClient {
    handle: BuckStarlarkDebuggerHandle,
    hook_id: HookId,
}

impl DapAdapterClient for BuckStarlarkDapAdapterClient {
    fn event_stopped(&self) -> starlark::Result<()> {
        self.handle.0.server.event_stopped(self.hook_id);
        Ok(())
    }
}

/// Information about ongoing commands held by the debugger server.
#[derive(Debug)]
struct CommandState {
    /// The EventDispatcher for the command. This is used to send debugger snapshots.
    events: EventDispatcher,
}

/// Information about ongoing starlark evaluations held by the debugger server.
#[derive(Debug)]
struct HookState {
    /// The starlark DapAdapter used to send commands into the starlark evaluation.
    adapter: Box<dyn DapAdapter>,
    /// The thread id we report to the DAP client for this evaluation. This is unique
    /// at any point in time (no two concurrent evaluations have the same thread id), but
    /// doesn't correlate to the real thread id.
    pseudo_thread_id: u32,
    /// The thread name that we report to the DAP client. This is a user-friendly description
    /// of the starlark operation on the thread (e.g. "load_buildfile:some_cell//some/package").
    pseudo_thread_name: String,
    /// If the evaluation is stopped (for example, at a breakpoint) this is a description of where
    /// it's stopped at. This is used for the debugger snapshots so that buck commands can provide
    /// UI affordances for the stopped evaluations.
    stopped_at: Option<String>,
    /// The id of the corresponding handle (also used for snapshots so a command can tell if a
    /// stopped evaluation is from itself or another command).
    handle_id: HandleId,
}

/// Provides a simple description of a stack frame, typically "<file>:<line>".
fn describe_frame(frame: dap::StackFrame) -> String {
    match frame {
        dap::StackFrame {
            source: Some(dap::Source {
                path: Some(path), ..
            }),
            line,
            ..
        } => {
            format!("{path}:{line}")
        }
        _ => "???".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::VariableId;

    fn check_variable_err(frame_id: u32, thread_id: u32, variable_id: u32) {
        assert!(
            VariableId::new(frame_id, thread_id, variable_id).is_err(),
            "Expecting error for values ({frame_id}, {thread_id}, {variable_id})"
        );
    }

    fn check_variable_id(frame_id: u32, thread_id: u32, variable_id: u32) {
        let var = VariableId::new(frame_id, thread_id, variable_id).unwrap();
        assert_eq!(
            (var.frame_id(), var.thread_id(), var.variable_id()),
            (frame_id, thread_id, variable_id)
        );
    }

    #[test]
    fn test_variable_id_failures() {
        check_variable_err(0, u32::MAX, u32::MAX);
        // thread_id exceeds 20-bit limit
        check_variable_err(0, 0xFFFFF + 1, 0);
        // variable_id exceeds 20-bit limit
        check_variable_err(0, 0, 0xFFFFF + 1);
        // frame_id exceeds 7-bit limit
        check_variable_err(0x80, 0, 0);
    }

    #[test]
    fn test_variable_id() {
        check_variable_id(0, 1234, 98673);
        check_variable_id(0, 0, 0);
        check_variable_id(1, 0, 0);
        check_variable_id(0, 0xFFFFF, 0xFFFFF);
        check_variable_id(1, 0xFFFFF, 0xFFFFF);
        check_variable_id(0x7F, 0xFFFFF, 0xFFFFF);
        check_variable_id(0, 0xFFFFF, 0);
        check_variable_id(5, 0xFFFFF - 1, 0xFFFFF / 2);
        check_variable_id(0x7F, 0xFFFFF - 1, 0xFFFFF / 2);
    }

    #[test]
    fn test_variable_id_is_never_zero() {
        // DAP reserves `variables_reference: 0` as the "no children" sentinel.
        // Every encoded VariableId must be non-zero so legitimate scope and variable
        // references on `(frame_id=0, thread_id=0, variable_id=0)` don't collide
        // with that sentinel and cause clients to render scopes as empty.
        let permutations = [
            (0, 0, 0),
            (0, 0, 1),
            (0, 1, 0),
            (1, 0, 0),
            (0x7F, 0xFFFFF, 0xFFFFF),
        ];
        for (frame_id, thread_id, variable_id) in permutations {
            let var = VariableId::new(frame_id, thread_id, variable_id).unwrap();
            assert_ne!(
                var.as_i64(),
                0,
                "Encoded VariableId for ({frame_id}, {thread_id}, {variable_id}) was 0"
            );
        }
    }
}

#[cfg(test)]
mod stopped_eval_tests {
    use std::sync::Arc;
    use std::sync::Condvar;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_events::Event;
    use buck2_events::create_source_sink_pair;
    use buck2_events::daemon_id::DaemonId;
    use buck2_events::dispatch::EventDispatcher;
    use buck2_events::source::ChannelEventSource;
    use buck2_interpreter::starlark_debug::StarlarkDebugController;
    use debugserver_types as dap;
    use dupe::Dupe;
    use starlark::environment::GlobalsBuilder;
    use starlark::environment::Module;
    use starlark::eval::Evaluator;
    use starlark::starlark_module;
    use starlark::syntax::AstModule;
    use starlark::syntax::Dialect;
    use starlark::values::none::NoneType;
    use tokio::sync::mpsc;

    use crate::BuckStarlarkDebuggerHandle;
    use crate::run::ToClientMessage;
    use crate::server::BuckStarlarkDebuggerServer;

    /// Holds a starlark evaluation inside a native call until the test releases it.
    struct Gate {
        open: Mutex<bool>,
        cv: Condvar,
    }

    impl Gate {
        const fn new() -> Self {
            Self {
                open: Mutex::new(false),
                cv: Condvar::new(),
            }
        }

        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.cv.wait(open).unwrap();
            }
        }

        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.cv.notify_all();
        }
    }

    static GATE_STALE: Gate = Gate::new();
    static GATE_EARLY: Gate = Gate::new();

    #[starlark_module]
    fn gate_functions(builder: &mut GlobalsBuilder) {
        fn wait_gate_stale() -> starlark::Result<NoneType> {
            GATE_STALE.wait();
            Ok(NoneType)
        }

        fn wait_gate_early() -> starlark::Result<NoneType> {
            GATE_EARLY.wait();
            Ok(NoneType)
        }
    }

    fn request(seq: i64, command: &str, arguments: serde_json::Value) -> dap::Request {
        serde_json::from_value(serde_json::json!({
            "seq": seq, "type": "request", "command": command, "arguments": arguments
        }))
        .unwrap()
    }

    struct Setup {
        root: ProjectRootTemp,
        server: Arc<BuckStarlarkDebuggerServer>,
        to_client: mpsc::UnboundedReceiver<ToClientMessage>,
        source: ChannelEventSource,
        handle: BuckStarlarkDebuggerHandle,
    }

    /// A server with one command handle and one breakpoint on `line` of `test.bzl`.
    async fn setup(content: &str, line: i64) -> Setup {
        let root = ProjectRootTemp::new().unwrap();
        root.write_file("test.bzl", content);
        let abs = root
            .path()
            .resolve(ProjectRelativePath::new("test.bzl").unwrap())
            .to_string();
        let (send, mut to_client) = mpsc::unbounded_channel();
        let server = Arc::new(BuckStarlarkDebuggerServer::new(send, root.path().dupe()));
        let (source, sink) = create_source_sink_pair();
        let handle = server
            .new_handle(EventDispatcher::new(
                EventDispatcher::null().trace_id().clone(),
                DaemonId::null(),
                sink,
            ))
            .unwrap();
        server
            .send_request(request(
                1,
                "setBreakpoints",
                serde_json::json!({"source": {"path": abs}, "breakpoints": [{"line": line}]}),
            ))
            .unwrap();
        loop {
            match to_client.recv().await.unwrap() {
                ToClientMessage::Response(r) if r.command == "setBreakpoints" => {
                    assert!(r.success, "{:?}", r.message);
                    break;
                }
                _ => {}
            }
        }
        Setup {
            root,
            server,
            to_client,
            source,
            handle,
        }
    }

    fn run_eval(controller: &mut Box<dyn StarlarkDebugController>, content: &str) {
        let ast = AstModule::parse("test.bzl", content.to_owned(), &Dialect::Standard).unwrap();
        let globals = GlobalsBuilder::standard().with(gate_functions).build();
        Module::with_temp_heap(|module| {
            let mut eval = Evaluator::new(&module);
            controller.initialize(&mut eval).unwrap();
            eval.eval_module(ast, &globals).unwrap();
        });
    }

    /// The number of stopped evaluations a debugger snapshot event reports, if `ev` is one.
    fn stopped_evals_in(ev: &Event) -> Option<usize> {
        let Event::Buck(ev) = ev else {
            return None;
        };
        let buck2_data::buck_event::Data::Instant(inst) = ev.data() else {
            return None;
        };
        let Some(buck2_data::instant_event::Data::DebugAdapterSnapshot(snap)) = &inst.data else {
            return None;
        };
        Some(
            snap.current_handles
                .values()
                .map(|h| h.stopped_evals.len())
                .sum(),
        )
    }

    /// Stops at a breakpoint inside `f`, continues, and collects the stopped-evaluation count
    /// of the snapshots the server emits after the continue was answered.
    async fn stopped_evals_after_continue() -> Vec<usize> {
        // Breakpoints go inside a function: a top-level statement is hit twice.
        let content = "def f():\n    x = 1\n    wait_gate_stale()\n    return x\nf()\n";
        let Setup {
            root: _root,
            server,
            mut to_client,
            mut source,
            handle,
        } = setup(content, 2).await;
        let mut controller = server.start_eval(&handle, "test eval").await.unwrap();
        let srv = server.dupe();
        let client = std::thread::spawn(move || {
            let tid = loop {
                match to_client.blocking_recv().unwrap() {
                    ToClientMessage::Event(ev) if ev.event == "stopped" => {
                        break ev.body.unwrap()["threadId"].as_i64().unwrap();
                    }
                    _ => {}
                }
            };
            srv.send_request(request(2, "continue", serde_json::json!({"threadId": tid})))
                .unwrap();
            loop {
                match to_client.blocking_recv().unwrap() {
                    ToClientMessage::Response(r) if r.command == "continue" => {
                        assert!(r.success, "{:?}", r.message);
                        break;
                    }
                    _ => {}
                }
            }
            // Everything the server emits from now on is after the continue took effect.
            while source.try_receive().is_some() {}
            // A request makes the server schedule a snapshot within a second.
            srv.send_request(request(3, "threads", serde_json::json!({})))
                .unwrap();
            let start = std::time::Instant::now();
            let mut snapshots = Vec::new();
            while std::time::Instant::now() - start < Duration::from_secs(4) && snapshots.is_empty()
            {
                match source.try_receive() {
                    Some(ev) => snapshots.extend(stopped_evals_in(&ev)),
                    None => std::thread::sleep(Duration::from_millis(20)),
                }
            }
            GATE_STALE.release();
            snapshots
        });
        tokio::task::block_in_place(|| run_eval(&mut controller, content));
        let snapshots = client.join().unwrap();
        drop(controller);
        snapshots
    }

    /// After `continue` the snapshots no longer list the evaluation as stopped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_stops_listing_an_eval_after_continue() {
        assert_eq!(stopped_evals_after_continue().await, vec![0]);
    }

    /// Sends `continue` and then `threads` while the evaluation is running (held in a native
    /// call before its breakpoint). Returns the responses answered while it ran, as (command,
    /// success), and whether the evaluation finished although the client never continued it
    /// after its stop.
    async fn continue_while_running() -> (Vec<(String, bool)>, bool) {
        let content = "def f():\n    wait_gate_early()\n    y = 2\n    return y\nf()\n";
        let Setup {
            root: _root,
            server,
            mut to_client,
            source: _source,
            handle,
        } = setup(content, 3).await;
        let mut controller = server.start_eval(&handle, "test eval").await.unwrap();
        let eval_finished = Arc::new(AtomicBool::new(false));
        let finished = eval_finished.dupe();
        let srv = server.dupe();
        let client = std::thread::spawn(move || {
            let tid = loop {
                match to_client.blocking_recv().unwrap() {
                    ToClientMessage::Event(ev) if ev.event == "thread" => {
                        break ev.body.unwrap()["threadId"].as_i64().unwrap();
                    }
                    _ => {}
                }
            };
            // Give the evaluation time to reach the native call; it is running, not stopped.
            std::thread::sleep(Duration::from_millis(200));
            srv.send_request(request(2, "continue", serde_json::json!({"threadId": tid})))
                .unwrap();
            srv.send_request(request(3, "threads", serde_json::json!({})))
                .unwrap();
            let mut answered_while_running = Vec::new();
            let start = std::time::Instant::now();
            while std::time::Instant::now() - start < Duration::from_secs(2) {
                match to_client.try_recv() {
                    Ok(ToClientMessage::Response(r)) => {
                        answered_while_running.push((r.command, r.success))
                    }
                    Ok(_) => {}
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
            GATE_EARLY.release();
            // The client does not continue after the stop; see whether the evaluation ends anyway.
            let start = std::time::Instant::now();
            let mut finished_without_continue = false;
            while std::time::Instant::now() - start < Duration::from_secs(2) {
                let _ignored = to_client.try_recv();
                if finished.load(Ordering::SeqCst) {
                    finished_without_continue = true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if !finished.load(Ordering::SeqCst) {
                srv.send_request(request(4, "continue", serde_json::json!({"threadId": tid})))
                    .unwrap();
            }
            (answered_while_running, finished_without_continue)
        });
        tokio::task::block_in_place(|| run_eval(&mut controller, content));
        eval_finished.store(true, Ordering::SeqCst);
        drop(controller);
        client.join().unwrap()
    }

    /// A `continue` for a running evaluation is refused, the server keeps answering, and the
    /// evaluation waits at its breakpoint for the client.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn continue_for_a_running_eval_is_refused() {
        let (answered, finished_without_continue) = continue_while_running().await;
        assert_eq!(
            answered,
            vec![("continue".to_owned(), false), ("threads".to_owned(), true)]
        );
        assert!(!finished_without_continue);
    }
}
