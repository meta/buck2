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

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use debugserver_types::*;
use dupe::Dupe;
pub(crate) use library::*;
use serde_json::Map;
use serde_json::Value;
use starlark::StarlarkResultExt;
use starlark::debug::DapAdapter;
use starlark::debug::DapAdapterClient;
use starlark::debug::DapAdapterEvalHook;
use starlark::debug::dap_capabilities;
use starlark::debug::prepare_dap_adapter;
use starlark::debug::resolve_breakpoints;
use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;

mod library;

#[derive(Debug)]
struct Backend {
    adapter: Arc<dyn DapAdapter>,
    eval_wrapper: Mutex<Option<Box<dyn DapAdapterEvalHook>>>,
    client: Client,
    file: Mutex<Option<String>>,
    dialect: Dialect,
    globals: Globals,
}

impl DapAdapterClient for Client {
    fn event_stopped(&self) -> starlark::Result<()> {
        self.event_stopped(StoppedEventBody {
            reason: "breakpoint".to_owned(),
            thread_id: Some(0),
            description: Some("Hello".to_owned()),
            all_threads_stopped: Some(true),
            preserve_focus_hint: None,
            text: None,
        });
        Ok(())
    }
}

impl Backend {
    fn execute(&self, path: &str) {
        let client = self.client.dupe();
        let client2 = self.client.dupe();
        let wrapper = self.eval_wrapper.lock().unwrap().take().unwrap();
        let path = PathBuf::from(path);
        let dialect = self.dialect.clone();
        let globals = self.globals.dupe();

        let go = move || -> anyhow::Result<String> {
            client.log(&format!("EVALUATION PREPARE: {}", path.display()));
            let ast = AstModule::parse_file(&path, &dialect).into_anyhow_result()?;
            Module::with_temp_heap(|module| {
                let mut eval = Evaluator::new(&module);
                wrapper.add_dap_hooks(&mut eval);

                // No way to pass back success/failure to the caller
                client.log(&format!("EVALUATION START: {}", path.display()));
                let v = eval.eval_module(ast, &globals).into_anyhow_result()?;
                let s = v.to_string();
                client.log(&format!("EVALUATION FINISHED: {}", path.display()));
                Ok::<_, anyhow::Error>(s)
            })
        };

        thread::spawn(move || {
            let res = go();
            let output = match &res {
                Err(e) => format!("{e:#}"),
                Ok(v) => v.to_owned(),
            };
            client2.event_output(OutputEventBody {
                output,
                category: None,
                column: None,
                data: None,
                line: None,
                source: None,
                variables_reference: None,
            });
            client2.event_exited(ExitedEventBody {
                exit_code: if res.is_ok() { 0 } else { 1 },
            });
            client2.event_terminated(None);
        });
    }

    fn get_ast(&self, source: &str) -> anyhow::Result<Arc<AstModule>> {
        Ok(Arc::new(
            AstModule::parse_file(Path::new(source), &self.dialect).into_anyhow_result()?,
        ))
    }
}

impl DebugServer for Backend {
    fn initialize(&self, _: InitializeRequestArguments) -> anyhow::Result<Option<Capabilities>> {
        self.client.event_initialized(None);
        Ok(Some(dap_capabilities()))
    }

    fn set_breakpoints(
        &self,
        x: SetBreakpointsArguments,
    ) -> anyhow::Result<SetBreakpointsResponseBody> {
        let source = x.source.path.as_ref().unwrap();
        let resolved = resolve_breakpoints(&x, &*self.get_ast(source)?)?;
        self.adapter.set_breakpoints(source, &resolved)?;
        Ok(resolved.to_response())
    }

    fn set_exception_breakpoints(&self, _: SetExceptionBreakpointsArguments) -> anyhow::Result<()> {
        // We just assume that break on error is always useful
        Ok(())
    }

