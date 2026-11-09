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

import static java.nio.file.StandardOpenOption.APPEND;
import static java.nio.file.StandardOpenOption.CREATE;
import static java.nio.file.StandardOpenOption.READ;
import static java.nio.file.StandardOpenOption.TRUNCATE_EXISTING;
import static java.nio.file.StandardOpenOption.WRITE;
import static java.util.Comparator.comparing;
import static java.util.Objects.requireNonNull;
import static java.util.stream.Collectors.partitioningBy;
import static java.util.stream.Collectors.toList;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedOutputStream;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.channels.FileChannel;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Predicate;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.stream.Stream;

/**
 * This class directly copies compressed data blocks from multiple JAR files and merges them into a
 * single JAR. A new Central Directory is created with updated relative offsets to reference the
 * combined content.
 *
 * <p>Advantages:
 *
 * <ul>
 *   <li>Significantly faster: Avoids the overhead of decompressing, merging, and recompressing JAR
 *       files in memory.
 *   <li>Preserves compression: Maintains original compression levels for optimal storage
 *       efficiency.
 * </ul>
 *
 * <p>Important:
 *
 * <p>Duplicate entries: If a class or resource exists in multiple input JARs, the last occurrence
 * will be retained in the final output JAR.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ConcatenateJars {

  public static final Predicate<? super CustomZipEntry> DONT_EXCLUDE = (name) -> false;

  /**
   * Represents a JAR file that has been relocated to a new offset in a larger archive. It contains
   * information about the original JAR file, its updated offset, and its relocated entries.
   */
  static class RelocatedJar {

    private final AbsPath jar;
    private final CentralDirectoryHeader header;
    private final long relocatedOffset;
    private final List<EntryAccounting> relocatedEntries;

    RelocatedJar(
        final AbsPath jar,
        final CentralDirectoryHeader header,
        final long relocatedOffset,
        final List<EntryAccounting> relocatedEntries) {
      this.jar = jar;
      this.header = header;
      this.relocatedOffset = relocatedOffset;
      this.relocatedEntries = relocatedEntries;
    }

    @Override
    public String toString() {
      return Objects.toString(path());
    }

    AbsPath jar() {
      return jar;
    }

    Path path() {
      return jar.getPath();
    }

    CentralDirectoryHeader header() {
      return requireNonNull(header);
    }

    long dataSize() {
      return header.getOffset();
    }

    long relocatedOffset() {
      return relocatedOffset;
    }

    Stream<EntryAccounting> entries() {
      return relocatedEntries.stream();
    }
  }

  private final CentralDirectoryReader directoryReader = new CentralDirectoryReader();

  private final Set<AbsPath> jars = new LinkedHashSet<>();
  private final Set<AbsPath> overrideJars = new LinkedHashSet<>();

  private boolean sortEntries = true;
  private boolean discardDirectories = true;
  private Predicate<? super CustomZipEntry> removeEntries = DONT_EXCLUDE;

  /**
   * Filter used to discard entries from the final merged index.
   *
   * @param excludeEntries predicate that test if an entry should be excluded.
   * @return this builder instance for method chaining
   */
  public ConcatenateJars removeEntries(Predicate<? super CustomZipEntry> removeEntries) {
    this.removeEntries = removeEntries != null ? removeEntries : DONT_EXCLUDE;
    return this;
  }

  /**
   * Adds a JAR file to the list of JARs to be concatenated.
   *
   * @param jar the path to the JAR file to add
   * @return this builder instance for method chaining
   * @see #addJarOverride(AbsPath)
   */
  public ConcatenateJars addJar(final AbsPath jar) {
    this.jars.add(jar);
    return this;
  }

  /**
   * Adds a JAR file to the list of override JARs to be concatenated.
   *
   * @param jar the path to the JAR file to add
   * @return this builder instance for method chaining
   * @see #addJar(AbsPath)
   */
  public ConcatenateJars addJarOverride(final AbsPath jar) {
    this.overrideJars.add(jar);
    return this;
  }

  /**
   * If true, keep compatibility with JarBuilder entry orders.
   *
   * @param sortEntries If entries should be sorted to preserve natural order.
   */
  public void setSortEntries(final boolean sortEntries) {
    this.sortEntries = sortEntries;
  }

  /**
   * If true, keep compatibility with JarBuilder entries.
   *
   * @param discardDirectories If entries that are directories should be discarded.
   */
  public void setDiscardDirectories(final boolean discardDirectories) {
    this.discardDirectories = discardDirectories;
  }

  /**
   * @param output Location of the final jar.
   * @return JAR OutputStream which all jars already concatenated, which can be used to add new
   *     entries.
   * @throws IOException In case it fail to concatenate jars.
   */
  public CustomJarOutputStream appendableOutputStream(final Path output) throws IOException {
    // read and process relocated jars
    final List<RelocatedJar> relocatedJars = relocateJars();

    // concat all data segments of all jars
    final long currentOffset = transferJars(relocatedJars, output);

    // create an appendable outputStream with buffer
    final AppendingZipOutputStreamImpl appendStream = newAppendStream(output);

    // partition jars and override jars
    final Map<Boolean, List<RelocatedJar>> overridePartition =
        relocatedJars.stream().collect(partitioningBy(this::isOverride));
    final Stream<EntryAccounting> entries =
        Objects.requireNonNull(overridePartition.get(Boolean.FALSE)).stream()
            .flatMap(RelocatedJar::entries);
    final Stream<EntryAccounting> overrideEntries =
        Objects.requireNonNull(overridePartition.get(Boolean.TRUE)).stream()
            .flatMap(RelocatedJar::entries);

    if (sortEntries) {
      // backward compatibility with JarBuilder ordering, which select the first occurrency
      entries
          .sorted(comparing(EntryAccounting::getName))
          .filter(entry -> !appendStream.hasEntry(entry.getName()))
          .filter(this::shouldSkipEntry)
          .forEach(appendStream::addEntry);
    } else {
      entries.filter(this::shouldSkipEntry).forEach(appendStream::addEntry);
    }
    // add entries that override at the end
    overrideEntries.filter(this::shouldSkipEntry).forEach(appendStream::addEntry);
    // reposition the current offset
    appendStream.setCurrentOffset(currentOffset);
    // return stream that will allow
    return new CustomJarOutputStream(appendStream);
  }

  private boolean shouldSkipEntry(final EntryAccounting entry) {
    final CustomZipEntry zipEntry = new CustomZipEntry(entry.getName());
    return !removeEntries.test(zipEntry);
  }

  private boolean isOverride(final RelocatedJar relocatedJar) {
    return overrideJars.contains(relocatedJar.jar());
  }

  private AppendingZipOutputStreamImpl newAppendStream(final Path output) throws IOException {
    return new AppendingZipOutputStreamImpl(
        new BufferedOutputStream(Files.newOutputStream(output, APPEND, WRITE)), false);
  }

  /**
   * Relocates a collection of JAR files to new offsets. Will concatenate jars and override jars
   * respecting its order.
   *
   * @return a list of relocated JAR files with their updated offsets and entries
   */
  private List<RelocatedJar> relocateJars() {
    final AtomicLong currentOffset = new AtomicLong();
    return Stream.concat(jars.stream(), overrideJars.stream())
        .map(
            jar -> {
              final CentralDirectoryHeader header = readCentralDirectory(jar);
              final long relocatedOffset = currentOffset.getAndAdd(header.getOffset());
              final List<EntryAccounting> relocatedEntries =
                  relocateEntries(header, relocatedOffset);

              return new RelocatedJar(jar, header, relocatedOffset, relocatedEntries);
            })
        .collect(toList());
  }

  /**
   * Reads the central directory header of a JAR file.
   *
   * @param jar the absolute path to the JAR file
   * @return the central directory header of the JAR file
   * @throws UncheckedIOException if an I/O error occurs while reading the JAR file
   */
  private CentralDirectoryHeader readCentralDirectory(final AbsPath jar) {
    try {
      return directoryReader.readCentralDirectory(jar.getPath());
    } catch (IOException e) {
      throw new UncheckedIOException("failed to read JAR header of " + jar.getPath(), e);
    }
  }

  /**
   * Relocates the entries of a JAR file to a new offset.
   *
   * @param header the central directory header of the JAR file
   * @param relocatedOffset the new offset to relocate the entries to
   * @return a list of entries with the updated offsets
   */
  private List<EntryAccounting> relocateEntries(
      final CentralDirectoryHeader header, final long relocatedOffset) {
    try (final JarFile jar = new JarFile(header.getPath().toFile(), false)) {

      final List<EntryAccounting> entries = new ArrayList<>(header.getCount());

      for (final CentralDirectoryFileHeader file : header.getFiles()) {
        // JarBuilder will exclude directories and recreate it
        if (discardDirectories && file.getName().endsWith("/")) {
          continue;
        }
        final JarEntry entry = Objects.requireNonNull(jar.getJarEntry(file.getName()));
        // add entry again to recompute the new offset
        entries.add(new EntryAccounting(new JarEntry(entry), relocatedOffset + file.getOffset()));
      }

      return entries;
    } catch (IOException e) {
      throw new UncheckedIOException("failed to relocate jar " + header.getPath(), e);
    }
  }

  /**
   * Transfers the contents of a collection of relocated JAR files to a single output file.
   *
   * @param relocatedJars an iterable collection of relocated JAR files to transfer
   * @param output the path to the output file
   * @return the total number of bytes transferred
   * @throws IOException if an I/O error occurs during the transfer
   */
  private long transferJars(final Iterable<RelocatedJar> relocatedJars, final Path output)
      throws IOException {
    try (final FileChannel outputChannel =
        FileChannel.open(output, CREATE, TRUNCATE_EXISTING, WRITE)) {

      outputChannel.truncate(0);
      outputChannel.position(0);

      long written = 0;
      for (RelocatedJar relocatedJar : relocatedJars) {
        try (FileChannel inputChannel = FileChannel.open(relocatedJar.jar().getPath(), READ)) {

          if (written != relocatedJar.relocatedOffset()) {
            throw new IOException(
                String.format(
                    "current position %s is different from relocated offset (%s) at %s",
                    written, relocatedJar.relocatedOffset(), relocatedJar.path()));
          }

          final long dataSize = relocatedJar.dataSize();
          final long transferred = transferTo(inputChannel, dataSize, outputChannel);

          if (transferred != dataSize) {
            throw new IOException(
                String.format(
                    "failed to transfer %s bytes from %s",
                    dataSize - transferred, relocatedJar.path()));
          }
          written += transferred;
        }
      }
      return written;
    }
  }

  /**
   * Transfers data from an input file channel to an output file channel.
   *
   * @param inputChannel the input file channel to read from
   * @param size the total number of bytes to transfer
   * @param outputChannel the output file channel to write to
   * @return the total number of bytes transferred
   * @throws IOException if an I/O error occurs during the transfer
   */
  private long transferTo(
      final FileChannel inputChannel, final long size, final FileChannel outputChannel)
      throws IOException {
    long written = 0;
    while (written < size) {
      final long transferred = inputChannel.transferTo(written, size - written, outputChannel);
      if (transferred > 0) {
        written += transferred;
      } else {
        break;
      }
    }
    return written;
  }
}
