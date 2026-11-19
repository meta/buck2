/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.manifest;

import com.android.utils.ILogger;
import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedWriter;
import java.io.Closeable;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import org.jetbrains.annotations.Nullable;

/**
 * Logger for manifest preprocessing activities.
 *
 * <p>Always writes preprocessing logs to a file. If no path is specified, creates a temporary log
 * file in /tmp. Throws exceptions if logging fails to ensure build transparency.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PreprocessLogger implements Closeable {
  private final BufferedWriter writer;
  private final Path logPath;

  private PreprocessLogger(BufferedWriter writer, Path logPath) {
    this.writer = writer;
    this.logPath = logPath;
  }

  /**
   * Creates a PreprocessLogger that writes to the specified file, or a temp file if path is null.
   *
   * @param logPath Path to the log file, or null to create a temp file in /tmp
   * @param logger Logger for reporting the log file location
   * @return PreprocessLogger instance
   * @throws IOException if the log file cannot be created
   */
  public static PreprocessLogger create(@Nullable Path logPath, ILogger logger) throws IOException {
    Path actualLogPath;
    if (logPath != null) {
      actualLogPath = logPath;
    } else {
      // Create a temporary log file in /tmp with a unique name
      String tempFileName = String.format("manifest_preprocess_%d.log", System.currentTimeMillis());
      actualLogPath = Paths.get("/tmp", tempFileName);
    }

    BufferedWriter writer = new BufferedWriter(new FileWriter(actualLogPath.toFile()));
    logger.info("Manifest preprocessing log: %s", actualLogPath.toAbsolutePath());
    return new PreprocessLogger(writer, actualLogPath);
  }

  /**
   * Gets the path to the log file.
   *
   * @return Path to the log file
   */
  public Path getLogPath() {
    return logPath;
  }

  /**
   * Writes a formatted line to the log file.
   *
   * @param format Format string
   * @param args Format arguments
   * @throws IOException if writing fails
   */
  public void log(String format, Object... args) throws IOException {
    writer.write(String.format(format, args));
    writer.flush();
  }

  /**
   * Writes a line to the log file without formatting.
   *
   * @param message Message to write
   * @throws IOException if writing fails
   */
  public void log(String message) throws IOException {
    writer.write(message);
    writer.flush();
  }

  @Override
  public void close() throws IOException {
    writer.close();
  }
}
