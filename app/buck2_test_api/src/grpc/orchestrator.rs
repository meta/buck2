/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use buck2_downward_api::DownwardApi;
use buck2_downward_api_proto::ConsoleRequest;
use buck2_downward_api_proto::ExternalEventRequest;
use buck2_downward_api_proto::LogRequest;
use buck2_downward_api_proto::downward_api_client;
use buck2_downward_api_proto::downward_api_server;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_grpc::ServerHandle;
use buck2_grpc::make_channel;
use buck2_grpc::spawn_oneshot;
use buck2_grpc::to_tonic;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutMap;
use buck2_test_proto::AttachInfoMessageRequest;
use buck2_test_proto::Empty;
use buck2_test_proto::EndOfTestResultsRequest;
use buck2_test_proto::ExecuteResponse2;
use buck2_test_proto::PrepareForLocalExecutionResponse;
use buck2_test_proto::ReportTestResultRequest;
use buck2_test_proto::ReportTestSessionRequest;
use buck2_test_proto::ReportTestsDiscoveredRequest;
use buck2_test_proto::Testing;
use buck2_test_proto::UploadFileToCasRequest;
use buck2_test_proto::UploadFileToCasResponse;
use buck2_test_proto::test_orchestrator_client;
use buck2_test_proto::test_orchestrator_server;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use host_sharing::HostSharingRequirements;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tonic::transport::Channel;
use tower_layer::Layer;
use tracing::Level;

use crate::data::ArgValue;
use crate::data::CasDigest;
use crate::data::ConfiguredTargetHandle;
use crate::data::DeclaredOutput;
use crate::data::ExecuteRequest2;
use crate::data::ExecuteResponse;
use crate::data::ExecutorConfigOverride;
use crate::data::PrepareForLocalExecutionResult;
use crate::data::RequiredLocalResources;
use crate::data::TestExecutable;
use crate::data::TestResult;
use crate::data::TestStage;
use crate::protocol::TestOrchestrator;

/// Test runner client to buck2 test orchestrator.
///
/// When running `buck2 test`, buck2 starts a gRPC server
/// and spawns a test runner which connects to buck2 process using this client.
pub struct TestOrchestratorClient {
    test_orchestrator_client: test_orchestrator_client::TestOrchestratorClient<Channel>,
    downward_api_client: downward_api_client::DownwardApiClient<Channel>,
}

impl TestOrchestratorClient {
    pub async fn new<T>(io: T) -> buck2_error::Result<Self>
    where
        T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    {
        let channel = make_channel(io, "orchestrator").await?;

        Ok(Self {
            test_orchestrator_client: test_orchestrator_client::TestOrchestratorClient::new(
                channel.clone(),
            )
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX),
            downward_api_client: downward_api_client::DownwardApiClient::new(channel)
                .max_encoding_message_size(usize::MAX)
                .max_decoding_message_size(usize::MAX),
        })
    }
}

#[async_trait::async_trait]
impl DownwardApi for TestOrchestratorClient {
    async fn console(&self, level: Level, message: String) -> buck2_error::Result<()> {
        let level = level.try_into().buck_error_context("Invalid `level`")?;

        self.downward_api_client
            .clone()
            .console(ConsoleRequest {
                level: Some(level),
                message,
            })
            .await?;

        Ok(())
    }

    async fn log(&self, level: Level, message: String) -> buck2_error::Result<()> {
        let level = level.try_into().buck_error_context("Invalid `level`")?;

        self.downward_api_client
            .clone()
            .log(LogRequest {
                level: Some(level),
                message,
            })
            .await?;

        Ok(())
    }

    async fn external(&self, data: BuckMutMap<String, String>) -> buck2_error::Result<()> {
        let event = data.into();

        self.downward_api_client
            .clone()
            .external_event(ExternalEventRequest { event: Some(event) })
            .await?;

        Ok(())
    }
}

