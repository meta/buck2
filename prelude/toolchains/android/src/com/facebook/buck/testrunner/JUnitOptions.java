/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner;

import com.facebook.buck.test.selectors.TestSelectorList;
import com.facebook.infer.annotation.Nullsafe;
import java.util.Optional;
import java.util.logging.Level;
import org.jspecify.annotations.Nullable;

/**
 * Model representing parsed execution arguments.
 *
 * <p>Parsed output level defined by the property {@code #STD_OUT_LOG_LEVEL_PROPERTY}.
 *
 * @see #STD_OUT_LOG_LEVEL_PROPERTY
 * @see #STD_ERR_LOG_LEVEL_PROPERTY
 * @see System#getProperty(String)
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JUnitOptions {

  static final String STD_OUT_LOG_LEVEL_PROPERTY = "com.facebook.buck.stdOutLogLevel";
  static final String STD_ERR_LOG_LEVEL_PROPERTY = "com.facebook.buck.stdErrLogLevel";

  private boolean dryRun;
  private boolean shouldExplainTestSelectors;
  private long defaultTestTimeoutMillis;
  private @Nullable TestSelectorList testSelectorList;
  private Optional<Level> stdOutLogLevel = Optional.empty();
  private Optional<Level> stdErrLogLevel = Optional.empty();

  private JUnitOptions() {}

  private JUnitOptions(JUnitOptions options) {
    this.dryRun = options.dryRun;
    this.shouldExplainTestSelectors = options.shouldExplainTestSelectors;
    this.defaultTestTimeoutMillis = options.defaultTestTimeoutMillis;
    this.testSelectorList = options.testSelectorList;
    this.stdOutLogLevel =
        Optional.ofNullable(System.getProperty(STD_OUT_LOG_LEVEL_PROPERTY)).map(Level::parse);
    this.stdErrLogLevel =
        Optional.ofNullable(System.getProperty(STD_ERR_LOG_LEVEL_PROPERTY)).map(Level::parse);
  }

  public @Nullable TestSelectorList getTestSelectorList() {
    return testSelectorList;
  }

  public boolean isDryRun() {
    return dryRun;
  }

  public boolean isShouldExplainTestSelectors() {
    return shouldExplainTestSelectors;
  }

  public long getDefaultTestTimeoutMillis() {
    return defaultTestTimeoutMillis;
  }

  public Optional<Level> getStdErrLogLevel() {
    return stdErrLogLevel;
  }

  public Optional<Level> getStdOutLogLevel() {
    return stdOutLogLevel;
  }

  /**
   * @return New instance of a builder for {@link JUnitOptions}
   */
  public static Builder builder() {
    return new Builder();
  }

  /** Builder class for {@link JUnitOptions} */
  public static class Builder {

    private JUnitOptions base = new JUnitOptions();

    public Builder testSelectorList(TestSelectorList testSelectorList) {
      base.testSelectorList = testSelectorList;
      return this;
    }

    public Builder dryRun(boolean dryRun) {
      base.dryRun = dryRun;
      return this;
    }

    public Builder shouldExplainTestSelectors(boolean shouldExplainTestSelectors) {
      base.shouldExplainTestSelectors = shouldExplainTestSelectors;
      return this;
    }

    public Builder defaultTestTimeoutMillis(long defaultTestTimeoutMillis) {
      base.defaultTestTimeoutMillis = defaultTestTimeoutMillis;
      return this;
    }

    public JUnitOptions build() {
      return new JUnitOptions(base);
    }
  }
}
