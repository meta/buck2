/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.build_config;

import com.facebook.buck.util.ThrowingPrintWriter;
import com.facebook.infer.annotation.Nullsafe;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Main entry point for generating BuildConfig.java. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class GenerateBuildConfigExecutableMain {
  @Option(name = "--source", required = true)
  private String source;

  @Option(name = "--java-package", required = true)
  private String javaPackage;

  @Option(name = "--use-constant-expressions", required = true)
  private String useConstantExpressions;

  @Option(name = "--default-values-file", required = true)
  private List<String> defaultValuesFiles = new ArrayList<>();

  @Option(name = "--values-file")
  private List<String> valuesFiles = new ArrayList<>();

  @Option(name = "--output", required = true)
  private String output;

  public static void main(String[] args) throws IOException {
    GenerateBuildConfigExecutableMain main = new GenerateBuildConfigExecutableMain();
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
    Path outputPath = Paths.get(output);

    BuildConfigFields fields = BuildConfigFields.of();
    for (String path : defaultValuesFiles) {
      fields = fields.putAll(readFields(path));
    }
    for (String path : valuesFiles) {
      fields = fields.putAll(readFields(path));
    }

    String java =
        BuildConfigs.generateBuildConfigDotJava(
            source, javaPackage, Boolean.parseBoolean(useConstantExpressions), fields);

    try (ThrowingPrintWriter writer =
        new ThrowingPrintWriter(new FileOutputStream(outputPath.toFile()))) {
      writer.printf(java);
    }

    System.exit(0);
  }

  private static BuildConfigFields readFields(String path) throws IOException {
    return BuildConfigFields.fromFieldDeclarations(Files.readAllLines(Paths.get(path)));
  }
}
