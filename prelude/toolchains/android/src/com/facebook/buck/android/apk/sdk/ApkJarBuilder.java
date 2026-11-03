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
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import java.io.BufferedOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Calendar;
import java.util.Objects;
import java.util.Set;
import java.util.jar.JarEntry;
import java.util.jar.JarOutputStream;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import org.jetbrains.annotations.Nullable;

/** A Jar file builder. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ApkJarBuilder {
  private static final long DOS_FAKE_TIME;

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
  @Nullable private JarOutputStream mOutputJar;
  private byte[] mBuffer = new byte[256 * 1024];
  private boolean mPackageMetaInfVersionFiles;

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
    mOutputJar = new JarOutputStream(new BufferedOutputStream(out));
    mOutputJar.setLevel(4);
    mPackageMetaInfVersionFiles = packageMetaInfVersionFiles;
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
      JarEntry entry = new JarEntry(jarPath);
      entry.setTime(DOS_FAKE_TIME);

      writeEntry(fis, entry);
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
    JarEntry entry = new JarEntry(jarPath);
    entry.setTime(DOS_FAKE_TIME);
    entry.setMethod(ZipEntry.STORED);
    entry.setSize(inputFile.length());
    entry.setCompressedSize(inputFile.length());
    entry.setCrc(crc32(inputFile));

    try (FileInputStream fis = new FileInputStream(inputFile)) {
      writeEntry(fis, entry);
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

        JarEntry newEntry;

        // Preserve the STORED method of the input entry.
        if (entry.getMethod() == JarEntry.STORED) {
          newEntry = new JarEntry(entry);
        } else {
          // Create a new entry so that the compressed len is recomputed.
          newEntry = new JarEntry(name);
        }

        newEntry.setTime(DOS_FAKE_TIME);
        writeEntry(zis, newEntry);

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
    Objects.requireNonNull(mOutputJar).close();
    mOutputJar = null;
  }

  /**
   * Clean up of the builder for interrupted workflow. This does nothing if {@link #close()} was
   * called successfully.
   */
  public void cleanUp() {
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
   * @param input The input stream from where to write the entry content.
   * @param entry the entry to write in the jar.
   * @throws IOException
   */
  private void writeEntry(InputStream input, JarEntry entry) throws IOException {
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
}
