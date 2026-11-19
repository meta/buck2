/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.zipalign;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.io.CharStreams;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.Reader;
import java.util.Objects;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipAlign {

  private final String zipalignPath;

  private final String intermediateApkPath;

  private final String zipalignApkPath;

  public ZipAlign(String zipalignPath, String intermediateApkPath, String zipalignApkPath) {
    this.zipalignPath = zipalignPath;
    this.intermediateApkPath = intermediateApkPath;
    this.zipalignApkPath = zipalignApkPath;
  }

  public void run() throws InterruptedException, IOException {
    Process zipalignProcess =
        new ProcessBuilder()
            .command(zipalignPath, "-f", "4", intermediateApkPath, zipalignApkPath)
            .start();
    zipalignProcess.waitFor();
    if (zipalignProcess.exitValue() != 0) {
      String errorMessage;
      try (Reader reader =
          new InputStreamReader(Objects.requireNonNull(zipalignProcess.getErrorStream()))) {
        errorMessage = CharStreams.toString(reader);
      }
      if (errorMessage.contains("Unable to open")) {
        errorMessage =
            errorMessage.concat(
                "\n"
                    + "This issue is usually caused by having more than 2^^16 files in the APK. Try"
                    + " filtering out some resources, or follow D75775793 to only pack single"
                    + " preferred density resource as mitigation\n");
      }

      throw new RuntimeException("zipalign failed to process apk file:\n" + errorMessage);
    }
  }
}