impl TestOrchestratorClient {
    pub async fn execute2(
        &self,
        ui_prints: TestStage,
        target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        timeout: Duration,
        host_sharing_requirements: HostSharingRequirements,
        pre_create_dirs: Vec<DeclaredOutput>,
        executor_override: Option<ExecutorConfigOverride>,
        required_local_resources: RequiredLocalResources,
        disable_test_execution_caching: bool,
    ) -> buck2_error::Result<ExecuteResponse> {
        let test_executable = TestExecutable {
            stage: ui_prints,
            target,
            cmd,
            env,
            pre_create_dirs,
        };

        let req = ExecuteRequest2 {
            test_executable,
            timeout,
            host_sharing_requirements,
            executor_override,
            required_local_resources,
            disable_test_execution_caching,
        };

        let req: buck2_test_proto::ExecuteRequest2 = req
            .try_into()
            .buck_error_context("Invalid execute request")?;

        let ExecuteResponse2 { response } = self
            .test_orchestrator_client
            .clone()
            .execute2(req)
            .await?
            .into_inner();

        let response = match response.internal_error("Missing `response`")? {
            buck2_test_proto::execute_response2::Response::Result(res) => {
                ExecuteResponse::Result(res.try_into().buck_error_context("Invalid `result`")?)
            }
            buck2_test_proto::execute_response2::Response::Cancelled(
                buck2_test_proto::Cancelled { reason },
            ) => {
                let reason = match reason {
                    Some(reason) => buck2_test_proto::CancellationReason::try_from(reason)
                        .map(|proto_reason| match proto_reason {
                            buck2_test_proto::CancellationReason::NotSpecified => {
                                crate::data::CancellationReason::NotSpecified
                            }
                            buck2_test_proto::CancellationReason::ReQueueTimeout => {
                                crate::data::CancellationReason::ReQueueTimeout
                            }
                        })
                        .ok(),
                    None => None,
                };
                ExecuteResponse::Cancelled(reason)
            }
        };

        Ok(response)
    }

    pub async fn report_test_result(&self, result: TestResult) -> buck2_error::Result<()> {
        let result = result.try_into().buck_error_context("Invalid `result`")?;

        self.test_orchestrator_client
            .clone()
            .report_test_result(ReportTestResultRequest {
                result: Some(result),
            })
            .await?;

        Ok(())
    }

    pub async fn report_tests_discovered(
        &self,
        target: ConfiguredTargetHandle,
        suite: String,
        tests: Vec<String>,
    ) -> buck2_error::Result<()> {
        let target = target.try_into().buck_error_context("Invalid `target`")?;

        self.test_orchestrator_client
            .clone()
            .report_tests_discovered(ReportTestsDiscoveredRequest {
                target: Some(target),
                testing: Some(Testing {
                    suite,
                    testcases: tests,
                    variant: None,
                    repeat_count: None,
                }),
            })
            .await?;

        Ok(())
    }

    pub async fn report_test_session(
        &self,
        session_info: String,
        test_session_id: Option<String>,
    ) -> buck2_error::Result<()> {
        self.test_orchestrator_client
            .clone()
            .report_test_session(ReportTestSessionRequest {
                session_info,
                test_session_id,
            })
            .await?;

        Ok(())
    }

    pub async fn end_of_test_results(&self, exit_code: i32) -> buck2_error::Result<()> {
        self.test_orchestrator_client
            .clone()
            .end_of_test_results(EndOfTestResultsRequest { exit_code })
            .await?;

        Ok(())
    }

    pub async fn prepare_for_local_execution(
        &self,
        stage: TestStage,
        target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        pre_create_dirs: Vec<DeclaredOutput>,
        required_local_resources: RequiredLocalResources,
    ) -> buck2_error::Result<PrepareForLocalExecutionResult> {
        let executable = TestExecutable {
            stage,
            target,
            cmd,
            env,
            pre_create_dirs,
        };

        let executable: buck2_test_proto::TestExecutable = executable
            .try_into()
            .buck_error_context("Invalid prepare_for_local_execution request")?;

        let request = buck2_test_proto::PrepareForLocalExecutionRequest {
            test_executable: Some(executable),
            required_local_resources: required_local_resources
                .resources
                .into_iter()
                .map(|r| r.into())
                .collect::<Vec<_>>(),
        };
        self.test_orchestrator_client
            .clone()
            .prepare_for_local_execution(request)
            .await?
            .into_inner()
            .try_into()
            .buck_error_context("Invalid `result`")
    }

    pub async fn attach_info_message(&self, message: String) -> buck2_error::Result<()> {
        self.test_orchestrator_client
            .clone()
            .attach_info_message(AttachInfoMessageRequest { message })
            .await?;
        Ok(())
    }

    pub async fn upload_to_cas(
        &self,
        local_path: String,
        ttl_seconds: i64,
        use_case: String,
    ) -> buck2_error::Result<CasDigest> {
        let UploadFileToCasResponse { digest } = self
            .test_orchestrator_client
            .clone()
            .upload_file_to_cas(UploadFileToCasRequest {
                local_path,
                ttl_config: Some(buck2_test_proto::TtlConfig {
                    ttl_seconds,
                    use_case,
                }),
            })
            .await?
            .into_inner();

        let digest = digest.internal_error("Missing `digest`")?;
        Ok(digest)
    }
}

