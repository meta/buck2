/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;

import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.jar.JarOutputStream;
import java.util.zip.ZipEntry;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class MultiDexExecutableMainTest {
  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  @Test
  public void skipsEmptySplitJarsAndPreservesOtherInputsInOrder() throws Exception {
    Path emptyJar = tmp.newFile("empty.jar").toPath();
    try (JarOutputStream jar = new JarOutputStream(Files.newOutputStream(emptyJar))) {}
    Path nonemptyJar = tmp.newFile("nonempty.jar").toPath();
    try (JarOutputStream jar = new JarOutputStream(Files.newOutputStream(nonemptyJar))) {
      jar.putNextEntry(new ZipEntry("Example.class"));
      jar.write(new byte[] {1, 2, 3});
      jar.closeEntry();
    }
    Path dex = tmp.newFile("classes.dex").toPath();
    Files.write(dex, new byte[22]);
    Path directory = tmp.newFolder("classes").toPath();
    Path inputs = writeInputList(emptyJar, nonemptyJar, dex, directory, emptyJar, nonemptyJar);

    assertEquals(
        ImmutableList.of(nonemptyJar, dex, directory, nonemptyJar),
        MultiDexExecutableMain.readInputPaths(inputs));
  }

  @Test
  public void skipsMinimumSizeArchivesBasedOnSizeAlone() throws Exception {
    Path invalidJar = tmp.newFile("invalid.jar").toPath();
    Files.write(invalidJar, new byte[22]);
    Path inputs = writeInputList(invalidJar);

    assertEquals(ImmutableList.of(), MultiDexExecutableMain.readInputPaths(inputs));
  }

  @Test
  public void reportsMissingJars() throws Exception {
    Path inputs = writeInputList(tmp.getRoot().toPath().resolve("missing.jar"));

    assertThrows(IOException.class, () -> MultiDexExecutableMain.readInputPaths(inputs));
  }

  private Path writeInputList(Path... paths) throws IOException {
    Path list = tmp.newFile("inputs.txt").toPath();
    ImmutableList.Builder<String> lines = ImmutableList.builder();
    for (Path path : paths) {
      lines.add(path.toString());
    }
    Files.write(list, lines.build());
    return list;
  }
}
