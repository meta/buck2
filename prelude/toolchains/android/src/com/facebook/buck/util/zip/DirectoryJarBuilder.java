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

import static java.nio.file.Files.isDirectory;
import static java.util.Collections.emptyList;
import static java.util.Comparator.comparingLong;
import static java.util.concurrent.CompletableFuture.completedFuture;
import static java.util.concurrent.CompletableFuture.supplyAsync;
import static java.util.stream.Collectors.partitioningBy;
import static java.util.stream.Collectors.toList;
import static java.util.stream.Stream.concat;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.util.zip.DirectoryJarEntryContainer.FileJarEntry;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Strings;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collection;
import java.util.Iterator;
import java.util.LinkedList;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import java.util.concurrent.CompletableFuture;
import java.util.function.Supplier;
import java.util.stream.Stream;

/**
 * Compress a list of directories into jars, leveraging multiple threads to maximize compression
 * efficiency. Automatically computes the optimal number of threads based on the number of files and
 * total byte size.
 * <li>Asynchronously compresses groups of evenly distributed files.
 * <li>Returns a list of temporary compressed JAR files.
 *
 * @see #buildJars(List)
 * @see #computeParallelism(DirectoryJarEntryContainer)
 * @see #buildJar(JarPartition)
 */
public class DirectoryJarBuilder {

  /** Minimum number of bytes, to be compressed that justify another thread to split the work. */
  static final int MIN_BYTES_PER_THREAD = 1024 * 1024;

  static final int MAX_PARALLEL_THREADS =
      Math.max(1, Runtime.getRuntime().availableProcessors() - 1);

  private final Supplier<JarBuilder> builderSupplier;

  private int parallelism = -1;

  @VisibleForTesting
  DirectoryJarBuilder() {
    this(JarBuilder::new);
  }

  /**
   * @param builderSupplier Factory method that creates the {@link JarBuilder} to be used for each
   *     compression operation.
   */
  public DirectoryJarBuilder(final Supplier<JarBuilder> builderSupplier) {
    this.builderSupplier = builderSupplier;
  }

  /**
   * Specify the parallelism level to use as threads/partitions when compressing files. Will be
   * considered by {@link #computeParallelism(DirectoryJarEntryContainer)} when evaluating
   * parallelism for {@link #buildJars(Stream)}
   *
   * @param parallelism Number of groups/threads to be used by the {@link #buildJars(Stream)}
   *     algorithm.
   * @return Itself to use chain methods.
   */
  public DirectoryJarBuilder withParallelism(final int parallelism) {
    this.parallelism = parallelism;
    return this;
  }

  /**
   * From a list of paths, partitions them into files and directories, then asynchronously
   * compresses the directories in JAR files. The compressed JARs and the other files are returned
   * as a single list.
   *
   * @see DirectoryJarEntryContainer
   */
  public CompletableFuture<List<AbsPath>> buildJars(final List<AbsPath> paths) {
    // split into files and directories
    final Map<Boolean, List<AbsPath>> partition =
        paths.stream().collect(partitioningBy(p -> isDirectory(p.getPath())));

    final List<AbsPath> files = partition.getOrDefault(Boolean.FALSE, emptyList());
    final List<AbsPath> directories = partition.getOrDefault(Boolean.TRUE, emptyList());

    // nothing to compress
    if (directories.isEmpty()) {
      return completedFuture(paths);
    }
    // compress directories asynchronously
    final CompletableFuture<List<AbsPath>> futureJars =
        buildJars(directories.stream().map(DirectoryJarEntryContainer::new));
    // concatenate the compressed jars with the other files
    return futureJars.thenApply(jars -> concat(jars.stream(), files.stream()).collect(toList()));
  }

  /**
   * Executes parallel compression of the files by grouping them based on the computed parallelism
   * level. This method utilizes the {@link #computeParallelism(DirectoryJarEntryContainer)} to
   * determine the optimal number of groups, and then creates a {@link JarBuilder} for each group to
   * generate a temporary compressed file.
   *
   * @param directories List of directories containing files to be compressed.
   * @return Future to a list of compressed temporary jar files.
   * @see #buildJar(JarPartition)
   */
  private CompletableFuture<List<AbsPath>> buildJars(
      final Stream<DirectoryJarEntryContainer> directories) {
    // for each container, split it in partitions for parallelism
    final Collection<JarPartition> partitions =
        directories.flatMap(this::createPartitions).collect(toList());
    // compress each partition in parallel
    final List<CompletableFuture<AbsPath>> futureJars =
        partitions.stream()
            .map(partition -> supplyAsync(() -> buildJar(partition)))
            .collect(toList());
    // convert to a single future to a list of paths
    return CompletableFuture.allOf(futureJars.toArray(CompletableFuture<?>[]::new))
        .thenApply((v) -> futureJars.stream().map(CompletableFuture::join).collect(toList()));
  }

