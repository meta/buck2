/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi;

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import java.util.Map;
import javax.tools.Diagnostic;
import org.jspecify.annotations.Nullable;

/** Utilities method for {@link AbiGenerationMode} */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class AbiGenerationModeUtils {

  private static final Map<AbiGenerationMode, Diagnostic.Kind> DIAGNOSTICS_MAP =
      ImmutableMap.of(
          AbiGenerationMode.MIGRATING_TO_SOURCE_ONLY, Diagnostic.Kind.WARNING,
          AbiGenerationMode.SOURCE_ONLY, Diagnostic.Kind.ERROR);

  private AbiGenerationModeUtils() {}

  public static boolean checkForSourceOnlyAbiCompatibility(AbiGenerationMode abiGenerationMode) {
    return DIAGNOSTICS_MAP.containsKey(abiGenerationMode);
  }

  public static Diagnostic.@Nullable Kind getDiagnosticKindForSourceOnlyAbiCompatibility(
      AbiGenerationMode abiGenerationMode) {
    return DIAGNOSTICS_MAP.get(abiGenerationMode);
  }

  public static boolean isNotClassAbi(AbiGenerationMode abiGenerationMode) {
    return abiGenerationMode != AbiGenerationMode.CLASS;
  }

  public static boolean isSourceAbi(AbiGenerationMode abiGenerationMode) {
    return abiGenerationMode == AbiGenerationMode.SOURCE;
  }

  public static boolean isSourceOnlyAbi(AbiGenerationMode abiGenerationMode) {
    return abiGenerationMode == AbiGenerationMode.SOURCE_ONLY;
  }

  public static boolean usesDependencies(AbiGenerationMode abiGenerationMode) {
    return abiGenerationMode != AbiGenerationMode.SOURCE_ONLY;
  }
}
