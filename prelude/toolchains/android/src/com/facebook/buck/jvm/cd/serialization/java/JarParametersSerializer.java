/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.serialization.java;

import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.cd.serialization.RelPathSerializer;
import com.facebook.buck.jvm.java.JarParameters;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSortedSet;
import java.util.Optional;
import java.util.logging.Level;

/** {@link JarParameters} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JarParametersSerializer {

  private JarParametersSerializer() {}

  /**
   * Deserializes javacd model's {@link com.facebook.buck.cd.model.java.JarParameters} into {@link
   * JarParameters}.
   */
  public static JarParameters deserialize(
      com.facebook.buck.cd.model.java.JarParameters jarParameters) {
    Optional<String> mainClass =
        Optional.of(jarParameters.getMainClass()).filter(s -> !s.isEmpty());
    Optional<RelPath> manifestFile =
        Optional.of(jarParameters.getManifestFile())
            .filter(s -> !s.isEmpty())
            .map(RelPathSerializer::deserialize);

    return new JarParameters(
        jarParameters.getHashEntries(),
        jarParameters.getMergeManifests(),
        RelPathSerializer.deserialize(jarParameters.getJarPath()),
        RemoveClassesPatternsMatcherSerializer.deserialize(jarParameters.getRemoveEntryPredicate()),
        jarParameters.getEntriesToJarList().stream()
            .map(RelPathSerializer::deserialize)
            .collect(ImmutableSortedSet.toImmutableSortedSet(RelPath.comparator())),
        jarParameters.getOverrideEntriesToJarList().stream()
            .map(RelPathSerializer::deserialize)
            .collect(ImmutableSortedSet.toImmutableSortedSet(RelPath.comparator())),
        mainClass,
        manifestFile,
        Level.FINE);
  }
}
