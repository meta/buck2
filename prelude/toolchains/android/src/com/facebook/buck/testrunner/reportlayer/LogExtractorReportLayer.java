/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner.reportlayer;

import com.facebook.buck.testrunner.InstrumentationTestRunner;
import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.FileWriter;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Scanner;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** report layer to extract logs to separate tra by regex matching */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class LogExtractorReportLayer extends ReportLayer {

  public static final String ARG = "--log-extractor";

  /** config for each extractor */
  protected static class ExtractorConfig {
    private final String traName;
    private final String traDescription;
    private final String regex;

    public ExtractorConfig(String traName, String traDescription, String regex) {
      this.traName = traName;
      this.traDescription = traDescription;
      this.regex = regex;
    }

    public String getTraName() {
      return traName;
    }

    public String getTraDescription() {
      return traDescription;
    }

    public String getRegex() {
      return regex;
    }
  }

  private final List<ExtractorConfig> extractors = new ArrayList<ExtractorConfig>();

  public LogExtractorReportLayer(
      InstrumentationTestRunner runner, Map<String, String> logExtractors) {
    super(runner);
    for (Map.Entry<String, String> entry : logExtractors.entrySet()) {
      String name = entry.getKey();
      String regex = entry.getValue();
      this.extractors.add(new ExtractorConfig(name, name, regex));
    }
  }

  @Override
  public void initialize() {
    // nothing needed.
  }

  @Override
  public void report() {
    try {
      Path logcatMainBufferTraPath = this.runner.createPathForLogcatBuffer("main");
      String logcatMainBufferFile = Objects.requireNonNull(logcatMainBufferTraPath).toString();
      for (ExtractorConfig extractorConfig : extractors) {
        Path traPath =
            this.runner.createTRAPlainTextLog(
                extractorConfig.traName, extractorConfig.traDescription);
        Pattern pattern = Pattern.compile(extractorConfig.regex);

        FileWriter writer = new FileWriter(Objects.requireNonNull(traPath).toFile());
        Scanner scanner = new Scanner(new File(logcatMainBufferFile));
        while (scanner.hasNextLine()) {
          String line = scanner.nextLine();
          Matcher matcher = pattern.matcher(line);
          if (matcher.find()) {
            writer.write(line + "\n");
          }
        }
        scanner.close();
        writer.close();
      }
    } catch (Exception e) {
      e.printStackTrace(System.err);
      System.err.println("fail to extract log: " + e.getMessage());
    }
  }
}
