/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import com.facebook.infer.annotation.Nullsafe;
import org.jspecify.annotations.Nullable;

/**
 * Strategy interface for merging manifest attribute values. Implementations define how to compute
 * the final value when merging attributes from multiple manifests.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
@FunctionalInterface
public interface ManifestAttributeMerger {

  /**
   * Computes the merged value for a manifest attribute.
   *
   * @param existingValue The current value in the target manifest, or null if not present.
   * @param newValue The incoming value from the source manifest.
   * @return The merged value to use in the target manifest.
   */
  Object merge(@Nullable Object existingValue, Object newValue);
}
