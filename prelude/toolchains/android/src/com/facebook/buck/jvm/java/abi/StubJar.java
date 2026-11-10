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

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.jvm.java.abi.kotlin.InlineFunctionScope;
import com.facebook.buck.jvm.java.lang.model.ElementsExtended;
import com.facebook.buck.util.zip.JarBuilder;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.Deque;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeSet;
import java.util.function.Supplier;
import java.util.stream.Collectors;
import javax.annotation.processing.Messager;
import javax.lang.model.SourceVersion;
import javax.lang.model.element.Element;
import javax.lang.model.util.Types;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class StubJar {
  private final Supplier<LibraryReader> libraryReaderSupplier;
  @Nullable private AbiGenerationMode compatibilityMode = null;
  @Nullable private Supplier<LibraryReader> existingAbiSupplier = null;
  private boolean keepSynthetic = false;

  public StubJar(AbsPath jarPath) {
    libraryReaderSupplier = () -> LibraryReader.of(jarPath.getPath());
  }

  public StubJar(AbsPath jarPath, boolean keepSynthetic) {
    libraryReaderSupplier = () -> LibraryReader.of(jarPath.getPath());
    this.keepSynthetic = keepSynthetic;
  }

  /**
   * @param targetVersion the class file version to output, expressed as the corresponding Java
   *     source version
   */
  public StubJar(
      SourceVersion targetVersion,
      ElementsExtended elements,
      Types types,
      Messager messager,
      Iterable<Element> topLevelElements,
      boolean includeParameterMetadata,
      boolean keepSynthetic) {
    libraryReaderSupplier =
        () ->
            LibraryReader.of(
                targetVersion,
                elements,
                types,
                messager,
                topLevelElements,
                includeParameterMetadata);
  }

  /**
   * Filters the stub jar through {@link SourceAbiCompatibleVisitor}. See that class for details.
   */
  public StubJar setCompatibilityMode(AbiGenerationMode compatibilityMode) {
    this.compatibilityMode = compatibilityMode;
    return this;
  }

  /** Specify a directory of existing abi we need to inherit */
  public StubJar setExistingAbiJar(AbsPath existingAbiJar) {
    this.existingAbiSupplier = () -> LibraryReader.of(existingAbiJar.getPath());
    return this;
  }

  public void writeClasses(AbsPath classesDir) throws IOException {
    try (LibraryReader input = libraryReaderSupplier.get();
        StubJarWriter writer = new StubClassFileWriter(classesDir)) {
      writeTo(input, writer, null);
    }
  }

  /** Writes output into the passed absolute path. */
  public void writeTo(AbsPath outputAbsPath) throws IOException {
    // The order of these declarations is important -- FilesystemStubJarWriter must be declared
    // after LibraryReader. This is because FilesystemStubJarWriter actually uses the LibraryReader
    // in its close method, and try-with-resources closes the items in the opposite order of their
    // creation.
    try (LibraryReader input = libraryReaderSupplier.get();
        LibraryReader existingAbiInput =
            existingAbiSupplier != null ? existingAbiSupplier.get() : null;
        StubJarWriter writer = new FilesystemStubJarWriter(outputAbsPath)) {
      writeTo(input, writer, existingAbiInput);
    }
  }

  public void writeTo(JarBuilder jarBuilder) throws IOException {
    try (LibraryReader input = libraryReaderSupplier.get();
        LibraryReader existingAbiInput =
            existingAbiSupplier != null ? existingAbiSupplier.get() : null;
        StubJarWriter writer = new JarBuilderStubJarWriter(jarBuilder)) {
      writeTo(input, writer, existingAbiInput);
    }
  }

  private void writeTo(
      LibraryReader input, StubJarWriter writer, @Nullable LibraryReader existingAbiInput)
      throws IOException {
    List<Path> relativePaths = input.getRelativePaths();
    Comparator<Path> visitOuterClassesFirst = Comparator.comparing(StubJar::pathWithoutClassSuffix);
    List<Path> paths =
        relativePaths.stream().sorted(visitOuterClassesFirst).collect(Collectors.toList());

    boolean isKotlinModule = isKotlinModule(relativePaths);

    InlineFunctionScope inlineFunctionScope = isKotlinModule ? new InlineFunctionScope() : null;

    if (!isKotlinModule && keepSynthetic) {
      throw new IllegalStateException(
          "keepSynthetic is intended only to be used on Kotlin modules");
    }

    // Calculate all existingAbiPathStrings here outside of for-loop,
    // to save calculation for each path / StubJarEntry
    Set<String> existingAbiPathStrings =
        existingAbiInput != null
            ? existingAbiInput.getRelativePaths().stream()
                .map(Path::toString)
                .collect(Collectors.toSet())
            : Collections.emptySet();

    Map<String, StubJarEntry> deferredEntries = new HashMap<>();
    Set<String> referencedByWritten = new HashSet<>();
    List<StubJarEntry> writtenExistingEntries = new ArrayList<>();

    for (Path path : paths) {
      StubJarEntry entry =
          StubJarEntry.of(
              input,
              path,
              existingAbiInput,
              existingAbiPathStrings,
              compatibilityMode,
              inlineFunctionScope,
              keepSynthetic);
      if (entry == null) {
        continue;
      }
      String className = entry.getClassName();
      String scopeOuter = entry.getScopeOuterClass();
      // Entries visit outer-first, so a nested class whose outer is already deferred
      // cannot be written yet: it must drop with its outer unless the fixpoint keeps it.
      // This covers scope-captured lambdas as well as effectively-private member
      // classes, such as a public companion nested in a file-private class.
      boolean outerDeferred =
          className != null && scopeOuter != null && deferredEntries.containsKey(scopeOuter);
      if ((entry.isFilePrivateKotlinClass() && className != null) || outerDeferred) {
        deferredEntries.put(className, entry);
      } else {
        entry.write(writer);
        if (entry instanceof StubJarExistingEntry) {
          writtenExistingEntries.add(entry);
        } else {
          referencedByWritten.addAll(entry.getReferencedClassNames());
          referencedByWritten.addAll(entry.getSealedSubclasses());
        }
      }
      if (inlineFunctionScope != null) {
        String pathNoSuffix = pathWithoutClassSuffix(path);
        inlineFunctionScope.createScopes(pathNoSuffix, entry.getInlineFunctions());
        inlineFunctionScope.recordInlineBodyFieldOwners(entry.getRetainedBodyFieldOwners());
        if (entry.extendsInlineFunctionScope()) {
          inlineFunctionScope.extendScope(pathNoSuffix);
        }
      }
    }

    if (!deferredEntries.isEmpty()) {
      for (StubJarEntry existingEntry : writtenExistingEntries) {
        referencedByWritten.addAll(existingEntry.getReferencedClassNames());
        referencedByWritten.addAll(existingEntry.getSealedSubclasses());
      }

      // keptNames only grows and each pass that changes anything adds to it, so this terminates.
      Set<String> keptNames = new TreeSet<>();
      Deque<String> worklist = new ArrayDeque<>(referencedByWritten);
      boolean changed = true;
      while (changed) {
        changed = false;
        while (!worklist.isEmpty()) {
          String referencedName = worklist.removeFirst();
          StubJarEntry deferred = deferredEntries.get(referencedName);
          if (deferred != null && keptNames.add(referencedName)) {
            worklist.addAll(deferred.getReferencedClassNames());
            worklist.addAll(deferred.getSealedSubclasses());
            changed = true;
          }
        }
        for (Map.Entry<String, StubJarEntry> e : deferredEntries.entrySet()) {
          StubJarEntry deferred = e.getValue();
          String outer = deferred.getScopeOuterClass();
          if (outer == null || keptNames.contains(e.getKey()) || !keptNames.contains(outer)) {
            continue;
          }
          keptNames.add(e.getKey());
          worklist.addAll(deferred.getReferencedClassNames());
          worklist.addAll(deferred.getSealedSubclasses());
          changed = true;
        }
      }

      for (String keptName : keptNames) {
        Objects.requireNonNull(deferredEntries.get(keptName)).write(writer);
      }
    }
  }

  private boolean isKotlinModule(List<Path> relativePaths) {
    return relativePaths.stream().anyMatch(path -> path.toString().endsWith(".kotlin_module"));
  }

  static String pathWithoutClassSuffix(Path path) {
    final String pathString = path.toString();
    return pathString.endsWith(".class")
        ? pathString.substring(0, pathString.length() - ".class".length())
        : pathString;
  }
}
