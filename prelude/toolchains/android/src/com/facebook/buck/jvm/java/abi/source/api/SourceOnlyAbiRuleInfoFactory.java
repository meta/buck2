/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source.api;

import com.facebook.infer.annotation.Nullsafe;
import javax.lang.model.element.Element;
import javax.lang.model.util.Elements;
import javax.tools.JavaFileManager;
import org.jspecify.annotations.Nullable;

/** Used to create the SourceOnlyAbiRuleInfo for a target given a JavaFileManager. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface SourceOnlyAbiRuleInfoFactory {
  SourceOnlyAbiRuleInfo create(JavaFileManager fileManager);

  /** Opt in only when the actual reduced dependency classpath is available. */
  default boolean shouldValidateInheritedMemberTypes() {
    return false;
  }

  /** Provides information related to source-only abi support. */
  interface SourceOnlyAbiRuleInfo {

    @Nullable String getOwningTarget(Elements elements, Element element);

    boolean elementIsAvailableForSourceOnlyAbi(Elements elements, Element element);

    String getRuleName();

    boolean ruleIsRequiredForSourceOnlyAbi();
  }
}
