/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source;

import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory.SourceOnlyAbiRuleInfo;
import com.facebook.buck.util.liteinfersupport.Nullable;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.util.Trees;
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeElement;
import javax.lang.model.util.Elements;

/** Simulates the behavior of {@code javac}'s file manager for source-only ABI classpaths. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class FileManagerSimulator {
  private final Elements elements;
  private final Trees trees;
  private final SourceOnlyAbiRuleInfo ruleInfo;

  public FileManagerSimulator(Elements elements, Trees trees, SourceOnlyAbiRuleInfo ruleInfo) {
    this.elements = elements;
    this.trees = trees;
    this.ruleInfo = ruleInfo;
  }

  @Nullable
  public String getOwningTarget(Element element) {
    return ruleInfo.getOwningTarget(elements, element);
  }

  public boolean typeWillBeAvailable(TypeElement type) {
    return isCompiledInCurrentRun(type)
        || ruleInfo.elementIsAvailableForSourceOnlyAbi(elements, type);
  }

  public boolean isCompiledInCurrentRun(TypeElement typeElement) {
    return trees.getTree(typeElement) != null;
  }
}
