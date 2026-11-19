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

import com.facebook.buck.cd.model.java.OutputPathsValue;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.CompilerOutputPathsValue;
import com.facebook.infer.annotation.Nullsafe;
import java.util.Optional;

/** {@link CompilerOutputPathsValue} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CompilerOutputPathsValueSerializer {

  private CompilerOutputPathsValueSerializer() {}

  /** Deserializes javacd model's {@link OutputPathsValue} into {@link CompilerOutputPathsValue}. */
  public static CompilerOutputPathsValue deserialize(OutputPathsValue outputPathsValue) {
    return deserialize(outputPathsValue, Optional.empty());
  }

  public static CompilerOutputPathsValue deserialize(
      OutputPathsValue outputPathsValue, Optional<RelPath> tmpDir) {
    return CompilerOutputPathsValue.of(
        outputPathsValue.getLibraryTargetFullyQualifiedName(),
        toCompilerOutputPaths(outputPathsValue.getLibraryPaths(), tmpDir),
        toCompilerOutputPaths(outputPathsValue.getSourceAbiPaths(), tmpDir),
        toCompilerOutputPaths(outputPathsValue.getSourceOnlyAbiPaths(), tmpDir));
  }

  private static CompilerOutputPaths toCompilerOutputPaths(
      OutputPathsValue.OutputPaths outputPaths, Optional<RelPath> tmpDir) {
    return CompilerOutputPathsSerializer.deserialize(outputPaths, tmpDir);
  }
}
