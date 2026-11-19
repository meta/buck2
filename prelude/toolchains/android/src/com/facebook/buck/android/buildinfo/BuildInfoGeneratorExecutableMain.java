/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.buildinfo;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Objects;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/**
 * Writes {@code <output-dir>/assets/BuildInfo.json} = {@code {"build_uuid": "<BUCK_BUILD_ID>"}} so
 * an installed build can be resolved back to the buck2 build that produced the APK (and from there
 * to its source commit). {@code BUCK_BUILD_ID} is the buck2 trace id that buck2 sets on every
 * action's environment.
 *
 * <p>The {@code apk_build} action invokes this tool, so the baked id refreshes when the APK content
 * changes and a no-op (cache-hit) rebuild keeps a prior, equally valid id.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class BuildInfoGeneratorExecutableMain {
  @Option(name = "--output-dir", required = true)
  private String outputDir;

  public static void main(String[] args) throws IOException {
    BuildInfoGeneratorExecutableMain main = new BuildInfoGeneratorExecutableMain();
    CmdLineParser parser = new CmdLineParser(main);
    try {
      parser.parseArgument(args);
      main.run();
      System.exit(0);
    } catch (CmdLineException e) {
      System.err.println(e.toString());
      parser.printUsage(System.err);
      System.exit(1);
    }
  }

  private void run() throws IOException {
    String buildId = Objects.requireNonNullElse(System.getenv("BUCK_BUILD_ID"), "");
    Path assetsDir = Files.createDirectories(Paths.get(outputDir).resolve("assets"));
    // BUCK_BUILD_ID is a buck2 trace id (a UUID), so it needs no JSON escaping.
    Files.writeString(assetsDir.resolve("BuildInfo.json"), "{\"build_uuid\": \"" + buildId + "\"}");
  }
}
