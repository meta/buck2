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
import org.jspecify.annotations.Nullable;

/**
 * An XML escaper that replaces invalid xml characters with valid ones. The behaviour is intended to
 * match that of guava's XmlEscapers
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class TestXmlEscaper {

  /**
   * @param str a string to perform XML escape over
   * @return the escaped string
   */
  @Nullable
  public final String escape(String /* Nullable */ str) {
    if (str == null) {
      return null;
    }
    StringBuilder sb = new StringBuilder(str.length());

    for (int i = 0; i < str.length(); i++) {
      sb.append(escapeChar(str.charAt(i)));
    }
    return sb.toString();
  }

  protected abstract String escapeChar(char c);

  public static final TestXmlEscaper CONTENT_ESCAPER = new ContentEscaper();
  public static final TestXmlEscaper ATTRIBUTE_ESCAPER = new AttributeEscaper();

  private static final char MIN_ASCII_CONTROL_CHAR = 0x00;
  private static final char MAX_ASCII_CONTROL_CHAR = 0x1F;

  private static final String UNICODE_REPLACE = Character.toString('\uFFFD');

  private static class ContentEscaper extends TestXmlEscaper {

    @Override
    protected String escapeChar(char c) {
      if (c > '\uFFFD' || c < Character.MIN_VALUE) {
        return UNICODE_REPLACE;
      }

      /*
       * Except for \n, \t, and \r, all ASCII control characters are replaced with the Unicode
       * replacement character.
       *
       */
      if (c >= MIN_ASCII_CONTROL_CHAR
          && c <= MAX_ASCII_CONTROL_CHAR
          && c != '\t'
          && c != '\n'
          && c != '\r') {
        return UNICODE_REPLACE;
      }

      if (c == '&') {
        return "&amp;";
      }
      if (c == '<') {
        return "&lt;";
      }
      if (c == '>') {
        return "&gt;";
      }

      return Character.toString(c);
    }
  }

  private static class AttributeEscaper extends ContentEscaper {

    @Override
    protected String escapeChar(char c) {
      String replaced = super.escapeChar(c);

      if (replaced.length() != 1 || replaced.charAt(0) != c) {
        return replaced;
      }

      if (c == '\'') {
        return "&apos;";
      }
      if (c == '"') {
        return "&quot;";
      }
      if (c == '\t') {
        return "&#x9;";
      }
      if (c == '\n') {
        return "&#xA;";
      }
      if (c == '\r') {
        return "&#xD;";
      }
      return Character.toString(c);
    }
  }
}
