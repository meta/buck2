/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd;

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.jvm.cd.workertool.MainUtils;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.IsolatedStepsRunner;
import com.facebook.buck.util.ClassLoaderCache;
import com.facebook.buck.util.Console;
import com.facebook.buck.util.Verbosity;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.io.Closeable;
import java.io.IOException;
import java.io.OutputStream;
import java.util.List;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

/**
 * Holds state needed to execute compilation commands. Can be used for a single command or stored
 * and re-used by a worker tool.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CompilerDaemonRunner implements Closeable {
  private final OutputStream eventsOutputStream;
  private final ClassLoaderCache classLoaderCache;
  private final Console console;

  private static final List<String> COMPILER_ERRORS =
      List.of(
          "Failed to execute isolated step <kotlinc>", "Failed to execute isolated step <javac>");

  static {
    // On macOS, this will suppress the Dock icon if any libraries try to use AWT somewhere.
    // We've seen this occasionally occur with kotlinc
    System.setProperty("apple.awt.UIElement", "true");
    // To prevent "Can't connect to X11 window server" errors seen on some linux hosts.
    System.setProperty("java.awt.headless", "true");
  }

  public CompilerDaemonRunner(OutputStream eventsOutputStream, Console console) {
    this.eventsOutputStream = eventsOutputStream;
    this.classLoaderCache = new ClassLoaderCache();
    this.console = console;
  }

  @Override
  public void close() throws IOException {
    eventsOutputStream.close();
    classLoaderCache.close();
  }

  private class CommandExecutionContext implements Closeable {
    IsolatedExecutionContext executionContext;

    public CommandExecutionContext(JvmCDCommand command) {
      this.executionContext =
          IsolatedExecutionContext.of(
              classLoaderCache, console, command.getBuildCommand().getRuleCellRoot());
    }

    @Override
    public void close() throws IOException {
      executionContext.close();
    }
  }

  /** Execute a single build command */
  public StepExecutionResult execute(JvmCDCommand command) throws IOException {
    try (CommandExecutionContext context = new CommandExecutionContext(command)) {
      ImmutableList<IsolatedStep> steps = command.getBuildCommand().getSteps();
      return IsolatedStepsRunner.executeWithDefaultExceptionHandling(
          steps, context.executionContext);
    }
  }

  /** Create a new runner, execute a single build command, close it and return */
  public static void run(JvmCDCommand command) throws IOException {
    Verbosity verbosity = getVerbosityForLevel(command.getLoggingLevel());
    Console console = new Console(verbosity, System.out, System.err);

    Thread.setDefaultUncaughtExceptionHandler(
        (t, e) -> MainUtils.handleExceptionAndTerminate(t, console, e));

    if (verbosity.shouldPrintStandardInformation()) {
      Executors.newSingleThreadScheduledExecutor()
          .scheduleAtFixedRate(MainUtils::logCurrentCDState, 1, 10, TimeUnit.SECONDS);
    }
    try (CompilerDaemonRunner runner =
        new CompilerDaemonRunner(OutputStream.nullOutputStream(), console)) {
      StepExecutionResult stepExecutionResult = runner.execute(command);

      if (!stepExecutionResult.isSuccess()) {
        String errorMessage = stepExecutionResult.getErrorMessage();
        System.err.println(errorMessage);
        // if there is a compiling error, we don't want to print buck stack trace
        if (!CompilerDaemonRunner.isCompilerError(errorMessage)) {
          stepExecutionResult.getCause().ifPresent(Throwable::printStackTrace);
        }
        throw new RuntimeException(
            "Compiler Daemon failed to execute command. Exit code: "
                + stepExecutionResult.getExitCode());
      }
    }
  }

  public static boolean isCompilerError(String errorMessage) {
    return COMPILER_ERRORS.stream().anyMatch(errorMessage::contains);
  }

  private static Verbosity getVerbosityForLevel(int loggingLevel) {
    if (loggingLevel >= 8) {
      return Verbosity.ALL;
    } else if (loggingLevel >= 5) {
      return Verbosity.COMMANDS_AND_OUTPUT;
    } else if (loggingLevel >= 4) {
      return Verbosity.COMMANDS_AND_SPECIAL_OUTPUT;
    } else if (loggingLevel >= 3) {
      return Verbosity.COMMANDS;
    } else if (loggingLevel >= 2) {
      return Verbosity.BINARY_OUTPUTS;
    } else if (loggingLevel >= 1) {
      return Verbosity.STANDARD_INFORMATION;
    } else {
      return Verbosity.SILENT;
    }
  }
}
