/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import com.facebook.buck.jvm.java.fatjar.WindowsCreateProcessEscape;
import com.facebook.buck.util.environment.Platform;
import com.facebook.buck.util.escaper.BashEscaper;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.CharMatcher;
import java.util.function.Function;

@Nullsafe(Nullsafe.Mode.LOCAL)
public final class Escaper {

  /** Utility class: do not instantiate. */
  private Escaper() {}

  /** The quoting style to use when escaping. */
  public enum Quoter {
    SINGLE {
      @Override
      public String quote(String str) {
        return '\'' + str.replace("'", "'\\''") + '\'';
      }
    },
    DOUBLE {
      @Override
      public String quote(String str) {
        return '"' + str.replace("\"", "\\\"") + '"';
      }
    },
    DOUBLE_WINDOWS_JAVAC {
      @Override
      public String quote(String str) {
        return '"' + str.replace("\\", "\\\\") + '"';
      }
    },
    ;

    /**
     * @return the string with this quoting style applied.
     */
    public abstract String quote(String str);
  }

  /**
   * Escapes the special characters identified the {@link CharMatcher}, using single quotes.
   *
   * @param matcher identifies characters to be escaped
   * @param str string to quote
   * @return possibly quoted string
   */
  public static String escape(Quoter quoter, CharMatcher matcher, String str) {
    if (matcher.matchesAnyOf(str) || str.isEmpty()) {
      return quoter.quote(str);
    } else {
      return str;
    }
  }

  /**
   * @return a escaper function using the given quote style and escaping characters determined by
   *     the given matcher.
   */
  public static Function<String, String> escaper(Quoter quoter, CharMatcher matcher) {
    return input -> escape(quoter, matcher, input);
  }

  public static Function<String, String> javacEscaper() {
    if (Platform.detect() == Platform.WINDOWS) {
      return Escaper.escaper(
          Quoter.DOUBLE_WINDOWS_JAVAC, CharMatcher.anyOf("#'").or(CharMatcher.whitespace()));
    } else {
      return Escaper.escaper(
          Escaper.Quoter.DOUBLE, CharMatcher.anyOf("#\"'").or(CharMatcher.whitespace()));
    }
  }

  /**
   * Bash quoting {@link com.google.common.base.Function Function} which can be passed to {@link
   * com.google.common.collect.Iterables#transform Iterables.transform()}.
   */
  public static final Function<String, String> BASH_ESCAPER = BashEscaper.BASH_ESCAPER;

  /**
   * CreateProcess (Windows) quoting {@link com.google.common.base.Function Function} which can be
   * passed to {@link com.google.common.collect.Iterables#transform Iterables.transform()}.
   */
  public static final Function<String, String> CREATE_PROCESS_ESCAPER =
      WindowsCreateProcessEscape::quote;

  /**
   * Platform-aware shell quoting {@link com.google.common.base.Function Function} which can be
   * passed to {@link com.google.common.collect.Iterables#transform Iterables.transform()}
   * TODO(sdwilsh): get proper cmd.EXE escaping implemented on Windows
   */
  public static final Function<String, String> SHELL_ESCAPER =
      Platform.detect() == Platform.WINDOWS ? CREATE_PROCESS_ESCAPER : BASH_ESCAPER;

  /**
   * Quotes a string to be passed to the shell, if necessary. This works for the appropriate shell
   * regardless of the platform it is run on.
   *
   * @param str string to escape
   * @return possibly escaped string
   */
  public static String escapeAsShellString(String str) {
    return SHELL_ESCAPER.apply(str);
  }
}
