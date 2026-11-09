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
import com.facebook.buck.jvm.java.lang.model.MoreElements;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTask;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTaskProxyImpl;
import com.facebook.buck.jvm.java.testutil.compiler.CompilerTreeApiTest;
import com.sun.source.util.TaskListener;
import java.util.HashSet;
import java.util.Set;
import javax.lang.model.element.Element;
import javax.lang.model.element.PackageElement;
import javax.lang.model.util.Elements;
import javax.tools.Diagnostic;

class ValidatingTaskListenerFactory implements CompilerTreeApiTest.TaskListenerFactory {
  private final String ruleName;
  private final Set<String> targetsAvailableForSoruceOnly = new HashSet<>();
  private boolean requiredForSourceAbi = false;
  private boolean inheritedMemberTypesOnly = false;

  ValidatingTaskListenerFactory(String ruleName) {
    this.ruleName = ruleName;
  }

  public void setRuleIsRequiredForSourceAbi(boolean value) {
    this.requiredForSourceAbi = value;
  }

  public void setInheritedMemberTypesOnly(boolean value) {
    this.inheritedMemberTypesOnly = value;
  }

  public void addTargetAvailableForSourceOnlyAbi(String target) {
    targetsAvailableForSoruceOnly.add(target);
  }

  @Override
  public TaskListener newTaskListener(BuckJavacTask task) {
    return new ValidatingTaskListener(
        new BuckJavacTaskProxyImpl(task),
        new SourceOnlyAbiRuleInfo() {
          @Override
          public String getRuleName() {
            return ruleName;
          }

          @Override
          public boolean ruleIsRequiredForSourceOnlyAbi() {
            return requiredForSourceAbi;
          }

          @Override
          public boolean elementIsAvailableForSourceOnlyAbi(Elements elements, Element element) {
            if (targetsAvailableForSoruceOnly.contains(getOwningTarget(elements, element))) {
              return true;
            }
            return elements
                .getBinaryName(MoreElements.getTypeElement(element))
                .toString()
                .startsWith("java.");
          }

          @Override
          public String getOwningTarget(Elements elements, Element element) {
            PackageElement packageElement = MoreElements.getPackageElement(element);

            return "//"
                + packageElement.getQualifiedName().toString().replace('.', '/')
                + ":"
                + packageElement.getSimpleName();
          }
        },
        () -> false,
        Diagnostic.Kind.ERROR,
        inheritedMemberTypesOnly);
  }
}
