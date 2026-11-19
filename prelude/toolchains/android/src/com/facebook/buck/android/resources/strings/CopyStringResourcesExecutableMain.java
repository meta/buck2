/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources.strings;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Entry point for copying string resources. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CopyStringResourcesExecutableMain {
  @Option(name = "--res-dirs", required = true)
  private String resDirsFileString;

  @Option(name = "--output", required = true)
  private String outputPathString;

  @Option(name = "--is-voltron")
  private boolean isVoltron;

  public static void main(String[] args) throws IOException {
    CopyStringResourcesExecutableMain main = new CopyStringResourcesExecutableMain();
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
    ImmutableList<Path> resDirs =
        Files.readAllLines(Paths.get(resDirsFileString)).stream()
            .map(Paths::get)
            .collect(ImmutableList.toImmutableList());
    Path outputPath = Paths.get(outputPathString);

    if (isVoltron) {
      StringResourcesUtils.copyVoltronStringResources(
          root, resDirs, ImmutableList.of(), outputPath);
    } else {
      StringResourcesUtils.copyResources(root, resDirs, outputPath);
    }

    System.exit(0);
  }
}
