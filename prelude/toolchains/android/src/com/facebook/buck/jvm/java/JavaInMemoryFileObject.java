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

import com.facebook.buck.util.zip.CustomZipEntry;
import com.facebook.buck.util.zip.JarBuilder;
import com.facebook.buck.util.zip.JarEntrySupplier;
import com.facebook.infer.annotation.Nullsafe;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.OutputStreamWriter;
import java.io.Reader;
import java.io.Writer;
import java.net.URI;
import javax.tools.SimpleJavaFileObject;

/**
 * A {@link SimpleJavaFileObject} implementation that forwards the content of the file to a Jar
 * output stream instead of writing it to disk. Since the Jar can be shared between multiple
 * threads, a semaphore is used to ensure exclusive access to the output stream.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavaInMemoryFileObject extends JarFileObject {
  // Bump the initial buffer size because usual file sizes using this are way more than 4K and the
  // default buffer size of a ByteArrayOutputStream is just 32
  private static final int BUFFER_SIZE = 4096;
  private static final String ALREADY_OPENED = "Output stream or writer has already been opened.";

  private boolean isOpened = false;
  private boolean isWritten = false;
  private final ByteArrayOutputStream bos = new ByteArrayOutputStream(BUFFER_SIZE);

  public JavaInMemoryFileObject(URI uri, String pathInJar, Kind kind) {
    super(uri, pathInJar, kind);
  }

  @Override
  public InputStream openInputStream() throws IOException {
    if (!isWritten) {
      throw new FileNotFoundException(uri.toString());
    }
    return new ByteArrayInputStream(bos.toByteArray());
  }

  @Override
  public synchronized OutputStream openOutputStream() throws IOException {
    if (isOpened) {
      throw new IOException(ALREADY_OPENED);
    }
    isOpened = true;
    return new OutputStream() {
      @Override
      public void write(int b) {
        bos.write(b);
      }

      @Override
      public void close() throws IOException {
        bos.close();
        isWritten = true;
      }
    };
  }

  @Override
  public Reader openReader(boolean ignoreEncodingErrors) throws IOException {
    return new InputStreamReader(openInputStream());
  }

  @Override
  public CharSequence getCharContent(boolean ignoreEncodingErrors) throws IOException {
    if (!isWritten) {
      throw new FileNotFoundException(uri.toString());
    }
    return bos.toString();
  }

  @Override
  public Writer openWriter() throws IOException {
    return new OutputStreamWriter(this.openOutputStream());
  }

  @Override
  public void writeToJar(JarBuilder jarBuilder) {
    if (!isWritten) {
      // Nothing was written to this file, so it doesn't really exist.
      return;
    }
    jarBuilder.addEntry(new JarEntrySupplier(new CustomZipEntry(getName()), this::openInputStream));
  }
}
