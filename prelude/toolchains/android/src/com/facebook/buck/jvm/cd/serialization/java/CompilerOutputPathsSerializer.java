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
import com.facebook.buck.jvm.cd.serialization.RelPathSerializer;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.infer.annotation.Nullsafe;
import java.util.Optional;

/** {@link CompilerOutputPaths} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CompilerOutputPathsSerializer {

  private CompilerOutputPathsSerializer() {}

  /**
   * Deserializes javacd model's {@link OutputPathsValue.OutputPaths} into {@link
   * CompilerOutputPaths}.
   */
  public static CompilerOutputPaths deserialize(OutputPathsValue.OutputPaths outputPaths) {
    return deserialize(outputPaths, Optional.empty());
  }

  public static CompilerOutputPaths deserialize(
      OutputPathsValue.OutputPaths outputPaths, Optional<RelPath> tmpDir) {
    return new CompilerOutputPaths(
        tmpDir.get().resolveRel("__classes__"),
        toRelPath(outputPaths.getOutputJarDirPath()),
        toOptionalRelPath(outputPaths.getAbiJarPath()),
        toOptionalRelPath(outputPaths.getAnnotationPath())
            .orElseGet(() -> tmpDir.get().resolveRel("__gen__")),
        outputPaths.getPathToSourcesList().isEmpty()
            ? tmpDir.map(p -> p.resolveRel("__srcs__")).get()
            : toRelPath(outputPaths.getPathToSourcesList()),
        outputPaths.getWorkingDirectory().isEmpty()
            ? tmpDir.get()
            : toRelPath(outputPaths.getWorkingDirectory()),
        toOptionalRelPath(outputPaths.getOutputJarPath()),
        toOptionalRelPath(outputPaths.getCompilerClassToSourceMap()).orElse(null));
  }

  private static RelPath toRelPath(String relPath) {
    return RelPathSerializer.deserialize(relPath);
  }

  private static Optional<RelPath> toOptionalRelPath(String value) {
    return Optional.of(value)
        .filter(s -> !s.isEmpty())
        .map(CompilerOutputPathsSerializer::toRelPath);
  }
}