/// Implement the `TestOrchestrator` trait for the gRPC client so that both the
/// gRPC-based and in-process paths can use `dyn TestOrchestrator` uniformly.
/// Each method delegates to the identically-named inherent method which handles
/// protobuf serialization and gRPC transport.
#[async_trait::async_trait]
impl TestOrchestrator for TestOrchestratorClient {
    async fn execute2(
        &self,
        stage: TestStage,
        target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        timeout: Duration,
        host_sharing_requirements: HostSharingRequirements,
        pre_create_dirs: Vec<DeclaredOutput>,
        executor_override: Option<ExecutorConfigOverride>,
        required_local_resources: RequiredLocalResources,
        disable_test_execution_caching: bool,
    ) -> buck2_error::Result<ExecuteResponse> {
        // Calls the inherent method (inherent methods take priority over trait methods).
        self.execute2(
            stage,
            target,
            cmd,
            env,
            timeout,
            host_sharing_requirements,
            pre_create_dirs,
            executor_override,
            required_local_resources,
            disable_test_execution_caching,
        )
        .await
    }

    async fn report_test_result(&self, r: TestResult) -> buck2_error::Result<()> {
        self.report_test_result(r).await
    }

    async fn report_tests_discovered(
        &self,
        target: ConfiguredTargetHandle,
        suite: String,
        name: Vec<String>,
    ) -> buck2_error::Result<()> {
        self.report_tests_discovered(target, suite, name).await
    }

    async fn report_test_session(
        &self,
        session_info: String,
        test_session_id: Option<String>,
    ) -> buck2_error::Result<()> {
        self.report_test_session(session_info, test_session_id)
            .await
    }

    async fn end_of_test_results(&self, exit_code: i32) -> buck2_error::Result<()> {
        self.end_of_test_results(exit_code).await
    }

    async fn prepare_for_local_execution(
        &self,
        stage: TestStage,
        target: ConfiguredTargetHandle,
        cmd: Vec<ArgValue>,
        env: BuckFixedSortedMap<String, ArgValue>,
        pre_create_dirs: Vec<DeclaredOutput>,
        required_local_resources: RequiredLocalResources,
    ) -> buck2_error::Result<PrepareForLocalExecutionResult> {
        self.prepare_for_local_execution(
            stage,
            target,
            cmd,
            env,
            pre_create_dirs,
            required_local_resources,
        )
        .await
    }

    async fn attach_info_message(&self, message: String) -> buck2_error::Result<()> {
        self.attach_info_message(message).await
    }

    async fn upload_to_cas(
        &self,
        local_path: String,
        ttl_seconds: i64,
        use_case: String,
    ) -> buck2_error::Result<CasDigest> {
        self.upload_to_cas(local_path, ttl_seconds, use_case).await
    }
}

struct TestOrchestratorService<T: TestOrchestrator> {
    inner: T,
}

