/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedReader;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Locale;
import java.util.Optional;
import javax.annotation.Nullable;
import javax.tools.Diagnostic;
import javax.tools.JavaFileObject;

/**
 * Take a {@link Diagnostic} and pretty print it, using the same formatting as you'd find in the
 * Oracle javac implementation. This ensures that no matter which compiler is being used, output to
 * the user is consistent.
 *
 * <p>Output is given in the following format, where "summary" is the first line of {@link
 * Diagnostic#getMessage(Locale)} and "Remainder of message" are all the other lines.
 *
 * <pre>
 * Path-to-file:line-number: kind-of-error: summary\n
 *
 * Context from code
 *      ^
 * Remainder of message
 * </pre>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DiagnosticPrettyPrinter {

  private DiagnosticPrettyPrinter() {
    // utility class
  }

  public static String format(Diagnostic<? extends JavaFileObject> diagnostic) {
    return format(diagnostic, Optional.empty());
  }

  public static String format(
      Diagnostic<? extends JavaFileObject> diagnostic, Optional<AbsPath> sourceRoot) {
    /*
    In BasicDiagnosticFormatter.class, the default format used is: "%f:%l:%_%p%L%m"
    This expands to:

    "filename":"line":" ""kind""lint category""format message"

    Format message is implemented in the BasicDiagnosticFormatter. This splits the message into
    lines and picks the first one as the summary (seems reasonable to do). If details are
    requested, then the remaining lines are added. Details are always requested.
    */

    StringBuilder builder = new StringBuilder();

    JavaFileObject source = diagnostic.getSource();
    if (source != null && !source.getName().isEmpty()) {
      String sourcePathString = source.getName();
      try {
        Path sourcePath = Paths.get(source.getName());
        if (sourceRoot.isPresent() && sourcePath.isAbsolute()) {
          sourcePathString = sourceRoot.get().getPath().relativize(sourcePath).toString();
        }
      } catch (Exception e) {
        // Do nothing. We'll just leave the path as is.
      }

      builder.append(sourcePathString).append(":").append(diagnostic.getLineNumber());
    }

    builder
        .append(": ")
        .append(diagnostic.getKind().toString().toLowerCase(Locale.US))
        .append(": ");

    String formattedMessage = diagnostic.getMessage(Locale.getDefault());
    String[] parts = formattedMessage.split(System.lineSeparator());
    if (parts.length == 0) {
      parts = new String[] {""};
    }

    // Use the first line as a summary. With the normal Oracle JSR199 javac, the remaining lines are
    // more detailed diagnostics, which we don't normally display. With ECJ, there's no additional
    // information anyway, so it doesn't matter that we discard the remaining output.
    builder.append(parts[0]).append(System.lineSeparator());

    appendContext(builder, diagnostic, source);

    // If there was additional information in the message, append it now.
    for (int i = 1; i < parts.length; i++) {
      builder.append(System.lineSeparator()).append(parts[i]);
    }

    return builder.toString();
  }

  private static void appendContext(
      StringBuilder builder,
      Diagnostic<? extends JavaFileObject> diagnostic,
      @Nullable JavaFileObject source) {

    if (source == null) {
      return;
    }

    Optional<String> line = getLine(source, diagnostic.getLineNumber());
    if (line.isPresent()) {
      builder.append(line.get()).append(System.lineSeparator());
      for (long i = 1; i < diagnostic.getColumnNumber(); i++) {
        builder.append(" ");
      }
      builder.append("^");
    }
  }

  private static Optional<String> getLine(JavaFileObject source, long line) {
    if (line < 0) {
      return Optional.empty();
    }

    String toReturn = null;
    try (BufferedReader reader = new BufferedReader(source.openReader(true))) {
      for (long i = 0; i < line; i++) {
        toReturn = reader.readLine();
      }
    } catch (IOException e) {
      // Do nothing. We're just trying to get some context, but we've failed.
      return Optional.empty();
    }

    return Optional.ofNullable(toReturn);
  }
}
