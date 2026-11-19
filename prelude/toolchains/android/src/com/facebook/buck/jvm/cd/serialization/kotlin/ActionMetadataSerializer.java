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
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.Map;
import java.util.stream.Collectors;

/**
 * Marshalling between:
 *
 * <ul>
 *   <li>{@link ActionMetadata} (metadata provided by incremental actions, see: <a
 *       href="https://buck2.build/docs/rule_authors/incremental_actions/">...</a>), and
 *   <li>{@link com.facebook.buck.cd.model.kotlin.ActionMetadata} (part of the protocol buffer
 *       model).
 * </ul>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ActionMetadataSerializer {

  private ActionMetadataSerializer() {}

  /** Protocol buffer model to internal buck representation. */
  public static ActionMetadata deserialize(
      Path incrementalMetadataFilePath,
      com.facebook.buck.cd.model.kotlin.ActionMetadata actionMetadata) {
    Map<Path, String> previousDigest =
        actionMetadata.getPreviousMetadata().getDigestsList().stream()
            .collect(
                Collectors.toMap(
                    digest -> PathSerializer.deserialize(digest.getPath()),
                    com.facebook.buck.cd.model.kotlin.Digests::getDigest));
    Map<Path, String> currentDigest =
        actionMetadata.getCurrentMetadata().getDigestsList().stream()
            .collect(
                Collectors.toMap(
                    digest -> PathSerializer.deserialize(digest.getPath()),
                    com.facebook.buck.cd.model.kotlin.Digests::getDigest));

    return new ActionMetadata(incrementalMetadataFilePath, previousDigest, currentDigest);
  }
}