  /**
   * Partition the file list into N groups with approximately similar total bytes, to optimize
   * parallel compression by balancing thread workload and maximizing compression throughput.
   */
  private Stream<JarPartition> createPartitions(final DirectoryJarEntryContainer directory) {
    // file partitions ordered by its total byte size
    final TreeSet<JarPartition> partitions = new TreeSet<>(JarPartition::compareSize);
    // generate the computed number of partitions
    Stream.generate(JarPartition::new)
        .limit(computeParallelism(directory))
        .forEach(partitions::add);
    // sort the files by size in descending order
    directory.getEntries().stream()
        .sorted(comparingLong(FileJarEntry::getFileSize).reversed())
        // distribute them evenly across partitions
        .forEach(entry -> partitions.add(partitions.pollFirst().add(entry)));
    // discard empty partitions
    return partitions.stream().filter(JarPartition::hasFiles);
  }

  /**
   * Calculates the optimal number of threads to use for processing a list of files. It will
   * consider:
   * <li>Total number of files.
   * <li>Total number of bytes, divided by the average bytes per thread (@link
   *     #AVG_BYTES_PER_THREAD).
   *
   *     <p>Evaluate the number of files and the number of threads to process group of files, given
   *     the group size of {@value #MIN_BYTES_PER_THREAD} bytes.
   *
   *     <p>In cases where there are fewer files than available cores or insufficient bytes to
   *     justify multiple threads, the method limits the result to the smallest value. Limiting the
   *     number of threads to {@link #MAX_PARALLEL_THREADS}, unless specified by {@link
   *     #parallelism}.
   *
   *     <p>For example, if there are 10 files but their combined size is less than 1 MB, using
   *     multiple threads would introduce more overhead than benefits.
   *
   *     <p>Ceil the number of threads based on the total bytes and the minimum bytes per thread, so
   *     it will add one thread for the remaining bytes. It doesn't use Math.ceil(double) because it
   *     looses precision. Example: 9223372036853727744L / 1mb = 8796093022207 with remainder 512,
   *     however when converting to double the remainder is not preserved given precision.
   *
   * @return The optimal number of threads required to process the given amount of files and bytes,
   *     or the given value at {@link #parallelism}. (Within the limits 1 and {@link
   *     #MAX_PARALLEL_THREADS})
   */
  private int computeParallelism(final DirectoryJarEntryContainer container) {
    // predefined number of threads
    if (parallelism > 0) {
      return parallelism;
    }
    // compute number of bytes to be compressed
    final long totalBytes =
        Math.max(1, container.getEntries().stream().mapToLong(FileJarEntry::getFileSize).sum());
    // calculate number of threads needed to process total of bytes
    final long numberOfThreads =
        (totalBytes / MIN_BYTES_PER_THREAD) + Math.min(totalBytes % MIN_BYTES_PER_THREAD, 1);
    // truncate values into the boundaries [1, MAX_PARALLEL_THREADS]
    return Math.toIntExact(Math.max(1, Math.min(numberOfThreads, MAX_PARALLEL_THREADS)));
  }

  private AbsPath buildJar(final JarPartition partition) {
    try {
      final AbsPath file = newTemporaryJarFile();
      final JarBuilder builder = builderSupplier.get();
      partition.forEach(builder::addEntry);
      builder.createJarFile(file.getPath());
      return file;
    } catch (IOException e) {
      throw new UncheckedIOException(e);
    }
  }

  /** Creates a temporary file that will be deleted after the JVM exits. */
  private AbsPath newTemporaryJarFile() throws IOException {
    String scratchPath = System.getenv("BUCK_SCRATCH_PATH");
    final Path scratchDir =
        Path.of(
            Strings.isNullOrEmpty(scratchPath)
                ? System.getProperty("java.io.tmpdir")
                : scratchPath);
    if (!isDirectory(scratchDir)) {
      throw new IOException("Scratch path must be an existing directory: " + scratchDir);
    }
    final Path temp = Files.createTempFile(scratchDir, "parallel-", ".jar").toAbsolutePath();
    temp.toFile().deleteOnExit();
    return AbsPath.of(temp);
  }

  /**
   * Represents a partition of a JAR file, containing a collection of files/entries and their total
   * size in bytes.
   */
  static class JarPartition implements Iterable<FileJarEntry> {

    /**
     * Compares two JarPartition instances based on their total size.
     *
     * @param a the first JarPartition instance
     * @param b the second JarPartition instance
     * @return 1 if a's total size is greater than b's, -1 otherwise
     */
    static int compareSize(final JarPartition a, final JarPartition b) {
      return a.totalSize > b.totalSize ? 1 : -1;
    }

    /** The total size of all files in this partition. */
    private long totalSize;

    /** The list of files in this partition. */
    private final List<FileJarEntry> files = new LinkedList<>();

    /**
     * Adds a file to this partition and updates the total size.
     *
     * @param file the file to add
     * @return this JarPartition instance
     */
    JarPartition add(FileJarEntry file) {
      this.files.add(file);
      this.totalSize += file.getFileSize();
      return this;
    }

    /**
     * Returns the list of files in this partition.
     *
     * @return the list of files
     */
    List<FileJarEntry> entries() {
      return files;
    }

    /**
     * Checks if this partition contains any files.
     *
     * @return true if this partition has files, false otherwise
     */
    boolean hasFiles() {
      return !entries().isEmpty();
    }

    @Override
    public Iterator<FileJarEntry> iterator() {
      return files.iterator();
    }

    @Override
    public String toString() {
      return "JarPartition{ files=" + files.size() + ", totalSize=" + totalSize + "}";
    }
  }
}
