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

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.io.CharStreams;
import java.io.Closeable;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.net.URI;
import java.net.URISyntaxException;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import javax.annotation.Nullable;
import javax.annotation.concurrent.GuardedBy;
import javax.tools.JavaCompiler;
import javax.tools.JavaFileObject;
import javax.tools.SimpleJavaFileObject;

@Nullsafe(Nullsafe.Mode.LOCAL)
class ZipEntryJavaFileObject extends SimpleJavaFileObject implements Closeable {

  private final ZipFile zipFile;
  private final ZipEntry zipEntry;

  @GuardedBy("this")
  @Nullable
  private String contents;

  public ZipEntryJavaFileObject(ZipFile zipFile, ZipEntry zipEntry) {
    super(createURIFromEntry(zipEntry), JavaFileObject.Kind.SOURCE);
    this.zipFile = zipFile;
    this.zipEntry = zipEntry;
  }

  /**
   * Creates a canonical URI that represents the {@link ZipEntry}. This URI starts with {@code
   * "string:///"} because {@link JavaCompiler} does not seem to tolerate URIs that start with
   * {@code "jar:///"}, even though that would be more appropriate.
   */
  private static URI createURIFromEntry(ZipEntry entry) {
    try {
      URI fileUri = new File(entry.getName()).toURI();
      return new URI("string", fileUri.getHost(), fileUri.getPath(), fileUri.getFragment());
    } catch (URISyntaxException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Returns the contents of the {@link ZipEntry} as a string. Ensures that the entry is read at
   * most once.
   */
  private synchronized String getContentsAsString() {
    if (contents != null) {
      return contents;
    }

    try (InputStream inputStream = Objects.requireNonNull(zipFile.getInputStream(zipEntry));
        InputStreamReader isr = new InputStreamReader(inputStream, StandardCharsets.UTF_8)) {
      contents = CharStreams.toString(isr);
    } catch (IOException e) {
      throw new RuntimeException(e);
    }

    return contents;
  }

  /** Closes the {@link ZipFile} this entry was loaded from. Use with care. */
  @Override
  public void close() throws IOException {
    zipFile.close();
  }

  @Override
  public CharSequence getCharContent(boolean ignoreEncodingErrors) {
    return getContentsAsString();
  }
}
