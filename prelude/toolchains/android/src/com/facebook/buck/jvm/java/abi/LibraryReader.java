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
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import javax.annotation.processing.Messager;
import javax.lang.model.SourceVersion;
import javax.lang.model.element.Element;
import javax.lang.model.util.Types;
import org.objectweb.asm.ClassVisitor;

/** An interface for reading and listing resources and classes in a library. */
@Nullsafe(Nullsafe.Mode.LOCAL)
interface LibraryReader extends AutoCloseable {
  static LibraryReader of(Path path) {
    if (Files.isDirectory(path)) {
      return new DirectoryReader(path);
    } else {
      return new JarReader(path);
    }
  }

  static LibraryReader of(
      SourceVersion targetVersion,
      ElementsExtended elements,
      Types types,
      Messager messager,
      Iterable<Element> topLevelElements,
      boolean includeParameterMetadata) {
    return new ElementsReader(
        targetVersion, elements, types, messager, topLevelElements, includeParameterMetadata);
  }

  List<Path> getRelativePaths() throws IOException;

  InputStream openResourceFile(Path relativePath) throws IOException;

  InputStream openInputStream(Path relativePath) throws IOException;

  void visitClass(Path relativePath, ClassVisitor cv, boolean skipCode) throws IOException;

  @Override
  void close() throws IOException;

  default boolean isResource(Path path) {
    return !isClass(path);
  }

  default boolean isClass(Path path) {
    return path.toString().endsWith(".class");
  }
}
