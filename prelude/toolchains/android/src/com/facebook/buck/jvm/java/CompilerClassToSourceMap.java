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

import com.facebook.buck.util.json.ObjectMappers;
import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.type.TypeReference;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collection;
import java.util.Collections;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.stream.Collectors;

/**
 * Compiler output paths relative to the classes directory, and their source paths relative to the
 * cell.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class CompilerClassToSourceMap {
  public static final String JAVA_FILE_NAME = "java-class-to-source.json";
  public static final String KOTLIN_FILE_NAME = "kotlin-class-to-source.json";

  private final Map<String, Set<String>> entries = new TreeMap<>();

  public void put(String classFile, Collection<Path> sources, Path root) {
    Set<String> paths = new TreeSet<>();
    Path absoluteRoot = root.toAbsolutePath().normalize();
    for (Path source : sources) {
      Path absoluteSource = absoluteRoot.resolve(source).normalize();
      if (absoluteSource.startsWith(absoluteRoot)) {
        paths.add(absoluteRoot.relativize(absoluteSource).toString().replace('\\', '/'));
      }
    }
    entries.put(classFile.replace('\\', '/'), paths);
  }

  public Map<String, Set<String>> getEntries() {
    return Collections.unmodifiableMap(entries);
  }

  public void merge(CompilerClassToSourceMap other) {
    other.entries.forEach(
        (classFile, sources) ->
            entries.computeIfAbsent(classFile, ignored -> new TreeSet<>()).addAll(sources));
  }

  public void retainExistingOutputs(Path classesDir) {
    entries.keySet().removeIf(classFile -> !Files.isRegularFile(classesDir.resolve(classFile)));
  }

  public void write(Path output) throws IOException {
    Files.createDirectories(Objects.requireNonNull(output.toAbsolutePath().getParent()));
    ObjectMappers.WRITER.writeValue(output.toFile(), entries);
  }

  public static CompilerClassToSourceMap read(Path input) throws IOException {
    CompilerClassToSourceMap result = new CompilerClassToSourceMap();
    Map<String, Set<String>> values =
        ObjectMappers.readValue(input, new TypeReference<Map<String, Set<String>>>() {});
    if (values == null
        || values.values().stream()
            .anyMatch(sources -> sources == null || sources.stream().anyMatch(Objects::isNull))) {
      throw new IOException("Invalid null value in class-to-source map: " + input);
    }
    values.forEach((classFile, sources) -> result.entries.put(classFile, new TreeSet<>(sources)));
    return result;
  }

  public static void prepare(CompilerOutputPaths paths, Path root) throws IOException {
    if (paths.getCompilerClassToSourceMap() == null) {
      return;
    }
    Path workingDir = root.resolve(paths.getWorkingDirectory().getPath());
    Files.deleteIfExists(workingDir.resolve(JAVA_FILE_NAME));
    Files.deleteIfExists(workingDir.resolve(KOTLIN_FILE_NAME));
  }

  public static void writeCombined(CompilerOutputPaths paths, Path root, Collection<Path> sources)
      throws IOException {
    if (paths.getCompilerClassToSourceMap() == null) {
      return;
    }
    CompilerClassToSourceMap combined = new CompilerClassToSourceMap();
    Path workingDir = root.resolve(paths.getWorkingDirectory().getPath());
    for (String name : java.util.List.of(JAVA_FILE_NAME, KOTLIN_FILE_NAME)) {
      Path input = workingDir.resolve(name);
      if (Files.exists(input)) {
        combined.merge(read(input));
      }
    }
    Set<String> declaredSources =
        sources.stream()
            .map(
                source ->
                    root.toAbsolutePath()
                        .normalize()
                        .relativize(root.resolve(source).toAbsolutePath().normalize())
                        .toString()
                        .replace('\\', '/'))
            .collect(Collectors.toSet());
    combined.entries.values().forEach(origins -> origins.retainAll(declaredSources));
    combined.write(root.resolve(paths.getCompilerClassToSourceMap().getPath()));
  }
}
