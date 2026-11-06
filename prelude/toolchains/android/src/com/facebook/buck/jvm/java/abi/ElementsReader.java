/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi;

import com.facebook.buck.jvm.java.lang.model.ElementsExtended;
import com.facebook.buck.util.MoreSuppliers;
import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.InputStream;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.function.Supplier;
import javax.annotation.processing.Messager;
import javax.lang.model.SourceVersion;
import javax.lang.model.element.Element;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.PackageElement;
import javax.lang.model.element.TypeElement;
import javax.lang.model.util.Types;
import org.objectweb.asm.ClassVisitor;

/** A {@link LibraryReader} that reads from a list of {@link Element}s and their inner types. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ElementsReader implements LibraryReader {
  private final SourceVersion targetVersion;
  private final ElementsExtended elements;
  private final Types types;
  private final Messager messager;
  private final Supplier<Map<Path, Element>> allElements;
  private final boolean includeParameterMetadata;

  ElementsReader(
      SourceVersion targetVersion,
      ElementsExtended elements,
      Types types,
      Messager messager,
      Iterable<Element> topLevelElements,
      boolean includeParameterMetadata) {
    this.targetVersion = targetVersion;
    this.elements = elements;
    this.types = types;
    this.messager = messager;
    this.allElements =
        MoreSuppliers.memoize(
            () -> {
              Map<Path, Element> allElements = new LinkedHashMap<>();
              topLevelElements.forEach(element -> addAllElements(element, allElements));
              return allElements;
            });
    this.includeParameterMetadata = includeParameterMetadata;
  }

  @Override
  public List<Path> getRelativePaths() {
    return new ArrayList<>(allElements.get().keySet());
  }

  @Override
  public InputStream openResourceFile(Path relativePath) {
    throw new UnsupportedOperationException();
  }

  @Override
  public InputStream openInputStream(Path relativePath) {
    throw new UnsupportedOperationException();
  }

  @Override
  public void visitClass(Path relativePath, ClassVisitor cv, boolean skipCodeUnused) {
    Element element = Objects.requireNonNull(allElements.get().get(relativePath));
    new ClassVisitorDriverFromElement(
            targetVersion, elements, messager, types, includeParameterMetadata)
        .driveVisitor(element, cv);
  }

  @Override
  public void close() {
    // Nothing
  }

  private void addAllElements(Element rootElement, Map<Path, Element> elements) {
    if (rootElement.getKind() == ElementKind.PACKAGE) {
      PackageElement packageElement = (PackageElement) rootElement;
      if (!packageElement.getAnnotationMirrors().isEmpty()) {
        elements.put(
            getRelativePathToClass(packageElement.getQualifiedName() + ".package-info"),
            packageElement);
      }
    }

    if (!rootElement.getKind().isClass() && !rootElement.getKind().isInterface()) {
      return;
    }

    TypeElement typeElement = (TypeElement) rootElement;
    elements.put(getRelativePath(typeElement), typeElement);
    for (Element enclosed : typeElement.getEnclosedElements()) {
      addAllElements(enclosed, elements);
    }
  }

  private Path getRelativePathToClass(CharSequence classBinaryName) {
    return Paths.get(
        String.format("%s.class", classBinaryName.toString().replace('.', File.separatorChar)));
  }

  private Path getRelativePath(TypeElement typeElement) {
    return getRelativePathToClass(elements.getBinaryName(typeElement));
  }
}
