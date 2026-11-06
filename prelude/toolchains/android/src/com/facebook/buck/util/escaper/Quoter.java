/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.escaper;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Collections;

/** The quoting style to use when escaping. */
@Nullsafe(Nullsafe.Mode.LOCAL)
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
  DOUBLE_WINDOWS_ARGFILE {
    @Override
    public String quote(String str) {
      // Implementation of proper Windows quoting from:
      // https://docs.microsoft.com/en-us/archive/blogs/twistylittlepassagesallalike/everyone-quotes-command-line-arguments-the-wrong-way
      StringBuilder builder = new StringBuilder();
      builder.append('"');
      for (int i = 0; ; ++i) {
        int numBackslashes = 0;
        while (i < str.length() && str.charAt(i) == '\\') {
          ++i;
          ++numBackslashes;
        }

        if (i == str.length()) {
          builder.append(String.join("", Collections.nCopies(2 * numBackslashes, "\\")));
          break;
        }

        char ch = str.charAt(i);
        if (ch == '"') {
          builder.append(String.join("", Collections.nCopies(2 * numBackslashes + 1, "\\")));
          builder.append(ch);
        } else {
          builder.append(String.join("", Collections.nCopies(numBackslashes, "\\")));
          builder.append(ch);
        }
      }
      builder.append('"');
      return builder.toString();
    }
  };

  /**
   * @return the string with this quoting style applied.
   */
  public abstract String quote(String str);
}
