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

import com.facebook.buck.util.io.IoUtil;
import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipOutputStreams {

  /** The size of the BufferedOutputStream used to wrap the underlying file. */
  private static final int FILE_BUFFER_SIZE = 512 * 1024;

  private ZipOutputStreams() {
    // factory class
  }

  /**
   * Create a new {@link CustomZipOutputStream} that outputs to the given {@code zipFile}. Note that
   * the parent directory of the {@code zipFile} must exist already. The returned stream will throw
   * an exception should duplicate entries be added.
   *
   * @param zipFile The file to write to.
   */
  public static CustomZipOutputStream newOutputStream(Path zipFile) throws IOException {
    return IoUtil.mapJustOpened(
        Files.newOutputStream(zipFile),
        s -> newOutputStream(new BufferedOutputStream(s, FILE_BUFFER_SIZE)));
  }

  /**
   * Create a new {@link CustomZipOutputStream} that will by default act in the same way as {@link
   * java.util.zip.ZipOutputStream}, notably by throwing an exception if duplicate entries are
   * added.
   *
   * @param out The output stream to write to.
   */
  public static CustomZipOutputStream newOutputStream(OutputStream out) {
    return newOutputStream(out, HandleDuplicates.THROW_EXCEPTION);
  }

  public static CustomJarOutputStream newJarOutputStream(OutputStream out) {
    return newJarOutputStream(out, HandleDuplicates.THROW_EXCEPTION);
  }

  /**
   * Create a new {@link CustomZipOutputStream} that handles duplicate entries in the way dictated
   * by {@code mode}.
   *
   * @param zipFile The file to write to.
   * @param mode How to handle duplicate entries.
   */
  public static CustomZipOutputStream newOutputStream(Path zipFile, HandleDuplicates mode)
      throws IOException {
    return IoUtil.mapJustOpened(
        Files.newOutputStream(zipFile),
        s -> newOutputStream(new BufferedOutputStream(s, FILE_BUFFER_SIZE), mode));
  }

  /** Open jar output stream. */
  public static CustomJarOutputStream newJarOutputStream(Path jarFile, HandleDuplicates mode)
      throws IOException {
    return IoUtil.mapJustOpened(
        Files.newOutputStream(jarFile),
        s -> newJarOutputStream(new BufferedOutputStream(s, FILE_BUFFER_SIZE), mode));
  }

  /** Open jar output stream that will to append entries on top of an input jar. */
  public static CustomJarOutputStream appendJarOutputStream(Path inputJar, Path outputJar)
      throws IOException {

    final IncrementalJarOutputStreamImpl incrementalJar =
        IncrementalJarOutputStreamImpl.createIncrementalJar(inputJar, outputJar);

    return IoUtil.mapJustOpened(
        incrementalJar.getDelegate(), s -> new CustomJarOutputStream(incrementalJar));
  }

  /**
   * Create a new {@link CustomZipOutputStream} that handles duplicate entries in the way dictated
   * by {@code mode}.
   *
   * @param out The output stream to write to.
   * @param mode How to handle duplicate entries.
   */
  public static CustomZipOutputStream newOutputStream(OutputStream out, HandleDuplicates mode) {
    return new CustomZipOutputStream(newImpl(out, mode));
  }

  public static CustomJarOutputStream newJarOutputStream(OutputStream out, HandleDuplicates mode) {
    return new CustomJarOutputStream(newImpl(out, mode));
  }

  public static CustomZipOutputStream newSimpleOutputStream(OutputStream out) {
    return new CustomZipOutputStream(new SimpleZipOutputStreamImpl(out));
  }

  protected static CustomZipOutputStream.Impl newImpl(OutputStream out, HandleDuplicates mode) {
    CustomZipOutputStream.Impl impl;
    switch (mode) {
      case APPEND_TO_ZIP:
      case THROW_EXCEPTION:
        impl = new AppendingZipOutputStreamImpl(out, mode == HandleDuplicates.THROW_EXCEPTION);
        break;
      case OVERWRITE_EXISTING:
        impl = new OverwritingZipOutputStreamImpl(out);
        break;
      default:
        throw new RuntimeException(
            String.format("Unable to determine which zip output mode to use: %s", mode));
    }

    return impl;
  }

  public enum HandleDuplicates {
    /** Duplicate entries are simply appended to the zip. */
    APPEND_TO_ZIP,
    /** An exception should be thrown if a duplicate entry is added to a zip. */
    THROW_EXCEPTION,
    /** A duplicate entry overwrites an existing entry with the same name. */
    OVERWRITE_EXISTING
  }
}