#[async_trait::async_trait]
impl<T> test_orchestrator_server::TestOrchestrator for TestOrchestratorService<T>
where
    T: TestOrchestrator + Send + Sync + 'static,
{
    async fn execute2(
        &self,
        request: tonic::Request<buck2_test_proto::ExecuteRequest2>,
    ) -> Result<tonic::Response<ExecuteResponse2>, tonic::Status> {
        to_tonic(async move {
            let ExecuteRequest2 {
                test_executable,
                timeout,
                host_sharing_requirements,
                executor_override,
                required_local_resources,
                disable_test_execution_caching,
            } = request
                .into_inner()
                .try_into()
                .buck_error_context("Invalid execute2 request")?;

            let TestExecutable {
                stage,
                target,
                cmd,
                env,
                pre_create_dirs,
            } = test_executable;

            let response = self
                .inner
                .execute2(
                    stage,
                    target,
                    cmd,
                    env,
                    timeout,
                    host_sharing_requirements,
                    pre_create_dirs,
                    executor_override,
                    required_local_resources,
                    disable_test_execution_caching,
                )
                .await
                .buck_error_context("Execution failed")?;

            let response = match response {
                ExecuteResponse::Result(r) => {
                    buck2_test_proto::execute_response2::Response::Result(
                        r.try_into()
                            .buck_error_context("Failed to serialize result")?,
                    )
                }
                ExecuteResponse::Cancelled(reason) => {
                    let reason = if let Some(reason) = reason {
                        match reason {
                            crate::data::CancellationReason::NotSpecified => {
                                Some(buck2_test_proto::CancellationReason::NotSpecified.into())
                            }
                            crate::data::CancellationReason::ReQueueTimeout => {
                                Some(buck2_test_proto::CancellationReason::ReQueueTimeout.into())
                            }
                        }
                    } else {
                        None
                    };
                    buck2_test_proto::execute_response2::Response::Cancelled(
                        buck2_test_proto::Cancelled { reason },
                    )
                }
            };

            Ok(ExecuteResponse2 {
                response: Some(response),
            })
        })
        .await
    }

    async fn end_of_test_results(
        &self,
        req: tonic::Request<EndOfTestResultsRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        to_tonic(async move {
            let EndOfTestResultsRequest { exit_code } = req.into_inner();

            self.inner
                .end_of_test_results(exit_code)
                .await
                .buck_error_context("Failed to report end-of-tests")?;

            Ok(Empty {})
        })
        .await
    }

    async fn report_test_result(
        &self,
        request: tonic::Request<ReportTestResultRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        to_tonic(async move {
            let ReportTestResultRequest { result } = request.into_inner();

            let result = result
                .internal_error("Missing `result`")?
                .try_into()
                .buck_error_context("Invalid `result`")?;

            self.inner
                .report_test_result(result)
                .await
                .buck_error_context("Failed to report end-of-tests")?;

            Ok(Empty {})
        })
        .await
    }

    async fn report_tests_discovered(
        &self,
        request: tonic::Request<ReportTestsDiscoveredRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        to_tonic(async move {
            let ReportTestsDiscoveredRequest { target, testing } = request.into_inner();

            let target = target
                .internal_error("Missing `target`")?
                .try_into()
                .buck_error_context("Invalid `target`")?;

            let Testing {
                suite, testcases, ..
            } = testing.internal_error("Missing `testing`")?;

            self.inner
                .report_tests_discovered(target, suite, testcases)
                .await
                .buck_error_context("Failed to report end-of-tests")?;

            Ok(Empty {})
        })
        .await
    }

    async fn report_test_session(
        &self,
        request: tonic::Request<ReportTestSessionRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        to_tonic(async move {
            let ReportTestSessionRequest {
                session_info,
                test_session_id,
            } = request.into_inner();

            self.inner
                .report_test_session(session_info, test_session_id)
                .await
                .buck_error_context("Failed to report test session summary")?;

            Ok(Empty {})
        })
        .await
    }

    async fn prepare_for_local_execution(
        &self,
        request: tonic::Request<buck2_test_proto::PrepareForLocalExecutionRequest>,
    ) -> Result<tonic::Response<PrepareForLocalExecutionResponse>, tonic::Status> {
        to_tonic(async move {
            let buck2_test_proto::PrepareForLocalExecutionRequest {
                test_executable,
                required_local_resources,
            } = request.into_inner();
            let resources = RequiredLocalResources {
                resources: required_local_resources
                    .into_iter()
                    .map(|r| r.into())
                    .collect::<Vec<_>>(),
            };

            let TestExecutable {
                stage,
                target,
                cmd,
                env,
                pre_create_dirs,
            } = test_executable
                .internal_error("Missing `test_executable`")?
                .try_into()
                .buck_error_context("Invalid `test_executable`")
                .buck_error_context("Invalid prepare_for_local_execution request")?;

            let result = self
                .inner
                .prepare_for_local_execution(stage, target, cmd, env, pre_create_dirs, resources)
                .await
                .buck_error_context("Prepare for local execution failed")?;

            result
                .try_into()
                .buck_error_context("Failed to serialize result")
        })
        .await
    }

    async fn attach_info_message(
        &self,
        request: tonic::Request<AttachInfoMessageRequest>,
    ) -> Result<tonic::Response<Empty>, tonic::Status> {
        to_tonic(async move {
            let AttachInfoMessageRequest { message } = request.into_inner();

            self.inner
                .attach_info_message(message)
                .await
                .buck_error_context("Failed to attach info messages")?;

            Ok(Empty {})
        })
        .await
    }

    async fn upload_file_to_cas(
        &self,
        request: tonic::Request<UploadFileToCasRequest>,
    ) -> Result<tonic::Response<UploadFileToCasResponse>, tonic::Status> {
        to_tonic(async move {
            let UploadFileToCasRequest {
                local_path,
                ttl_config,
            } = request.into_inner();

            let ttl_config = ttl_config.internal_error("Missing `ttl_config`")?;

            let digest = self
                .inner
                .upload_to_cas(local_path, ttl_config.ttl_seconds, ttl_config.use_case)
                .await
                .buck_error_context("Failed to upload file to CAS")?;

            Ok(UploadFileToCasResponse {
                digest: Some(digest),
            })
        })
        .await
    }
}

