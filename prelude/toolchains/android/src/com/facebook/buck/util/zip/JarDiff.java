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

import static java.util.stream.Collectors.toCollection;

import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Iterator;
import java.util.LinkedHashSet;
import java.util.Objects;
import java.util.Set;
import java.util.TreeSet;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.stream.Stream;
import org.jspecify.annotations.Nullable;

/**
 * A utility class for comparing the contents of two JAR files. Compares the contents of two JAR
 * files. Detects entries that exist only in one jar. Validate the content of binary files, and
 * property files. It will exclude property comments since they might have built dates that differ.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JarDiff {

  private static final String INNER_JAR = "inner.jar";
  private static final byte[] BUFFER = new byte[8096];
  private static final byte[] OTHER_BUFFER = new byte[8096];

  /**
   * @param args command-line arguments (two JAR file paths)
   * @throws IOException if an I/O error occurs
   */
  public static void main(String[] args) throws IOException {
    if (args.length == 2) {
      new JarDiff().diff(Path.of(args[0]), Path.of(args[1]));
    } else {
      System.out.println("requires 2 jar paths");
    }
  }

  private void diff(Path path, Path otherPath) throws IOException {
    try (final JarFile jar = new JarFile(path.toFile(), true);
        final JarFile otherJar = new JarFile(otherPath.toFile(), true); ) {
      final Set<String> names = entryNames(jar);
      final Set<String> otherNames = entryNames(otherJar);

      final Set<String> commonNames = common(otherNames, names);
      final Set<String> uniqueNames = unique(names, otherNames);
      final Set<String> otherUniqueNames = unique(otherNames, names);

      System.out.printf(
          "evaluating %s entries of [%s, %s] %n",
          commonNames.size(), path.getFileName(), otherPath.getFileName());

      uniqueNames.forEach(
          name -> System.out.printf("- [%s] has unique entry: %s %n", path.getFileName(), name));
      otherUniqueNames.forEach(
          name ->
              System.out.printf("- [%s] has unique entry: %s %n", otherPath.getFileName(), name));

      commonNames.stream()
          .map(
              name -> {
                if (INNER_JAR.equals(name)) {
                  return null;
                }
                if (isPropertyFile(name) || name.startsWith("META-INF/services/")) {
                  return compareText(name, jar, otherJar);
                }
                return compareBytes(name, jar, otherJar);
              })
          .filter(Objects::nonNull)
          .forEach(System.out::println);

      if (commonNames.contains(INNER_JAR)) {
        diff(extractInnerJar(jar), extractInnerJar(otherJar));
      }
    }
  }

  private Path extractInnerJar(final JarFile jar) throws IOException {
    final Path tempFile =
        Files.createTempFile(Path.of(jar.getName()).getFileName() + "-", "-" + INNER_JAR);
    try (final InputStream input =
            jar.getInputStream(Objects.requireNonNull(jar.getJarEntry(INNER_JAR)));
        final OutputStream output = Files.newOutputStream(tempFile); ) {
      input.transferTo(output);
    }
    return tempFile;
  }

  private boolean isPropertyFile(String name) {
    return Stream.of(".properties", ".MF", ".factories").anyMatch(name::endsWith);
  }

  private @Nullable String compareBytes(String name, JarFile jar, JarFile otherJar) {
    try (final InputStream in = jar.getInputStream(Objects.requireNonNull(jar.getJarEntry(name)));
        final InputStream otherIn =
            otherJar.getInputStream(Objects.requireNonNull(otherJar.getJarEntry(name))); ) {

      long totalSize = 0;
      long otherTotalSize = 0;

      while (true) {

        int read = in.readNBytes(BUFFER, 0, BUFFER.length);
        int otherRead = otherIn.readNBytes(OTHER_BUFFER, 0, OTHER_BUFFER.length);

        totalSize += Math.max(0, read);
        otherTotalSize += Math.max(0, otherRead);

        if (read != otherRead) {
          return String.format(
              "! %s has byte difference at %s", name, Math.min(totalSize, otherTotalSize));
        }
        if (read > 0) {
          for (int i = 0; i < read; i++) {
            if (BUFFER[i] != OTHER_BUFFER[i]) {
              return String.format("! %s has byte difference at %s", name, (totalSize - read) + i);
            }
          }
        } else {
          break;
        }
      }
    } catch (IOException e) {
      throw new UncheckedIOException(e);
    }
    // both are similar
    return null;
  }

  private @Nullable String compareText(String name, JarFile jar, JarFile otherJar) {
    try (final InputStream in = jar.getInputStream(Objects.requireNonNull(jar.getJarEntry(name)));
        final InputStream otherIn =
            otherJar.getInputStream(Objects.requireNonNull(otherJar.getJarEntry(name))); ) {

      final Iterator<String> lines = textLines(in);
      final Iterator<String> otherLines = textLines(otherIn);

      while (lines.hasNext() && otherLines.hasNext()) {
        String line = lines.next();
        String otherLine = otherLines.next();
        if (!line.equals(otherLine)) {
          return String.format(
              "! %s has difference in lines \"%s\" <> \"%s\"", name, line, otherLine);
        }
      }
      if (lines.hasNext()) {
        return String.format("! %s has difference in line \"%s\"", name, lines.next());
      }
      if (otherLines.hasNext()) {
        return String.format("! %s has difference in line \"%s\"", name, otherLines.next());
      }

    } catch (IOException e) {
      throw new UncheckedIOException(e);
    }
    return null;
  }

  private Iterator<String> textLines(InputStream inputStream) {
    return new BufferedReader(new InputStreamReader(inputStream))
        .lines()
        .map(String::trim)
        .filter(s -> !s.isEmpty())
        // ignore comment lines
        .filter(s -> Stream.of("!", "#").noneMatch(s::startsWith))
        .iterator();
  }

  private Set<String> common(Set<String> names, Set<String> universe) {
    return names.stream().filter(universe::contains).collect(toCollection(TreeSet::new));
  }

  private Set<String> unique(Set<String> names, Set<String> universe) {
    return names.stream()
        .filter(name -> !universe.contains(name))
        .collect(toCollection(TreeSet::new));
  }

  private Set<String> entryNames(JarFile jar) {
    return jar.stream()
        .filter(e -> !e.isDirectory())
        .map(JarEntry::getName)
        .collect(toCollection(LinkedHashSet::new));
  }
}