    fn launch(&self, _: LaunchRequestArguments, args: Map<String, Value>) -> anyhow::Result<()> {
        // Expecting program of type string
        match args.get("program") {
            Some(Value::String(path)) => {
                *self.file.lock().unwrap() = Some(path.to_owned());
                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "Couldn't find a program to launch, got args {:?}",
                args
            )),
        }
    }

    fn threads(&self) -> anyhow::Result<ThreadsResponseBody> {
        Ok(ThreadsResponseBody {
            threads: vec![Thread {
                id: 0,
                name: "main".to_owned(),
            }],
        })
    }

    fn configuration_done(&self) -> anyhow::Result<()> {
        if let Some(path) = self.file.lock().unwrap().as_ref() {
            self.execute(path);
        }
        Ok(())
    }

    fn stack_trace(&self, v: StackTraceArguments) -> anyhow::Result<StackTraceResponseBody> {
        self.adapter.stack_trace(v)
    }

    fn scopes(&self, args: ScopesArguments) -> anyhow::Result<ScopesResponseBody> {
        let frame_id = args.frame_id.try_into()?;
        let scopes_info = self.adapter.scopes(frame_id)?;
        Ok(ScopesResponseBody {
            scopes: vec![Scope {
                name: "Locals".to_owned(),
                named_variables: Some(scopes_info.num_locals as i64),
                // Encode frame_id into variables_reference. DAP uses 0 to mean
                // "no children", so offset by 1.
                variables_reference: frame_id as i64 + 1,
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

    fn variables(&self, args: VariablesArguments) -> anyhow::Result<VariablesResponseBody> {
        if args.variables_reference < 1 {
            return Err(anyhow::anyhow!(
                "invalid variables_reference: {}",
                args.variables_reference
            ));
        }
        let frame_id = (args.variables_reference - 1) as usize;
        let vars_info = self.adapter.variables(frame_id)?;
        Ok(VariablesResponseBody {
            variables: vars_info
                .locals
                .into_iter()
                .map(|var| var.to_dap())
                .collect(),
        })
    }

    fn evaluate(&self, x: EvaluateArguments) -> anyhow::Result<EvaluateResponseBody> {
        let expr_result = self.adapter.evaluate(&x.expression)?;

        Ok(EvaluateResponseBody {
            indexed_variables: None,
            named_variables: None,
            presentation_hint: None,
            result: expr_result.result,
            type_: Some(expr_result.type_),
            variables_reference: 0.0,
        })
    }

    fn continue_(&self, _: ContinueArguments) -> anyhow::Result<ContinueResponseBody> {
        self.adapter.continue_()?;
        Ok(ContinueResponseBody::default())
    }
}

pub(crate) fn server(dialect: Dialect, globals: Globals) {
    DapService::run(|client| {
        let (adapter, wrapper) = prepare_dap_adapter(Box::new(client.dupe()));
        Backend {
            adapter: Arc::new(adapter),
            eval_wrapper: Mutex::new(Some(Box::new(wrapper))),
            client,
            file: Default::default(),
            dialect,
            globals,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::hint;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    #[derive(Debug)]
    struct TestClient {
        breakpoints_hit: Arc<AtomicUsize>,
    }

    impl DapAdapterClient for TestClient {
        fn event_stopped(&self) -> starlark::Result<()> {
            self.breakpoints_hit.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct TestHarness {
        breakpoints_hit: Arc<AtomicUsize>,
        backend: Backend,
        eval_hook: Option<Box<dyn DapAdapterEvalHook>>,
    }

    impl TestHarness {
        fn new() -> Self {
            let breakpoints_hit = Arc::new(AtomicUsize::new(0));
            let client = TestClient {
                breakpoints_hit: breakpoints_hit.clone(),
            };
            let (adapter, eval_hook) = prepare_dap_adapter(Box::new(client));
            // Backend requires a Client, but we only call methods that don't use it
            // (scopes, variables, stack_trace, continue_). The Client field is unused
            // in those code paths.
            let backend = Backend {
                adapter: Arc::new(adapter),
                eval_wrapper: Mutex::new(None),
                client: Client::new(),
                file: Default::default(),
                dialect: Dialect::AllOptionsInternal,
                globals: Globals::extended_internal(),
            };
            Self {
                breakpoints_hit,
                backend,
                eval_hook: Some(Box::new(eval_hook)),
            }
        }

        fn take_eval_hook(&mut self) -> Box<dyn DapAdapterEvalHook> {
            self.eval_hook.take().unwrap()
        }

        fn wait_for_eval_stopped(&self, count: usize) {
            let timeout = Duration::from_secs(10);
            let start = Instant::now();
            loop {
                let hit = self.breakpoints_hit.load(Ordering::SeqCst);
                assert!(hit <= count, "too many breakpoint hits");
                if hit == count {
                    return;
                }
                if start.elapsed() > timeout {
                    panic!("timed out waiting for breakpoint hit");
                }
                hint::spin_loop();
            }
        }
    }

    fn breakpoints_args(path: &str, lines: &[(i64, Option<&str>)]) -> SetBreakpointsArguments {
        SetBreakpointsArguments {
            breakpoints: Some(
                lines
                    .iter()
                    .map(|(line, condition)| SourceBreakpoint {
                        column: None,
                        condition: condition.map(|v| v.to_owned()),
                        hit_condition: None,
                        line: *line,
                        log_message: None,
                    })
                    .collect(),
            ),
            lines: None,
            source: Source {
                adapter_data: None,
                checksums: None,
                name: None,
                origin: None,
                path: Some(path.to_owned()),
                presentation_hint: None,
                source_reference: None,
                sources: None,
            },
            source_modified: None,
        }
    }

    fn eval_with_hook(ast: AstModule, hook: Box<dyn DapAdapterEvalHook>) -> starlark::Result<()> {
        let globals = Globals::extended_internal();
        Module::with_temp_heap(|module| {
            let mut eval = Evaluator::new(&module);
            hook.add_dap_hooks(&mut eval);
            eval.eval_module(ast, &globals)?;
            Ok(())
        })
    }

    fn join_timeout<T>(handle: thread::ScopedJoinHandle<T>, timeout: Duration) -> T {
        let start = Instant::now();
        while !handle.is_finished() {
            if start.elapsed() > timeout {
                panic!("timed out waiting for thread");
            }
        }
        handle.join().unwrap()
    }

    /// `arguments` is optional in the protocol: a request without it is answered, and a
    /// request whose arguments do not parse gets an error response instead of a crash.
    #[test]
    fn requests_without_arguments_are_answered() {
        let harness = TestHarness::new();
        let request = |command: &str| Request {
            arguments: None,
            command: command.to_owned(),
            seq: 1,
            type_: "request".to_owned(),
        };
        let response = dispatch(&harness.backend, &request("disconnect"));
        assert!(response.success, "{response:?}");
        let response = dispatch(&harness.backend, &request("stackTrace"));
        assert!(!response.success, "{response:?}");
        assert!(response.message.is_some(), "{response:?}");
    }

    fn dap_request(args: impl serde::Serialize, command: &str, seq: i64) -> Request {
        Request {
            arguments: Some(serde_json::to_value(args).unwrap()),
            command: command.to_owned(),
            seq,
            type_: "request".to_owned(),
        }
    }

    fn response_body<T: serde::de::DeserializeOwned>(response: Response) -> anyhow::Result<T> {
        assert!(response.success, "{response:?}");
        serde_json::from_value(response.body.unwrap()).map_err(Into::into)
    }

    #[test]
    fn test_multi_frame_scopes_and_variables() -> anyhow::Result<()> {
        let file_contents = "\
def inner(z):
    return z + 1

def middle(y):
    return inner(y * 2)

def outer(x):
    return middle(x + 1)

outer(5)
";
        let mut harness = TestHarness::new();
        let ast = AstModule::parse(
            "test.bzl",
            file_contents.to_owned(),
            &Dialect::AllOptionsInternal,
        )
        .into_anyhow_result()?;
        let breakpoints = resolve_breakpoints(&breakpoints_args("test.bzl", &[(2, None)]), &ast)?;
        harness
            .backend
            .adapter
            .set_breakpoints("test.bzl", &breakpoints)?;

        let eval_hook = harness.take_eval_hook();
        let timeout = Duration::from_secs(10);

        thread::scope(|s| -> anyhow::Result<()> {
            let eval_result =
                s.spawn(move || -> starlark::Result<()> { eval_with_hook(ast, eval_hook) });
            harness.wait_for_eval_stopped(1);

            let scopes_0: ScopesResponseBody = response_body(dispatch(
                &harness.backend,
                &dap_request(ScopesArguments { frame_id: 0 }, "scopes", 1),
            ))?;
            let scopes_1: ScopesResponseBody = response_body(dispatch(
                &harness.backend,
                &dap_request(ScopesArguments { frame_id: 1 }, "scopes", 2),
            ))?;
            let scopes_2: ScopesResponseBody = response_body(dispatch(
                &harness.backend,
                &dap_request(ScopesArguments { frame_id: 2 }, "scopes", 3),
            ))?;

            assert_eq!(1, scopes_0.scopes.len());
            assert_eq!(1, scopes_1.scopes.len());
            assert_eq!(1, scopes_2.scopes.len());

            // Verify each scope reports the right number of locals.
            assert_eq!(Some(1), scopes_0.scopes[0].named_variables); // inner: z
            assert_eq!(Some(1), scopes_1.scopes[0].named_variables); // middle: y
            assert_eq!(Some(1), scopes_2.scopes[0].named_variables); // outer: x

            // Verify variables_reference encodes the frame_id (frame_id + 1).
            assert_eq!(1, scopes_0.scopes[0].variables_reference); // 0 + 1
            assert_eq!(2, scopes_1.scopes[0].variables_reference); // 1 + 1
            assert_eq!(3, scopes_2.scopes[0].variables_reference); // 2 + 1

            let vars_0: VariablesResponseBody = response_body(dispatch(
                &harness.backend,
                &dap_request(
                    VariablesArguments {
                        variables_reference: scopes_0.scopes[0].variables_reference,
                        count: None,
                        filter: None,
                        format: None,
                        start: None,
                    },
                    "variables",
                    4,
                ),
            ))?;
            let vars_1: VariablesResponseBody = response_body(dispatch(
                &harness.backend,
                &dap_request(
                    VariablesArguments {
                        variables_reference: scopes_1.scopes[0].variables_reference,
                        count: None,
                        filter: None,
                        format: None,
                        start: None,
                    },
                    "variables",
                    5,
                ),
            ))?;
            let vars_2: VariablesResponseBody = response_body(dispatch(
                &harness.backend,
                &dap_request(
                    VariablesArguments {
                        variables_reference: scopes_2.scopes[0].variables_reference,
                        count: None,
                        filter: None,
                        format: None,
                        start: None,
                    },
                    "variables",
                    6,
                ),
            ))?;

            // Frame 0 (inner): z = (5+1)*2 = 12
            assert_eq!(1, vars_0.variables.len());
            assert_eq!("z", vars_0.variables[0].name);
            assert_eq!("12", vars_0.variables[0].value);

            // Frame 1 (middle): y = 5+1 = 6
            assert_eq!(1, vars_1.variables.len());
            assert_eq!("y", vars_1.variables[0].name);
            assert_eq!("6", vars_1.variables[0].value);

            // Frame 2 (outer): x = 5
            assert_eq!(1, vars_2.variables.len());
            assert_eq!("x", vars_2.variables[0].name);
            assert_eq!("5", vars_2.variables[0].value);

            harness
                .backend
                .continue_(ContinueArguments { thread_id: 0 })?;
            join_timeout(eval_result, timeout).into_anyhow_result()?;
            Ok(())
        })
    }
}
