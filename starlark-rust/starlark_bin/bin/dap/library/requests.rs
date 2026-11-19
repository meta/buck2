/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use debugserver_types::*;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

pub(crate) trait DebugServer {
    fn initialize(&self, x: InitializeRequestArguments) -> anyhow::Result<Option<Capabilities>>;
    fn set_breakpoints(
        &self,
        x: SetBreakpointsArguments,
    ) -> anyhow::Result<SetBreakpointsResponseBody>;
    fn set_exception_breakpoints(&self, x: SetExceptionBreakpointsArguments) -> anyhow::Result<()>;
    fn launch(&self, x: LaunchRequestArguments, args: Map<String, Value>) -> anyhow::Result<()>;
    fn threads(&self) -> anyhow::Result<ThreadsResponseBody>;
    fn configuration_done(&self) -> anyhow::Result<()>;
    fn stack_trace(&self, x: StackTraceArguments) -> anyhow::Result<StackTraceResponseBody>;
    fn scopes(&self, x: ScopesArguments) -> anyhow::Result<ScopesResponseBody>;
    fn variables(&self, x: VariablesArguments) -> anyhow::Result<VariablesResponseBody>;
    fn continue_(&self, x: ContinueArguments) -> anyhow::Result<ContinueResponseBody>;
    fn evaluate(&self, x: EvaluateArguments) -> anyhow::Result<EvaluateResponseBody>;
    fn disconnect(&self, _x: DisconnectArguments) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(crate) fn dispatch(server: &impl DebugServer, r: &Request) -> Response {
    // `arguments` is optional in the protocol, so a missing object means "no arguments".
    fn arg<T: for<'a> Deserialize<'a>>(r: &Request) -> anyhow::Result<T> {
        let arguments = r
            .arguments
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new()));
        Ok(serde_json::from_value(arguments)?)
    }

    fn arg_extra(r: &Request) -> Map<String, Value> {
        match &r.arguments {
            Some(Value::Object(x)) => x.clone(),
            _ => Default::default(),
        }
    }

    fn ret<T: Serialize>(r: &Request, v: anyhow::Result<Option<T>>) -> Response {
        Response {
            type_: "response".to_owned(),
            command: r.command.clone(),
            request_seq: r.seq,
            seq: 0,
            success: v.is_ok(),
            message: v.as_ref().err().map(|e| format!("{e:#}")),
            body: v.unwrap_or(None).map(|v| serde_json::to_value(v).unwrap()),
        }
    }

    fn ret_some<T: Serialize>(r: &Request, v: anyhow::Result<T>) -> Response {
        ret(r, v.map(Some))
    }

    fn ret_none(r: &Request, v: anyhow::Result<()>) -> Response {
        ret::<()>(r, v.map(|_| None))
    }

    match r.command.as_str() {
        "initialize" => ret(r, arg(r).and_then(|x| server.initialize(x))),
        "setBreakpoints" => ret_some(r, arg(r).and_then(|x| server.set_breakpoints(x))),
        "setExceptionBreakpoints" => {
            ret_none(r, arg(r).and_then(|x| server.set_exception_breakpoints(x)))
        }
        "launch" => ret_none(r, arg(r).and_then(|x| server.launch(x, arg_extra(r)))),
        "threads" => ret_some(r, server.threads()),
        "configurationDone" => ret_none(r, server.configuration_done()),
        "stackTrace" => ret_some(r, arg(r).and_then(|x| server.stack_trace(x))),
        "scopes" => ret_some(r, arg(r).and_then(|x| server.scopes(x))),
        "variables" => ret_some(r, arg(r).and_then(|x| server.variables(x))),
        "continue" => ret_some(r, arg(r).and_then(|x| server.continue_(x))),
        "evaluate" => ret_some(r, arg(r).and_then(|x| server.evaluate(x))),
        "disconnect" => ret_none(r, arg(r).and_then(|x| server.disconnect(x))),
        _ => ret_none(r, Err(anyhow::anyhow!("Unknown command: {}", r.command))),
    }
}
