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
import com.google.common.base.CharMatcher;
import java.util.function.Function;

/** Utils for escaping bash */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BashEscaper {
  public static final CharMatcher BASH_SPECIAL_CHARS =
      CharMatcher.anyOf("<>|!?*[]$\\(){}\"'`&;=").or(CharMatcher.whitespace());

  public static final Function<String, String> BASH_ESCAPER =
      EscaperUtils.escaper(Quoter.SINGLE, BASH_SPECIAL_CHARS);
}
