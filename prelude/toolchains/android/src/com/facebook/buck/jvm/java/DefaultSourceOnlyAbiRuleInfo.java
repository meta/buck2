/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory.SourceOnlyAbiRuleInfo;
import com.facebook.buck.jvm.java.lang.model.MoreElements;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import javax.lang.model.element.Element;
import javax.lang.model.util.Elements;
import javax.tools.JavaFileManager;
import javax.tools.JavaFileObject;
import javax.tools.StandardLocation;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
class DefaultSourceOnlyAbiRuleInfo implements SourceOnlyAbiRuleInfo {

  private final String fullyQualifiedBuildTargetName;
  private final boolean ruleIsRequiredForSourceOnlyAbi;

  private final Map<String, Set<String>> packagesContents = new HashMap<>();
  private final JavaFileManager fileManager;

  public DefaultSourceOnlyAbiRuleInfo(
      JavaFileManager fileManager,
      String fullyQualifiedBuildTargetName,
      boolean ruleIsRequiredForSourceOnlyAbi) {
    this.fullyQualifiedBuildTargetName = fullyQualifiedBuildTargetName;
    this.ruleIsRequiredForSourceOnlyAbi = ruleIsRequiredForSourceOnlyAbi;
    this.fileManager = fileManager;
  }

  private JavaFileManager getFileManager() {
    return Objects.requireNonNull(fileManager);
  }

  @Override
  public String getRuleName() {
    return fullyQualifiedBuildTargetName;
  }

  @Override
  public boolean ruleIsRequiredForSourceOnlyAbi() {
    return ruleIsRequiredForSourceOnlyAbi;
  }

  @Override
  @Nullable
  public String getOwningTarget(Elements elements, Element element) {
    return null;
  }

  @Override
  public boolean elementIsAvailableForSourceOnlyAbi(Elements elements, Element element) {
    return classIsOnBootClasspath(elements, element);
  }

  private boolean classIsOnBootClasspath(Elements elements, Element element) {
    String binaryName = elements.getBinaryName(MoreElements.getTypeElement(element)).toString();
    String packageName = getPackageName(binaryName);
    Set<String> packageContents = getBootclasspathPackageContents(packageName);
    return packageContents.contains(binaryName);
  }

  private Set<String> getBootclasspathPackageContents(String packageName) {
    Set<String> packageContents = packagesContents.get(packageName);
    if (packageContents == null) {
      packageContents = new HashSet<>();

      try {
        JavaFileManager fileManager = getFileManager();
        for (JavaFileObject javaFileObject :
            fileManager.list(
                StandardLocation.PLATFORM_CLASS_PATH,
                packageName,
                EnumSet.of(JavaFileObject.Kind.CLASS),
                true)) {
          packageContents.add(
              fileManager.inferBinaryName(StandardLocation.PLATFORM_CLASS_PATH, javaFileObject));
        }
      } catch (IOException e) {
        throw new RuntimeException("Failed to list boot classpath contents.", e);
        // Do nothing
      }
      packagesContents.put(packageName, packageContents);
    }
    return packageContents;
  }

  private String getPackageName(String binaryName) {
    int lastDot = binaryName.lastIndexOf('.');
    if (lastDot < 0) {
      return "";
    }

    return binaryName.substring(0, lastDot);
  }
}
