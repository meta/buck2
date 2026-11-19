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

import com.facebook.buck.cd.model.java.ResolvedJavacOptions;
import com.facebook.buck.jvm.cd.serialization.RelPathSerializer;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;

/** {@link ResolvedJavacPluginProperties} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ResolvedJavacPluginPropertiesSerializer {

  private ResolvedJavacPluginPropertiesSerializer() {}

  /**
   * Deserializes javacd model's {@link ResolvedJavacOptions.ResolvedJavacPluginProperties} into
   * {@link ResolvedJavacPluginProperties}.
   */
  public static ResolvedJavacPluginProperties deserialize(
      ResolvedJavacOptions.ResolvedJavacPluginProperties pluginProperties) {
    return new ResolvedJavacPluginProperties(
        pluginProperties.getCanReuseClassLoader(),
        pluginProperties.getDoesNotAffectAbi(),
        pluginProperties.getSupportsAbiGenerationFromSource(),
        pluginProperties.getRunsOnJavaOnly(),
        ImmutableSortedSet.copyOf(pluginProperties.getProcessorNamesList()),
        pluginProperties.getClasspathList().stream()
            .map(RelPathSerializer::deserialize)
            .collect(ImmutableList.toImmutableList()),
        ImmutableMap.of(),
        ImmutableList.copyOf(pluginProperties.getArgumentsList()));
  }
}
