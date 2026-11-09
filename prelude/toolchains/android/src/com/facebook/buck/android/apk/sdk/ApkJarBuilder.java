/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk.sdk;

import com.facebook.buck.android.apk.sdk.ApkJarBuilder.IZipEntryFilter.ZipAbortException;
import com.facebook.buck.util.zip.CustomZipEntry;
import com.facebook.buck.util.zip.CustomZipOutputStream;
import com.facebook.buck.util.zip.DeflatedZipEntries;
import com.facebook.buck.util.zip.ZipOutputStreams;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import java.io.BufferedOutputStream;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FilterInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.SequenceInputStream;
import java.util.ArrayDeque;
import java.util.Calendar;
import java.util.HashSet;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipException;
import java.util.zip.ZipInputStream;
import org.jetbrains.annotations.Nullable;

/** A Jar file builder. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ApkJarBuilder {
  private static final long DOS_FAKE_TIME;
  private static final int COMPRESSION_LEVEL = 4;

  /**
   * {@link java.util.zip.ZipEntry#setTime(long)} is timezone-sensitive. Use this value instead of a
   * hardcoded constant to produce timezone-agnostic .zip files.
   *
   * @return time in milliseconds that represents a fixed date in the current timezone.
   */
  static {
    // Using any timestamp before 1980 (which is supposedly the earliest time supported by the .zip
    // format) causes files created by JRE 1.7 to be different than JRE 1.8. This is because 1.8
    // actually supports setting timestamps to earlier than 1980, whereas 1.7 rounds up to 1980.
    // We also need to resort to the deprecated date constructor as the ZipEntry uses deprecated
    // Date methods that depend on the current timezone.
    // Finally 1980.01.01 doesn't work across all timezones across Java 1.7 and 1.8. Fun times.
    Calendar c = Calendar.getInstance();
    c.set(1985, Calendar.FEBRUARY, 1, 0, 0, 0);
    DOS_FAKE_TIME = c.getTimeInMillis();
  }

  public static final Set<String> ALLOWLISTED_META_INF_SERVICES_FILES =
      ImmutableSet.of(
          "META-INF/services/kotlin.reflect.jvm.internal.impl.builtins.BuiltInsLoader",
          "META-INF/services/kotlin.reflect.jvm.internal.impl.km.internal.extensions.MetadataExtensions",
          "META-INF/services/kotlinx.coroutines.CoroutineExceptionHandler",
          "META-INF/androidx.compose.ui_ui.version");
  @Nullable private CustomZipOutputStream mOutputJar;
  private byte[] mBuffer = new byte[256 * 1024];
  private boolean mPackageMetaInfVersionFiles;

  // Entry data held in memory while entries wait to be written, plain and deflated.
  private static final long MAX_BUDGET = 512L << 20;

  private final long mBudget;
  // An entry larger than this is deflated on the calling thread instead.
  private final int mMaxBufferedEntry;
  @Nullable private final ExecutorService mPool;
  private final ArrayDeque<PendingEntry> mPending = new ArrayDeque<>();
  private final Set<String> mNames = new HashSet<>();
  private long mPendingBytes;

  /**
   * Classes which implement this interface provides a method to check whether a file should be
   * added to a Jar file.
   */
  public interface IZipEntryFilter {

    /**
     * An exception thrown during packaging of a zip file into APK file. This is typically thrown by
     * implementations of {@link IZipEntryFilter#checkEntry(String)}.
     */
    class ZipAbortException extends Exception {
      private static final long serialVersionUID = 1L;

      public ZipAbortException() {
        super();
      }

      public ZipAbortException(String format, Object... args) {
        super(String.format(format, args));
      }

      public ZipAbortException(Throwable cause, String format, Object... args) {
        super(String.format(format, args), cause);
      }

      public ZipAbortException(Throwable cause) {
        super(cause);
      }
    }

    /**
     * Checks a file for inclusion in a Jar archive.
     *
     * @param archivePath the archive file path of the entry
     * @return <code>true</code> if the file should be included.
     * @throws ZipAbortException if writing the file should be aborted.
     */
    boolean checkEntry(String archivePath) throws ZipAbortException;
  }

  /**
   * Creates a {@link ApkJarBuilder} with a given output stream.
   *
   * @param out the {@link OutputStream} where to write the Jar archive.
   * @throws IOException
   */
  public ApkJarBuilder(OutputStream out, boolean packageMetaInfVersionFiles) throws IOException {
    this(out, packageMetaInfVersionFiles, 0);
  }

  /**
   * Creates a {@link ApkJarBuilder} that deflates entries on {@code compressionThreads} threads.
   * The archive is byte-identical to the one written with no threads.
   *
   * @param compressionThreads threads that deflate entries; 0 deflates every entry on the calling
   *     thread
   */
  public ApkJarBuilder(OutputStream out, boolean packageMetaInfVersionFiles, int compressionThreads)
      throws IOException {
    this(
        out,
        packageMetaInfVersionFiles,
        compressionThreads,
        defaultBudget(),
        (int) Math.min(Integer.MAX_VALUE - 16, defaultBudget() / 2));
  }

  private static long defaultBudget() {
    return Math.min(Runtime.getRuntime().maxMemory() / 4, MAX_BUDGET);
  }

  /**
   * @param budget bytes of entry data, plain and deflated, held in memory while entries wait
   * @param maxBufferedEntry the largest entry deflated on a worker; larger ones are deflated on the
   *     calling thread
   */
  ApkJarBuilder(
      OutputStream out,
      boolean packageMetaInfVersionFiles,
      int compressionThreads,
      long budget,
      int maxBufferedEntry)
      throws IOException {
    mOutputJar = ZipOutputStreams.newOutputStream(new BufferedOutputStream(out));
    mPackageMetaInfVersionFiles = packageMetaInfVersionFiles;
    mBudget = budget;
    mMaxBufferedEntry = maxBufferedEntry;
    mPool =
        compressionThreads > 0
            ? Executors.newFixedThreadPool(compressionThreads, ApkJarBuilder::daemonThread)
            : null;
  }

  private static Thread daemonThread(Runnable r) {
    Thread t = new Thread(r, "apk-builder-deflate");
    t.setDaemon(true);
    return t;
  }

  /**
   * Writes a new {@link File} into the archive.
   *
   * @param inputFile the {@link File} to write.
   * @param jarPath the filepath inside the archive.
   * @throws IOException
   */
  public void writeFile(File inputFile, String jarPath) throws IOException {
    // Get an input stream on the file.
    FileInputStream fis = new FileInputStream(inputFile);
    try {

      // create the zip entry
      CustomZipEntry entry = new CustomZipEntry(jarPath);
      entry.setTime(DOS_FAKE_TIME);

      writeEntry(fis, entry, inputFile.length());
    } finally {
      // close the file stream used to read the file
      fis.close();
    }
  }

  /**
   * Writes a new {@link File} into the archive without compressing it.
   *
   * @param inputFile the {@link File} to write.
   * @param jarPath the filepath inside the archive.
   * @throws IOException
   */
  public void writeStoredFile(File inputFile, String jarPath) throws IOException {
    CustomZipEntry entry = new CustomZipEntry(jarPath);
    entry.setTime(DOS_FAKE_TIME);
    entry.setMethod(ZipEntry.STORED);
    entry.setSize(inputFile.length());
    entry.setCompressedSize(inputFile.length());
    entry.setCrc(crc32(inputFile));

    try (FileInputStream fis = new FileInputStream(inputFile)) {
      writeEntry(fis, entry, inputFile.length());
    }
  }

  private long crc32(File inputFile) throws IOException {
    CRC32 crc = new CRC32();
    try (FileInputStream fis = new FileInputStream(inputFile)) {
      int count;
      while ((count = fis.read(mBuffer)) != -1) {
        crc.update(mBuffer, 0, count);
      }
    }
    return crc.getValue();
  }

  /**
   * Copies the content of a Jar/Zip archive into the receiver archive.
   *
   * <p>An optional {@link IZipEntryFilter} allows to selectively choose which files to copy over.
   *
   * @param input the {@link InputStream} for the Jar/Zip to copy.
   * @param filter the filter or <code>null</code>
   * @throws IOException
   * @throws ZipAbortException if the {@link IZipEntryFilter} filter indicated that the write must
   *     be aborted.
   */
  public void writeZip(InputStream input, IZipEntryFilter filter)
      throws IOException, ZipAbortException {
    ZipInputStream zis = new ZipInputStream(input);

    try {
      // loop on the entries of the intermediary package and put them in the final package.
      ZipEntry entry;
      while ((entry = zis.getNextEntry()) != null) {
        String name = entry.getName();

        // do not take directories
        if (entry.isDirectory()) {
          continue;
        }

        // Do not take anything inside a potential META-INF folder, except for some
        // META-INF/services files that we know we need.
        // TODO(T139016417) This hack is only necessary because we are using an old and deprecated
        // version of this class. We should migrate to Android-Builder instead.
        if (name.startsWith("META-INF/")) {
          if (!isMetaInfFileAllowed(name)) {
            continue;
          }

          // if we have a filter, we check the entry against it
        } else if (filter != null && filter.checkEntry(name) == false) {
          continue;
        }

        CustomZipEntry newEntry;

        // Preserve the STORED method of the input entry.
        if (entry.getMethod() == ZipEntry.STORED) {
          newEntry = new CustomZipEntry(entry);
        } else {
          // Create a new entry so that the compressed len is recomputed.
          newEntry = new CustomZipEntry(name);
        }

        newEntry.setTime(DOS_FAKE_TIME);
        writeEntry(zis, newEntry, entry.getSize());

        zis.closeEntry();
      }
    } finally {
      zis.close();
    }
  }

  private boolean isMetaInfFileAllowed(String name) {
    if (ALLOWLISTED_META_INF_SERVICES_FILES.contains(name)) {
      return true;
    }
    if (mPackageMetaInfVersionFiles && name.endsWith(".version")) {
      return true;
    }
    return false;
  }

  /**
   * Closes the Jar archive.
   *
   * @throws IOException
   */
  public void close() throws IOException {
    try {
      drain(-1);
      Objects.requireNonNull(mOutputJar).close();
      mOutputJar = null;
    } finally {
      if (mPool != null) {
        mPool.shutdown();
      }
    }
  }

  /**
   * Clean up of the builder for interrupted workflow. This does nothing if {@link #close()} was
   * called successfully.
   */
  public void cleanUp() {
    for (PendingEntry p : mPending) {
      if (p.deflated != null) {
        p.deflated.cancel(true);
      }
    }
    mPending.clear();
    mPendingBytes = 0;
    if (mPool != null) {
      mPool.shutdownNow();
    }
    if (mOutputJar != null) {
      try {
        mOutputJar.close();
      } catch (IOException e) {
        // pass
      }
    }
  }

  /**
   * Adds an entry to the output jar, and write its content from the {@link InputStream}
   *
   * <p>With compression threads, the entry is read here and deflated on a worker. It is written
   * once it and every entry before it are ready, so the archive keeps the order of the calls.
   *
   * @param input The input stream from where to write the entry content.
   * @param entry the entry to write in the jar.
   * @param sizeHint the entry's length when the caller knows it, otherwise -1
   * @throws IOException
   */
  private void writeEntry(InputStream input, CustomZipEntry entry, long sizeHint)
      throws IOException {
    if (mPool == null) {
      writeNow(input, entry);
      return;
    }
    // The output rejects a repeated name when the entry is written; reject it here, while the
    // caller that added it is still on the stack, rather than when the queue reaches it.
    if (!mNames.add(entry.getName())) {
      throw new ZipException("duplicate entry: " + entry.getName());
    }
    BoundedBytes data = BoundedBytes.read(input, mMaxBufferedEntry, sizeHint, mBuffer);
    if (!data.complete) {
      drain(-1);
      // SequenceInputStream closes each stream it finishes; the caller still owns input.
      InputStream rest =
          new FilterInputStream(input) {
            @Override
            public void close() {}
          };
      writeNow(
          new SequenceInputStream(new ByteArrayInputStream(data.bytes, 0, data.length), rest),
          entry);
      return;
    }
    PendingEntry p = new PendingEntry(entry, data.bytes, data.length);
    if (entry.getMethod() != ZipEntry.STORED) {
      p.deflated =
          Objects.requireNonNull(mPool)
              .submit(
                  () ->
                      DeflatedZipEntries.deflate(
                          new ByteArrayInputStream(p.data, 0, p.length), COMPRESSION_LEVEL));
    }
    mPending.add(p);
    mPendingBytes += p.cost;
    drainReady();
    drain(mBudget);
  }

  /** Writes an entry on the calling thread. */
  private void writeNow(InputStream input, CustomZipEntry entry) throws IOException {
    if (entry.getMethod() != ZipEntry.STORED) {
      entry.setCompressionLevel(COMPRESSION_LEVEL);
      DeflatedZipEntries.write(Objects.requireNonNull(mOutputJar), entry, input);
      return;
    }

    // add the entry to the jar archive
    Objects.requireNonNull(mOutputJar).putNextEntry(entry);

    // read the content of the entry from the input stream, and write it into the archive.
    int count;
    while ((count = input.read(mBuffer)) != -1) {
      Objects.requireNonNull(mOutputJar).write(mBuffer, 0, count);
    }

    // close the entry for this file
    Objects.requireNonNull(mOutputJar).closeEntry();
  }

  /** Writes queued entries from the head that need no more work, without waiting. */
  private void drainReady() throws IOException {
    while (!mPending.isEmpty()) {
      Future<DeflatedZipEntries.Deflated> head =
          Objects.requireNonNull(mPending.peekFirst()).deflated;
      if (head != null && !head.isDone()) {
        return;
      }
      writeHead();
    }
  }

  /**
   * Writes queued entries in order until at most {@code budget} bytes are queued; -1 writes all.
   */
  private void drain(long budget) throws IOException {
    while (!mPending.isEmpty() && mPendingBytes > budget) {
      writeHead();
    }
  }

  private void writeHead() throws IOException {
    PendingEntry p = mPending.removeFirst();
    mPendingBytes -= p.cost;
    CustomZipOutputStream out = Objects.requireNonNull(mOutputJar);
    try {
      Future<DeflatedZipEntries.Deflated> deflated = p.deflated;
      if (deflated == null) {
        out.putNextEntry(p.entry);
        out.write(p.data, 0, p.length);
        out.closeEntry();
        return;
      }
      DeflatedZipEntries.Deflated d;
      try {
        d = deflated.get();
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        throw new IOException("Interrupted while deflating", e);
      } catch (ExecutionException e) {
        throw new IOException("Failed to deflate", e.getCause());
      }
      p.entry.setCompressionLevel(COMPRESSION_LEVEL);
      DeflatedZipEntries.write(out, p.entry, d);
    } catch (IOException e) {
      throw new IOException("Failed to write " + p.entry.getName() + ": " + e.getMessage(), e);
    }
  }

  private static final class PendingEntry {
    final CustomZipEntry entry;
    final byte[] data;
    final int length;
    final long cost;
    @Nullable Future<DeflatedZipEntries.Deflated> deflated;

    PendingEntry(CustomZipEntry entry, byte[] data, int length) {
      this.entry = entry;
      this.data = data;
      this.length = length;
      // The plain bytes, and for a deflated entry at most about as many deflated ones.
      this.cost = entry.getMethod() == ZipEntry.STORED ? length : 2L * length;
    }
  }

  /** The bytes of a stream up to a limit, and whether the stream ended within it. */
  private static final class BoundedBytes {
    final byte[] bytes;
    final int length;
    final boolean complete;

    private BoundedBytes(byte[] bytes, int length, boolean complete) {
      this.bytes = bytes;
      this.length = length;
      this.complete = complete;
    }

    /**
     * Reads up to {@code limit} bytes, plus one more to learn whether the stream goes on, into an
     * array sized from {@code sizeHint} when the caller knows the length.
     */
    static BoundedBytes read(InputStream input, int limit, long sizeHint, byte[] buffer)
        throws IOException {
      ExposedBuffer out =
          new ExposedBuffer(sizeHint >= 0 ? (int) Math.min(sizeHint + 1, limit + 1L) : 8192);
      int count;
      while (out.size() <= limit
          && (count = input.read(buffer, 0, (int) Math.min(buffer.length, limit + 1L - out.size())))
              != -1) {
        out.write(buffer, 0, count);
      }
      return new BoundedBytes(out.array(), out.size(), out.size() <= limit);
    }
  }

  /** A ByteArrayOutputStream whose array is used in place, so an entry is not copied again. */
  private static final class ExposedBuffer extends ByteArrayOutputStream {
    ExposedBuffer(int size) {
      super(size);
    }

    byte[] array() {
      return buf;
    }
  }
}
