/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.manifest;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/**
 * Main entry point for executing {@link GenerateManifest#replaceApplicationIdPlaceholders(String,
 * boolean)} calls.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ReplaceApplicationIdPlaceholdersExecutableMain {
  @Option(name = "--manifest", required = true)
  private String manifest;

  @Option(name = "--output", required = true)
  private String output;

  @Option(name = "--sanity-check-placeholders")
  private boolean runSanityCheck;

  public static void main(String[] args) throws IOException {
    System.exit(runMain(args));
  }

  static int runMain(String[] args) throws IOException {
    ReplaceApplicationIdPlaceholdersExecutableMain main =
        new ReplaceApplicationIdPlaceholdersExecutableMain();
    CmdLineParser parser = new CmdLineParser(main);
    try {
      parser.parseArgument(args);
      main.execute();
      return 0;
    } catch (CmdLineException e) {
      System.err.println(String.valueOf(e.getMessage()));
      parser.printUsage(System.err);
      return 1;
    }
  }

  private void execute() throws IOException {
    String currentManifest = Files.readString(Paths.get(manifest));
    String updatedManifest =
        GenerateManifest.replaceApplicationIdPlaceholders(currentManifest, runSanityCheck);
    Files.writeString(Paths.get(output), updatedManifest);
  }
}
