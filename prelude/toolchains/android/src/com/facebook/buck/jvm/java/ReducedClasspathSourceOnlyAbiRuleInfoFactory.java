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

import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory;
import com.facebook.buck.jvm.java.lang.model.MoreElements;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.zip.ZipFile;
import javax.lang.model.element.Element;
import javax.lang.model.util.Elements;
import javax.tools.JavaFileManager;
import org.jspecify.annotations.Nullable;

/** Availability on the reduced dependency classpath and this compilation's generated classes. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ReducedClasspathSourceOnlyAbiRuleInfoFactory
    implements SourceOnlyAbiRuleInfoFactory {
  private final SourceOnlyAbiRuleInfoFactory delegate;
  private final List<Path> classpath;
  private final List<Path> localClassDirectories;

  public ReducedClasspathSourceOnlyAbiRuleInfoFactory(
      SourceOnlyAbiRuleInfoFactory delegate,
      List<Path> classpath,
      List<Path> localClassDirectories) {
    this.delegate = delegate;
    this.classpath = List.copyOf(classpath);
    this.localClassDirectories = List.copyOf(localClassDirectories);
  }

  @Override
  public boolean shouldValidateInheritedMemberTypes() {
    return true;
  }

  @Override
  public SourceOnlyAbiRuleInfo create(JavaFileManager fileManager) {
    SourceOnlyAbiRuleInfo base = delegate.create(fileManager);
    Set<String> classNames = new HashSet<>();
    List<Path> directories = new ArrayList<>(localClassDirectories);
    for (Path entry : classpath) {
      if (Files.isDirectory(entry)) {
        directories.add(entry);
        continue;
      }
      try (ZipFile zip = new ZipFile(entry.toFile())) {
        zip.stream()
            .map(e -> e.getName())
            .filter(name -> name.endsWith(".class") && !name.startsWith("META-INF/"))
            .forEach(
                name -> classNames.add(name.substring(0, name.length() - 6).replace('/', '.')));
      } catch (IOException e) {
        // An incomplete availability index cannot safely validate inherited member types.
        throw new UncheckedIOException("Cannot read source-only ABI classpath entry " + entry, e);
      }
    }
    return new SourceOnlyAbiRuleInfo() {
      @Override
      @Nullable
      public String getOwningTarget(Elements elements, Element element) {
        return base.getOwningTarget(elements, element);
      }

      @Override
      public boolean elementIsAvailableForSourceOnlyAbi(Elements elements, Element element) {
        String binaryName = elements.getBinaryName(MoreElements.getTypeElement(element)).toString();
        return classNames.contains(binaryName)
            || directories.stream()
                .anyMatch(
                    dir ->
                        Files.isRegularFile(dir.resolve(binaryName.replace('.', '/') + ".class")))
            || base.elementIsAvailableForSourceOnlyAbi(elements, element);
      }

      @Override
      public String getRuleName() {
        return base.getRuleName();
      }

      @Override
      public boolean ruleIsRequiredForSourceOnlyAbi() {
        return base.ruleIsRequiredForSourceOnlyAbi();
      }
    };
  }
}
