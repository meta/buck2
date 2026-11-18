/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex.desugarclasspath;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Main entry point for writing the desugar classpath view of one program jar. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DesugarClasspathViewExecutableMain {

  @Option(name = "--program-jar", required = true)
  private String programJar;

  /** A file that lists the classpath jars, one per line, in the format of D8's classpath file. */
  @Option(name = "--classpath-files", required = true)
  private String classpathFiles;

  @Option(name = "--output", required = true)
  private String output;

  @Option(name = "--duplicates-output", required = true)
  private String duplicatesOutput;

  public static void main(String[] args) throws IOException {
    DesugarClasspathViewExecutableMain main = new DesugarClasspathViewExecutableMain();
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
    DesugarClasspathView.write(
        Paths.get(programJar),
        Files.readAllLines(Paths.get(classpathFiles)).stream()
            .map(Paths::get)
            .collect(ImmutableList.toImmutableList()),
        Paths.get(output),
        Paths.get(duplicatesOutput));
  }
}
