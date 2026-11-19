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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.util.environment.EnvVariablesProvider;
import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.nio.file.Paths;

/** {@link AbsPath} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class AbsPathSerializer {
  private static final String EXPECT_RELATIVE_PATHS_ENV_VAR =
      "JAVACD_ABSOLUTE_PATHS_ARE_RELATIVE_TO_CWD";

  private static final boolean EXPECT_RELATIVE_PATHS =
      EnvVariablesProvider.getSystemEnv().get(EXPECT_RELATIVE_PATHS_ENV_VAR) != null;

  private AbsPathSerializer() {}

  /** Deserializes javacd model into {@link AbsPath}. */
  public static AbsPath deserialize(String absPath) {
    Path path = Paths.get(absPath);
    if (EXPECT_RELATIVE_PATHS) {
      RelPath relPath = RelPath.of(path);

      return AbsPath.of(Paths.get("").toAbsolutePath()).resolve(relPath);
    } else {
      return AbsPath.of(path);
    }
  }
}
