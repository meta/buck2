/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.serialization.kotlin;

import com.facebook.buck.jvm.cd.serialization.PathSerializer;
import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.AbstractMap;
import java.util.Map;

/**
 * Marshalling between:
 *
 * <ul>
 *   <li>{@link Map.Entry<Path-String>} (metadata provided by incremental actions, see: <a
 *       href="https://buck2.build/docs/rule_authors/incremental_actions/">...</a>), and
 *   <li>{@link com.facebook.buck.cd.model.kotlin.Digests} (part of the protocol buffer model).
 * </ul>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DigestSerializer {

  private DigestSerializer() {}

  /** Protocol buffer model to internal buck representation. */
  public static Map.Entry<Path, String> deserialize(
      com.facebook.buck.cd.model.kotlin.Digests digests) {
    return new AbstractMap.SimpleEntry<>(
        PathSerializer.deserialize(digests.getPath()), digests.getDigest());
  }
}
