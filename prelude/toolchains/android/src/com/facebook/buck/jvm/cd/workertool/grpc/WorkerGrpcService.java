/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.workertool.grpc;

import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.cd.CompilerDaemonRunner;
import com.facebook.buck.jvm.cd.ErrorInterceptor;
import com.facebook.buck.jvm.cd.JvmCDCommand;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.worker.model.ExecuteCommand;
import com.facebook.buck.worker.model.ExecuteResponse;
import com.facebook.buck.worker.model.WorkerGrpc;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Throwables;
import com.google.common.collect.ImmutableMap;
import com.google.protobuf.ByteString;
import io.grpc.stub.StreamObserver;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.PrintStream;
import org.kohsuke.args4j.CmdLineException;

/** Worker Service that implements {@code workertool.v2.proto} */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class WorkerGrpcService extends WorkerGrpc.WorkerImplBase {
  private static final Logger LOG = Logger.get(WorkerGrpcService.class);

  /** Used to supply executable commands for a specific JvmCD server */
  public interface JvmCDCommandFactory {
    JvmCDCommand createCommand(String[] args, ImmutableMap<String, String> env)
        throws CmdLineException, IOException;
  }

  private final JvmCDCommandFactory commandFactory;
  private final CompilerDaemonRunner runner;

  public WorkerGrpcService(JvmCDCommandFactory commandFactory, CompilerDaemonRunner runner) {
    this.commandFactory = commandFactory;
    this.runner = runner;
  }

  @Override
  public void execute(ExecuteCommand request, StreamObserver<ExecuteResponse> responseObserver) {
    try {
      JvmCDCommand command = createCommand(request);
      StepExecutionResult result = runner.execute(command);
      ExecuteResponse response = handleCompletedExecution(command, result);
      responseObserver.onNext(response);
      responseObserver.onCompleted();
    } catch (Exception e) {
      handleException(responseObserver, "executing", e);
    }
  }

  private JvmCDCommand createCommand(ExecuteCommand request) throws IOException, CmdLineException {
    String[] argv =
        request.getArgvList().stream().map(ByteString::toStringUtf8).toArray(String[]::new);

    ImmutableMap.Builder<String, String> builder = ImmutableMap.builder();
    for (ExecuteCommand.EnvironmentEntry entry : request.getEnvList()) {
      builder.put(entry.getKey().toStringUtf8(), entry.getValue().toStringUtf8());
    }
    ImmutableMap<String, String> env = builder.build();
    LOG.info("Execute\nargs: %s\nenv: %s", String.join(" ", argv), env);

    return commandFactory.createCommand(argv, env);
  }

  private ExecuteResponse handleCompletedExecution(JvmCDCommand command, StepExecutionResult result)
      throws IOException {
    if (!result.isSuccess()) {

      String errorMessage = result.getErrorMessage();
      String stackTrace = "";
      // if there is a compiling error, we don't want to print buck stack trace
      if (!CompilerDaemonRunner.isCompilerError(errorMessage)) {
        ByteArrayOutputStream outputStream = new ByteArrayOutputStream();
        PrintStream ps = new PrintStream(outputStream);
        result.getCause().ifPresent(c -> c.printStackTrace(ps));
        stackTrace = outputStream.toString();
      }
      String error = String.join("\n", errorMessage, stackTrace);
      LOG.info("Command failed %s", command.getActionId());
      LOG.error(error);
      return ExecuteResponse.newBuilder()
          .setExitCode(result.getExitCode())
          .setStderr(java.util.Objects.requireNonNull(ErrorInterceptor.prettyPrint(error)))
          .build();
    }
    command.postExecute();
    LOG.info("Command succeeded %s", command.getActionId());
    // value of 0 is equivalent to unset in proto3 so this looks like an empty response in logs
    return ExecuteResponse.newBuilder().setExitCode(0).build();
  }

  private void handleException(StreamObserver<?> responseObserver, String context, Throwable e) {
    String message = String.format("Unexpected exception while %s", context);
    LOG.error(message);
    LOG.error(e);
    responseObserver.onError(
        io.grpc.Status.INTERNAL
            .withDescription(message)
            .augmentDescription(Throwables.getStackTraceAsString(e))
            .asException());
  }
}
