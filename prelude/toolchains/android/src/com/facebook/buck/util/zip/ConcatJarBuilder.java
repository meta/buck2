/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import static java.util.Optional.ofNullable;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collection;
import java.util.LinkedList;
import java.util.List;
import java.util.stream.Collectors;
import org.jspecify.annotations.Nullable;

/**
 * Class designed to create a new JAR file by concatenating the contents of multiple existing JAR
 * files. It extends the JarBuilder class and provides additional functionality for handling merging
 * logic of resource files.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ConcatJarBuilder extends JarBuilder {

  private final List<AbsPath> entriesToJar = new LinkedList<>();
  private final List<AbsPath> overrideEntriesToJar = new LinkedList<>();

  /**
   * Creates a new JAR file at the specified output path by concatenating the contents of multiple
   * input JAR files.
   */
  protected CustomJarOutputStream getJarOutputStream(
      @Nullable final Path appendJar, final Path outputFile) throws IOException {

    final ConcatenateJars concatenateJars =
        new ConcatenateJars().removeEntries(getRemoveEntryPredicate());

    ofNullable(appendJar)
        .map(Path::toAbsolutePath)
        .map(AbsPath::of)
        .ifPresent(concatenateJars::addJar);

    entriesToJar.forEach(concatenateJars::addJar);
    overrideEntriesToJar.forEach(concatenateJars::addJarOverride);

    return concatenateJars.appendableOutputStream(outputFile);
  }

  @Override
  public JarBuilder setEntriesToJar(final Iterable<AbsPath> entries) {
    final List<AbsPath> jars = toList(entries);
    entriesToJar.addAll(jars);
    return super.setEntriesToJar(jars);
  }

  @Override
  public JarBuilder setOverrideEntriesToJar(final Iterable<AbsPath> entries) {
    final List<AbsPath> jars = toList(entries);
    overrideEntriesToJar.addAll(jars);
    return super.setOverrideEntriesToJar(jars);
  }

  private List<AbsPath> toList(Iterable<AbsPath> entries) {
    // An Iterable might be iterable only once, so we convert it to a list.
    final List<AbsPath> jars = new ArrayList<>();
    entries.forEach(jars::add);
    return jars;
  }

  /** All concatenated jars are read only, and are already written into the final jar. */
  protected JarEntryContainer toJarEntryContainer(final Path jar) {
    return JarEntryContainer.readOnlyJar(jar);
  }

  /**
   * Make addToEntries parallel, given it won't write to the output, add just update entries with
   * new offset.
   */
  protected void addToEntries(
      final Collection<JarEntrySupplier> sortedEntries,
      final List<JarEntryContainer> sourceContainers)
      throws IOException {

    final List<JarEntrySupplier> collectedEntries =
        sourceContainers.parallelStream()
            .flatMap(
                c -> {
                  try {
                    return c.stream();
                  } catch (IOException e) {
                    throw new UncheckedIOException(e);
                  }
                })
            .collect(Collectors.toList());

    sortedEntries.addAll(collectedEntries);
  }
}
