/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.remote_worker;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.facebook.buck.jvm.cd.BuildCommandStepsBuilder;
import com.facebook.buck.jvm.cd.CompilerDaemonRunner;
import com.facebook.buck.jvm.cd.JvmCDCommand;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.swift.remoteexecution.workertool.WorkerToolExecuteRequest;
import com.facebook.swift.remoteexecution.workertool.WorkerToolExecuteResponse;
import java.io.IOException;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import org.junit.Before;
import org.junit.Test;

public class WorkerThriftServiceTest {

  private CompilerDaemonRunner mockRunner;

  /** Simple test implementation of JvmCDCommand to avoid mocking */
  private static class TestJvmCDCommand implements JvmCDCommand {
    private final String actionId;
    private boolean postExecuteCalled = false;

    TestJvmCDCommand(String actionId) {
      this.actionId = actionId;
    }

    @Override
    public BuildCommandStepsBuilder getBuildCommand() {
      throw new UnsupportedOperationException("Not needed for these tests");
    }

    @Override
    public String getActionId() {
      return actionId;
    }

    @Override
    public int getLoggingLevel() {
      return 0;
    }

    @Override
    public void postExecute() throws IOException {
      postExecuteCalled = true;
    }

    public boolean wasPostExecuteCalled() {
      return postExecuteCalled;
    }
  }

  @Before
  public void setUp() {
    mockRunner = mock(CompilerDaemonRunner.class);
  }

  private WorkerToolExecuteRequest createRequest(List<String> argv, Map<String, String> env) {
    return new WorkerToolExecuteRequest.Builder().setArgv(argv).setEnvironmentVars(env).build();
  }

  @Test
  public void testExecuteCommand_successfulExecution_returnsZeroExitCodeAndCallsPostExecute()
      throws Exception {
    // Arrange
    TestJvmCDCommand command = new TestJvmCDCommand("test-action");
    WorkerThriftService service = new WorkerThriftService((args, env) -> command, mockRunner);

    WorkerToolExecuteRequest request =
        createRequest(
            List.of("--action-id", "test-action", "--command-file", "/tmp/cmd.json"),
            Map.of("BUCK_SCRATCH_PATH", "/tmp/scratch"));

    StepExecutionResult successResult = new StepExecutionResult(0, Optional.empty());
    when(mockRunner.execute(command)).thenReturn(successResult);

    // Act
    WorkerToolExecuteResponse response = service.executeCommand(request);

    // Assert
    assertEquals(0, response.getExitCode());
    assertTrue("postExecute should be called on success", command.wasPostExecuteCalled());
  }

  @Test
  public void testExecuteCommand_failedExecution_returnsErrorWithMessage() throws Exception {
    // Arrange
    TestJvmCDCommand command = new TestJvmCDCommand("fail-action");
    WorkerThriftService service = new WorkerThriftService((args, env) -> command, mockRunner);

    WorkerToolExecuteRequest request =
        createRequest(
            List.of("--action-id", "fail-action"), Map.of("BUCK_SCRATCH_PATH", "/tmp/scratch"));

    StepExecutionResult failResult =
        new StepExecutionResult(1, Optional.of("compilation error"), Optional.empty());
    when(mockRunner.execute(command)).thenReturn(failResult);

    // Act
    WorkerToolExecuteResponse response = service.executeCommand(request);

    // Assert
    assertEquals(1, response.getExitCode());
    assertTrue(response.getStderr().contains("compilation error"));
    assertFalse("postExecute should not be called on failure", command.wasPostExecuteCalled());
  }

  @Test
  public void testExecuteCommand_factoryThrowsException_returnsErrorResponse() throws Exception {
    // Arrange
    WorkerThriftService service =
        new WorkerThriftService(
            (args, env) -> {
              throw new IOException("Invalid command file");
            },
            mockRunner);

    WorkerToolExecuteRequest request = createRequest(List.of("--bad-arg"), Map.of());

    // Act
    WorkerToolExecuteResponse response = service.executeCommand(request);

    // Assert
    assertEquals(1, response.getExitCode());
    assertTrue(response.getStderr().contains("Invalid command file"));
    assertTrue(response.getStderr().contains("Unexpected exception while executing"));
  }

