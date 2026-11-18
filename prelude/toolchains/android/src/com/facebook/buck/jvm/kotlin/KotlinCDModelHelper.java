/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableList;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

// TODO(navidq) Deserialize BuildKotlinCommand into our own data model and use it everywhere
@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlinCDModelHelper {
  private static final Pattern LANGUAGE_VERSION_REGEX =
      Pattern.compile("-language-version=\\d+\\.\\d+");

  public static String getLanguageVersion(ImmutableList<String> args) {
    for (String arg : args) {
      Matcher matcher = LANGUAGE_VERSION_REGEX.matcher(arg);
      if (matcher.matches()) {
        String[] splitArg = arg.split("=");
        Preconditions.checkState(splitArg.length == 2);
        return splitArg[1];
      }
    }
    return "1.9";
  }
}
