/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.serialization;

import com.facebook.buck.cd.model.common.RelPathMapEntry;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;
import java.util.List;
import java.util.Map;

/** {@link RelPath} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class RelPathSerializer {

  private RelPathSerializer() {}

  /** Deserializes list of {@link RelPathMapEntry} into a map of resources */
  public static ImmutableMap<RelPath, RelPath> toResourceMap(
      List<RelPathMapEntry> resourcesMapList) {
    ImmutableMap.Builder<RelPath, RelPath> builder =
        ImmutableMap.builderWithExpectedSize(resourcesMapList.size());
    for (RelPathMapEntry entry : resourcesMapList) {
      builder.put(
          RelPathSerializer.deserialize(entry.getKey()),
          RelPathSerializer.deserialize(entry.getValue()));
    }
    return builder.build();
  }

  /** Deserializes list of paths into a sorted set of {@link RelPath} */
  public static ImmutableSortedSet<RelPath> toSortedSetOfRelPath(List<String> list) {
    ImmutableSortedSet.Builder<RelPath> builder =
        ImmutableSortedSet.orderedBy(RelPath.comparator());
    for (String item : list) {
      builder.add(RelPathSerializer.deserialize(item));
    }
    return builder.build();
  }

  /** Deserializes list of paths into a map of {@link RelPath} to {@link RelPath} */
  public static ImmutableMap<RelPath, RelPath> toMap(Map<String, String> map) {
    return map.entrySet().stream()
        .filter(e -> !e.getValue().isEmpty())
        .collect(
            ImmutableMap.toImmutableMap(
                e -> RelPathSerializer.deserialize(e.getKey()),
                e -> RelPathSerializer.deserialize(e.getValue())));
  }

  /** Deserializes list of paths into a list of {@link RelPath} */
  public static ImmutableList<RelPath> toListOfRelPath(List<String> list) {
    return list.stream()
        .map(RelPathSerializer::deserialize)
        .collect(ImmutableList.toImmutableList());
  }

  /** Deserializes javacd model into {@link RelPath}. */
  public static RelPath deserialize(String relPath) {
    return RelPath.get(relPath);
  }
}
