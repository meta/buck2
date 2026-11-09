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
import com.google.common.hash.Hashing;
import com.google.common.hash.HashingInputStream;
import com.google.common.io.ByteStreams;
import com.google.common.io.CharStreams;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.PrintWriter; // NOPMD required by API
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.stream.Stream;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.util.Textifier;
import org.objectweb.asm.util.TraceClassVisitor;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class JarDumper {
  private int asmFlags = 0;

  /**
   * Sets the flags that are passed to ASM's {@link ClassReader} when dumping class files. See
   * {@link ClassReader#accept(ClassVisitor, int)};
   *
   * @param asmFlags
   * @return
   */
  public JarDumper setAsmFlags(int asmFlags) {
    this.asmFlags = asmFlags;
    return this;
  }

  public List<String> dump(Path jarPath) throws IOException {
    List<String> result = new ArrayList<>();
    result.add("Directory:");
    try (JarFile abiJar = new JarFile(jarPath.toFile())) {
      abiJar.stream().map(JarEntry::toString).forEach(result::add);

      result.add("");
      result.add("Contents:");
      abiJar.stream()
          .flatMap(
              entry ->
                  Stream.concat(
                      Stream.of(String.format("%s:", entry.getName())),
                      Stream.concat(dumpEntry(abiJar, entry), Stream.of(""))))
          .forEach(result::add);
    }

    return result;
  }

  public Stream<String> dumpEntry(JarFile file, JarEntry entry) {
    try (InputStream inputStream = file.getInputStream(entry)) {
      String fileName = entry.getName();
      if (fileName.endsWith(".class")) {
        return dumpClassFile(inputStream);
      } else if (isTextFile(fileName)) {
        return dumpTextFile(inputStream);
      } else {
        return dumpBinaryFile(fileName, inputStream);
      }
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  protected static boolean isTextFile(String fileName) {
    return fileName.equals(JarFile.MANIFEST_NAME)
        || fileName.endsWith(".java")
        || fileName.endsWith(".json")
        || fileName.endsWith(".txt");
  }

  private Stream<String> dumpClassFile(InputStream stream) throws IOException {
    byte[] textifiedClass;
    try (ByteArrayOutputStream bos = new ByteArrayOutputStream();
        PrintWriter pw = new PrintWriter(bos)) { // NOPMD required by API
      ClassReader reader = new ClassReader(stream);
      TraceClassVisitor traceVisitor = new TraceClassVisitor(null, new Textifier(), pw);
      reader.accept(traceVisitor, asmFlags);
      textifiedClass = bos.toByteArray();
    }

    try (InputStreamReader streamReader =
        new InputStreamReader(new ByteArrayInputStream(textifiedClass))) {
      return CharStreams.readLines(streamReader).stream();
    }
  }

  private Stream<String> dumpTextFile(InputStream inputStream) throws IOException {
    try (InputStreamReader streamReader = new InputStreamReader(inputStream)) {
      return CharStreams.readLines(streamReader).stream();
    }
  }

  private Stream<String> dumpBinaryFile(String name, InputStream inputStream) throws IOException {
    try (HashingInputStream is = new HashingInputStream(Hashing.murmur3_128(), inputStream)) {
      ByteStreams.exhaust(is);
      return Stream.of(String.format("Murmur3-128 of %s: %s", name, is.hash().toString()));
    }
  }
}