struct DownwardApiService<T: DownwardApi> {
    inner: T,
}

#[async_trait::async_trait]
impl<T> downward_api_server::DownwardApi for DownwardApiService<T>
where
    T: DownwardApi + Send + Sync + 'static,
{
    async fn console(
        &self,
        request: tonic::Request<ConsoleRequest>,
    ) -> Result<tonic::Response<buck2_downward_api_proto::Empty>, tonic::Status> {
        to_tonic(async move {
            let ConsoleRequest { level, message } = request.into_inner();

            let level = level
                .internal_error("Missing `level`")?
                .try_into()
                .buck_error_context("Invalid `level`")?;

            self.inner
                .console(level, message)
                .await
                .buck_error_context("Failed to console")?;

            Ok(buck2_downward_api_proto::Empty {})
        })
        .await
    }

    async fn log(
        &self,
        request: tonic::Request<LogRequest>,
    ) -> Result<tonic::Response<buck2_downward_api_proto::Empty>, tonic::Status> {
        to_tonic(async move {
            let LogRequest { level, message } = request.into_inner();

            let level = level
                .internal_error("Missing `level`")?
                .try_into()
                .buck_error_context("Invalid `level`")?;

            self.inner
                .log(level, message)
                .await
                .buck_error_context("Failed to log")?;

            Ok(buck2_downward_api_proto::Empty {})
        })
        .await
    }

    async fn external_event(
        &self,
        request: tonic::Request<ExternalEventRequest>,
    ) -> Result<tonic::Response<buck2_downward_api_proto::Empty>, tonic::Status> {
        to_tonic(async move {
            let ExternalEventRequest { event } = request.into_inner();

            let event = event
                .internal_error("Missing `event`")?
                .try_into()
                .buck_error_context("Invalid `event`")?;

            self.inner
                .external(event)
                .await
                .buck_error_context("Failed to deliver event")?;

            Ok(buck2_downward_api_proto::Empty {})
        })
        .await
    }
}

pub fn spawn_orchestrator_server<I, O, D>(
    io: I,
    orchestrator: O,
    downward_api: D,
    dispatcher: EventDispatcher,
) -> ServerHandle
where
    I: AsyncRead + AsyncWrite + Send + Unpin + 'static + tonic::transport::server::Connected,
    O: TestOrchestrator + Send + Sync + 'static,
    D: DownwardApi + Send + Sync + 'static,
{
    let router = buck2_grpc::server_builder()
        .layer(EventDispatcherLayer::new(dispatcher))
        .add_service(
            test_orchestrator_server::TestOrchestratorServer::new(TestOrchestratorService {
                inner: orchestrator,
            })
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX),
        )
        .add_service(
            downward_api_server::DownwardApiServer::new(DownwardApiService {
                inner: downward_api,
            })
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX),
        );

    spawn_oneshot(io, router)
}

/// Used to wrap the Tonic server so that the spawned server has its EventDispatcher set.
#[derive(Clone, Dupe)]
pub struct EventDispatcherLayer {
    dispatcher: EventDispatcher,
}

impl EventDispatcherLayer {
    pub fn new(dispatcher: EventDispatcher) -> Self {
        EventDispatcherLayer { dispatcher }
    }
}

impl<S> Layer<S> for EventDispatcherLayer {
    type Service = DispatcherService<S>;

    fn layer(&self, service: S) -> Self::Service {
        DispatcherService {
            service,
            dispatcher: self.dispatcher.dupe(),
        }
    }
}

#[derive(Clone)]
pub struct DispatcherService<S> {
    service: S,
    dispatcher: EventDispatcher,
}

impl<T, Request> tower_service::Service<Request> for DispatcherService<T>
where
    T: tower_service::Service<Request>,
    T::Future: 'static + Send,
    T::Error: 'static,
    T::Response: 'static,
{
    type Response = T::Response;
    type Error = T::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        // Call the inner service and get a future that resolves to the response
        let fut = self.service.call(req);

        with_dispatcher_async(self.dispatcher.dupe(), fut).boxed()
    }
}
