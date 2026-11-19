/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableMultimap;
import com.google.common.collect.Multimap;
import java.nio.file.Path;
import java.util.Map;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ExopackageUtil {
  public static ImmutableMap<Path, Path> applyFilenameFormat(
      Map<String, Path> filesToHashes, Path deviceDir, String filenameFormat) {
    ImmutableMap.Builder<Path, Path> filesBuilder = ImmutableMap.builder();
    for (Map.Entry<String, Path> entry : filesToHashes.entrySet()) {
      filesBuilder.put(
          deviceDir.resolve(String.format(filenameFormat, entry.getKey())), entry.getValue());
    }
    return filesBuilder.build();
  }

  public static ImmutableMultimap<Path, Path> applyFilenameFormat(
      Multimap<String, Path> filesToHashes, Path deviceDir, String filenameFormat) {
    ImmutableMultimap.Builder<Path, Path> filesBuilder = ImmutableMultimap.builder();
    for (Map.Entry<String, Path> entry : filesToHashes.entries()) {
      filesBuilder.put(
          deviceDir.resolve(String.format(filenameFormat, entry.getKey())), entry.getValue());
    }
    return filesBuilder.build();
  }
}
