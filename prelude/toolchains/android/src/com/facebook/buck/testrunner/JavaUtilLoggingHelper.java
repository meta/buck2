/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner;

import com.facebook.infer.annotation.Nullsafe;
import java.io.ByteArrayOutputStream;
import java.io.OutputStream;
import java.util.logging.Formatter;
import java.util.logging.Handler;
import java.util.logging.Level;
import java.util.logging.LogManager;
import java.util.logging.Logger;
import java.util.logging.StreamHandler;
import org.jspecify.annotations.Nullable;

/** Helper class for Java Util Logging functionality used in test runners. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavaUtilLoggingHelper {

  /** Container class for log handlers. */
  public static class LogHandlers {
    private final @Nullable Handler stdOutHandler;
    private final @Nullable Handler stdErrHandler;

    public LogHandlers(@Nullable Handler stdOutHandler, @Nullable Handler stdErrHandler) {
      this.stdOutHandler = stdOutHandler;
      this.stdErrHandler = stdErrHandler;
    }

    public @Nullable Handler getStdOutHandler() {
      return stdOutHandler;
    }

    public @Nullable Handler getStdErrHandler() {
      return stdErrHandler;
    }
  }

  /**
   * Sets up logging handlers for capturing log output.
   *
   * @param julLogBytes ByteArrayOutputStream to capture standard log output
   * @param julErrLogBytes ByteArrayOutputStream to capture error log output
   * @param stdOutLogLevel Level for standard output logging
   * @param stdErrLogLevel Level for error output logging
   * @return A LogHandlers object containing the created handlers
   */
  public static LogHandlers setupLogging(
      ByteArrayOutputStream julLogBytes,
      ByteArrayOutputStream julErrLogBytes,
      Level stdOutLogLevel,
      Level stdErrLogLevel) {

    // Listen to any java.util.logging messages reported by the test and write them to
    // julLogBytes / julErrLogBytes.
    Logger rootLogger = LogManager.getLogManager().getLogger("");

    if (rootLogger != null) {
      rootLogger.setLevel(Level.FINE);
    }

    JulLogFormatter formatter = new JulLogFormatter();
    Handler julLogHandler = addStreamHandler(rootLogger, julLogBytes, formatter, stdOutLogLevel);
    Handler julErrLogHandler =
        addStreamHandler(rootLogger, julErrLogBytes, formatter, stdErrLogLevel);

    return new LogHandlers(julLogHandler, julErrLogHandler);
  }

  /**
   * Adds a stream handler to the logger.
   *
   * @param rootLogger The logger to add the handler to
   * @param stream The output stream to write to
   * @param formatter The formatter to use
   * @param level The log level
   * @return The created handler
   */
  public static @Nullable Handler addStreamHandler(
      @Nullable Logger rootLogger, OutputStream stream, Formatter formatter, Level level) {
    Handler result;
    if (rootLogger != null) {
      result = new StreamHandler(stream, formatter);
      result.setLevel(level);
      rootLogger.addHandler(result);
    } else {
      result = null;
    }
    return result;
  }

  /**
   * Flushes and removes a log handler from the logger.
   *
   * @param rootLogger The logger to remove the handler from
   * @param handler The handler to flush and remove
   */
  public static void flushAndRemoveLogHandler(
      @Nullable Logger rootLogger, @Nullable Handler handler) {
    if (handler != null) {
      handler.flush();
    }
    if (rootLogger != null && handler != null) {
      rootLogger.removeHandler(handler);
    }
  }

  /**
   * Cleans up logging handlers.
   *
   * @param handlers The LogHandlers object containing the handlers to clean up
   */
  public static void cleanupLogging(LogHandlers handlers) {
    Logger rootLogger = LogManager.getLogManager().getLogger("");

    flushAndRemoveLogHandler(rootLogger, handlers.getStdOutHandler());
    flushAndRemoveLogHandler(rootLogger, handlers.getStdErrHandler());
  }
}