  @Test
  public void testExecuteCommand_passesConvertedArgsAndEnvToFactory() throws Exception {
    // Arrange
    String[][] capturedArgs = new String[1][];
    String[][] capturedEnvKeys = new String[1][];

    WorkerThriftService service =
        new WorkerThriftService(
            (args, env) -> {
              capturedArgs[0] = args;
              capturedEnvKeys[0] = env.keySet().toArray(new String[0]);
              return new TestJvmCDCommand("my-action");
            },
            mockRunner);

    WorkerToolExecuteRequest request =
        createRequest(
            List.of("--action-id", "my-action", "--command-file", "/path/to/cmd"),
            Map.of("BUCK_SCRATCH_PATH", "/scratch", "KEY", "VALUE"));

    StepExecutionResult successResult = new StepExecutionResult(0, Optional.empty());
    when(mockRunner.execute(any())).thenReturn(successResult);

    // Act
    service.executeCommand(request);

    // Assert
    assertEquals(4, capturedArgs[0].length);
    assertEquals("--action-id", capturedArgs[0][0]);
    assertEquals("my-action", capturedArgs[0][1]);
    assertEquals("--command-file", capturedArgs[0][2]);
    assertEquals("/path/to/cmd", capturedArgs[0][3]);
  }

  @Test
  public void testExecuteCommand_compilerError_doesNotIncludeStackTrace() throws Exception {
    // Arrange
    TestJvmCDCommand command = new TestJvmCDCommand("compile-fail");
    WorkerThriftService service = new WorkerThriftService((args, env) -> command, mockRunner);

    WorkerToolExecuteRequest request =
        createRequest(List.of("--action-id", "compile-fail"), Map.of("BUCK_SCRATCH_PATH", "/tmp"));

    // The cause message must contain the compiler error string because
    // StepExecutionResult.getErrorMessage() prioritizes cause.message over stderr
    String compilerErrorMsg = "Failed to execute isolated step <javac>";
    StepExecutionResult failResult =
        new StepExecutionResult(
            1, Optional.of(compilerErrorMsg), Optional.of(new Exception(compilerErrorMsg)));
    when(mockRunner.execute(command)).thenReturn(failResult);

    // Act
    WorkerToolExecuteResponse response = service.executeCommand(request);

    // Assert
    assertEquals(1, response.getExitCode());
    String stderr = response.getStderr();
    assertTrue(stderr.contains(compilerErrorMsg));
    // For compiler errors, the stack trace should NOT be appended
    assertFalse("Stack trace should not be included for compiler errors", stderr.contains("\tat "));
  }

  @Test
  public void testExecuteCommand_nonCompilerError_includesStackTrace() throws Exception {
    // Arrange
    TestJvmCDCommand command = new TestJvmCDCommand("other-fail");
    WorkerThriftService service = new WorkerThriftService((args, env) -> command, mockRunner);

    WorkerToolExecuteRequest request =
        createRequest(List.of("--action-id", "other-fail"), Map.of("BUCK_SCRATCH_PATH", "/tmp"));

    // Non-compiler error: the cause message does NOT contain compiler error strings
    Exception cause = new RuntimeException("unexpected NPE");
    StepExecutionResult failResult =
        new StepExecutionResult(1, Optional.of("unexpected NPE"), Optional.of(cause));
    when(mockRunner.execute(command)).thenReturn(failResult);

    // Act
    WorkerToolExecuteResponse response = service.executeCommand(request);

    // Assert
    assertEquals(1, response.getExitCode());
    String stderr = response.getStderr();
    // For non-compiler errors, the stack trace SHOULD be included
    assertTrue(stderr.contains("unexpected NPE"));
    assertTrue("Stack trace should be included for non-compiler errors", stderr.contains("\tat "));
  }

  @Test
  public void testClose_closesRunner() throws IOException {
    // Arrange
    WorkerThriftService service =
        new WorkerThriftService((args, env) -> new TestJvmCDCommand("unused"), mockRunner);

    // Act
    service.close();

    // Assert
    verify(mockRunner).close();
  }

  @Test
  public void testClose_handlesIOExceptionFromRunner() throws IOException {
    // Arrange
    doThrow(new IOException("close failed")).when(mockRunner).close();
    WorkerThriftService service =
        new WorkerThriftService((args, env) -> new TestJvmCDCommand("unused"), mockRunner);

    // Act - should not throw even though runner.close() throws
    service.close();

    // Assert - runner.close() was still called
    verify(mockRunner).close();
  }
}
