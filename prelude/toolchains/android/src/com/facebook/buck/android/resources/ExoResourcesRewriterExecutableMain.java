/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.hash.Hashing;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Main entry point for executing {@link ExoResourcesRewriter} calls. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ExoResourcesRewriterExecutableMain {
  @Option(name = "--original-r-dot-txt", required = true)
  private String originalRDotTxt;

  @Option(name = "--new-r-dot-txt", required = true)
  private String newRDotTxt;

  @Option(name = "--original-primary-apk-resources", required = true)
  private String originalPrimaryApkResources;

  @Option(name = "--new-primary-apk-resources", required = true)
  private String newPrimaryApkResources;

  @Option(name = "--exo-resources", required = true)
  private String exoResources;

  @Option(name = "--exo-resources-hash", required = true)
  private String exoResourcesHash;

  public static void main(String[] args) throws IOException {
    ExoResourcesRewriterExecutableMain main = new ExoResourcesRewriterExecutableMain();
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
    AbsPath root = AbsPath.of(Paths.get(".").normalize().toAbsolutePath());

    ExoResourcesRewriter.rewrite(
        root,
        RelPath.get(originalPrimaryApkResources),
        RelPath.get(originalRDotTxt),
        RelPath.get(newPrimaryApkResources),
        Paths.get(exoResources),
        RelPath.get(newRDotTxt));

    Files.writeString(
        Paths.get(exoResourcesHash),
        com.google.common.io.Files.hash(Paths.get(exoResources).toFile(), Hashing.sha1())
            .toString());
  }
}
