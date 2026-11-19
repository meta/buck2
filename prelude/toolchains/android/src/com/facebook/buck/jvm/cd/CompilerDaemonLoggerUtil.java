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

import com.facebook.buck.core.util.log.Logger;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Objects;
import java.util.UUID;
import java.util.logging.ConsoleHandler;
import java.util.logging.FileHandler;
import java.util.logging.Handler;
import java.util.logging.Level;
import java.util.logging.SimpleFormatter;

/** Utility class for setting up default logger for compiler daemon worker tool. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CompilerDaemonLoggerUtil {
  static {
    // set java.util.logging (JUL) simple formatter to 1 liner.
    System.setProperty(
        "java.util.logging.SimpleFormatter.format",
        "[%1$tY-%1$tm-%1$td %1$tH:%1$tM:%1$tS] [%4$s] %3$s - %5$s%6$s%n");
  }

  private static final Level DEFALUT_LOG_LEVEL = Level.INFO;

  public static void setDefaultLogger(String logFilePrefix, String logPath) throws IOException {
    setDefaultLogger(logFilePrefix, logPath, DEFALUT_LOG_LEVEL);
  }

  /**
   * Sets default logger for compiler daemon worker tool.
   *
   * @param logFilePrefix prefix for log file name
   * @param logPath path to store log files
   * @param logLevel log level
   * @throws IOException if failed to create log file
   */
  public static void setDefaultLogger(String logFilePrefix, String logPath, Level logLevel)
      throws IOException {
    String logFileName =
        String.format(
            "%s/%s_%s_%s.log",
            logPath,
            logFilePrefix,
            new SimpleDateFormat("yyyyMMdd_HHmmss_SSS").format(new Date().getTime()),
            UUID.randomUUID().toString().substring(0, 8));
    Files.createDirectories(Objects.requireNonNull(Paths.get(logFileName).getParent()));
    FileHandler fileHandler = new FileHandler(logFileName);
    fileHandler.setFormatter(new SimpleFormatter());
    fileHandler.setLevel(logLevel);
    Logger.get("").addHandler(fileHandler);
  }

  /**
   * Sets console handler log level to given level.
   *
   * @param level
   */
  public static void setConsoleHandlerLogLevelTo(Level level) {
    for (Handler handler : Logger.get("").getHandlers()) {
      if (handler instanceof ConsoleHandler) {
        handler.setLevel(level);
        break;
      }
    }
  }
}
