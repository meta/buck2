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

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.android.tools.r8.ByteDataView;
import com.android.tools.r8.ProgramResource;
import com.android.tools.r8.ProgramResourceProvider;
import com.android.tools.r8.ResourceException;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.EnumSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/** Tests for {@link D8Utils#moveSingleDexOutput}. */
public class D8UtilsTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private static final Set<D8Options> FAIL_ON_MULTIPLE = EnumSet.of(D8Options.FAIL_ON_MULTIPLE_DEX);

  private static final Set<D8Options> NO_FAIL = EnumSet.noneOf(D8Options.class);

  @Test
  public void testMoveSingleDexOutput_singleFile() throws Exception {
    Path d8OutputDir = tmp.newFolder("d8-output").toPath();
    Files.write(d8OutputDir.resolve("classes.dex"), new byte[] {1, 2, 3});

    Path dest = tmp.getRoot().toPath().resolve("final.dex");
    D8Utils.moveSingleDexOutput(d8OutputDir, dest, FAIL_ON_MULTIPLE);

    assertTrue("Destination file should exist", Files.exists(dest));
    assertEquals("File content should match", 3, Files.size(dest));
  }

  @Test
  public void testMoveSingleDexOutput_emptyDir() throws Exception {
    Path d8OutputDir = tmp.newFolder("d8-empty").toPath();
    Path dest = tmp.getRoot().toPath().resolve("final.dex");

    D8Utils.moveSingleDexOutput(d8OutputDir, dest, FAIL_ON_MULTIPLE);

    assertFalse("Destination should not exist for empty input", Files.exists(dest));
  }

  @Test
  public void testMoveSingleDexOutput_multipleFiles_throwsWithAllFilenames() throws Exception {
    Path d8OutputDir = tmp.newFolder("d8-multi").toPath();
    Files.write(d8OutputDir.resolve("classes.dex"), new byte[] {1});
    Files.write(d8OutputDir.resolve("classes2.dex"), new byte[] {2});
    Files.write(d8OutputDir.resolve("classes3.dex"), new byte[] {3});

    Path dest = tmp.getRoot().toPath().resolve("final.dex");

    try {
      D8Utils.moveSingleDexOutput(d8OutputDir, dest, FAIL_ON_MULTIPLE);
      fail("Expected IllegalStateException for multiple DEX files");
    } catch (IllegalStateException e) {
      String msg = e.getMessage();
      assertTrue("Error should mention file count: " + msg, msg.contains("3 DEX files"));
      assertTrue("Error should list classes.dex: " + msg, msg.contains("classes.dex"));
      assertTrue("Error should list classes2.dex: " + msg, msg.contains("classes2.dex"));
      assertTrue("Error should list classes3.dex: " + msg, msg.contains("classes3.dex"));
      assertTrue("Error should suggest split_dex: " + msg, msg.contains("split_dex"));
    }
    assertFalse("Destination should not be written on error", Files.exists(dest));
  }

  @Test
  public void testMoveSingleDexOutput_twoFiles_throwsWithSplitDexHint() throws Exception {
    Path d8OutputDir = tmp.newFolder("d8-two").toPath();
    Files.write(d8OutputDir.resolve("classes.dex"), new byte[] {1});
    Files.write(d8OutputDir.resolve("classes2.dex"), new byte[] {2});

    Path dest = tmp.getRoot().toPath().resolve("final.dex");

    try {
      D8Utils.moveSingleDexOutput(d8OutputDir, dest, FAIL_ON_MULTIPLE);
      fail("Expected IllegalStateException for multiple DEX files");
    } catch (IllegalStateException e) {
      assertTrue(
          "Error should suggest use_split_dex: " + e.getMessage(),
          e.getMessage().contains("use_split_dex = True"));
    }
  }

  @Test
  public void testMoveSingleDexOutput_multipleFiles_noFailOption_movesFirstFile() throws Exception {
    Path d8OutputDir = tmp.newFolder("d8-multi-nofail").toPath();
    Files.write(d8OutputDir.resolve("classes.dex"), new byte[] {1, 2, 3});
    Files.write(d8OutputDir.resolve("classes2.dex"), new byte[] {4, 5});

    Path dest = tmp.getRoot().toPath().resolve("final.dex");
    D8Utils.moveSingleDexOutput(d8OutputDir, dest, NO_FAIL);

    assertTrue("Destination file should exist", Files.exists(dest));
    assertEquals("Should move first file (classes.dex)", 3, Files.size(dest));
  }

  @Test
  public void testSelectedDexUnitsExcludeOtherContexts() throws Exception {
    Path archive = tmp.getRoot().toPath().resolve("library.dex.jar");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(archive))) {
      for (String name : new String[] {"com/Alpha.dex", "com/AlphaSecondary.dex"}) {
        zip.putNextEntry(new ZipEntry(name));
        zip.write(new byte[] {1, 2, 3});
        zip.closeEntry();
      }
    }
    D8Utils.writeDexUnitMetadata(
        archive,
        java.util.List.of(
            "com/Alpha\t3\t0\t0\t0\tcom/Alpha\tcom/Alpha$$Backport0",
            "com/AlphaSecondary\t3\t0\t0\t0\tcom/AlphaSecondary"));
    ProgramResourceProvider provider =
        D8Utils.createSelectedDexProvider(
            archive, Set.of("com/Alpha.class", "com/Alpha$$Backport0.class"));
    List<ProgramResource> resources = List.copyOf(provider.getProgramResources());
    assertEquals(1, resources.size());
    ProgramResource resource = resources.get(0);
    assertEquals("com/Alpha.dex", resource.getOrigin().part());
    assertEquals(ProgramResource.Kind.DEX, resource.getKind());
    try (InputStream stream = resource.getByteStream()) {
      assertArrayEquals(new byte[] {1, 2, 3}, stream.readAllBytes());
    }
    provider.finished(new D8Utils.D8DiagnosticsHandler());
  }

  @Test
  public void testSelectedDexProviderDefersReadingUntilRequested() throws Exception {
    Path archive = tmp.getRoot().toPath().resolve("deferred.dex.jar");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(archive))) {
      zip.putNextEntry(new ZipEntry("com/Alpha.dex"));
      zip.write(new byte[] {1, 2, 3});
      zip.closeEntry();
    }
    D8Utils.writeDexUnitMetadata(archive, List.of("com/Alpha\t3\t0\t0\t0\tcom/Alpha"));
    ProgramResourceProvider provider =
        D8Utils.createSelectedDexProvider(archive, Set.of("com/Alpha.class"));
    Files.delete(archive);
    try {
      provider.getProgramResources();
      fail("Expected the provider to read the archive only when D8 requests resources");
    } catch (ResourceException expected) {
      assertEquals(archive.toString(), expected.getOrigin().part());
    } finally {
      provider.finished(new D8Utils.D8DiagnosticsHandler());
    }
  }

  @Test
  public void testSelectedDexProviderCanReloadResources() throws Exception {
    Path archive = tmp.getRoot().toPath().resolve("reload.dex.jar");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(archive))) {
      zip.putNextEntry(new ZipEntry("com/Alpha.dex"));
      zip.write(new byte[] {4, 5});
      zip.closeEntry();
    }
    D8Utils.writeDexUnitMetadata(archive, List.of("com/Alpha\t2\t0\t0\t0\tcom/Alpha"));
    ProgramResourceProvider provider =
        D8Utils.createSelectedDexProvider(archive, Set.of("com/Alpha.class"));
    for (int i = 0; i < 2; i++) {
      List<ProgramResource> resources = List.copyOf(provider.getProgramResources());
      assertEquals(1, resources.size());
      try (InputStream stream = resources.get(0).getByteStream()) {
        assertArrayEquals(new byte[] {4, 5}, stream.readAllBytes());
      }
      provider.finished(new D8Utils.D8DiagnosticsHandler());
    }
  }

  @Test(expected = IOException.class)
  public void testSelectedDexProviderRejectsMissingArchiveEntry() throws Exception {
    Path archive = tmp.getRoot().toPath().resolve("missing-entry.dex.jar");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(archive))) {
      zip.putNextEntry(new ZipEntry("com/Other.dex"));
      zip.write(new byte[] {1});
      zip.closeEntry();
    }
    D8Utils.writeDexUnitMetadata(archive, List.of("com/Alpha\t1\t0\t0\t0\tcom/Alpha"));
    D8Utils.createSelectedDexProvider(archive, Set.of("com/Alpha.class"));
  }

  @Test(expected = IOException.class)
  public void testMissingSelectedUnitFails() throws Exception {
    Path archive = tmp.getRoot().toPath().resolve("empty.dex.jar");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(archive))) {
      zip.putNextEntry(new ZipEntry("com/Other.dex"));
      zip.write(new byte[] {1});
      zip.closeEntry();
    }
    D8Utils.createSelectedDexProvider(archive, Set.of("com/Missing.class"));
  }

  @Test(expected = IOException.class)
  public void testSelectedMergeRejectsOverflowOutputs() throws Exception {
    Path output = tmp.newFolder("selected-output").toPath();
    Files.write(output.resolve("classes.dex"), new byte[] {1});
    Files.write(output.resolve("classes2.dex"), new byte[] {2});
    D8Utils.requireSingleDexOutput(output);
  }

  @Test
  public void testPartialMatchAndSplitCompanionsFail() throws Exception {
    Path archive = tmp.getRoot().toPath().resolve("partial.dex.jar");
    try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(archive))) {
      zip.putNextEntry(new ZipEntry("com/Alpha.dex"));
      zip.write(new byte[] {1});
      zip.closeEntry();
    }
    D8Utils.writeDexUnitMetadata(
        archive, java.util.List.of("com/Alpha\t1\t0\t0\t0\tcom/Alpha\tcom/Alpha$$Backport0"));
    for (Set<String> classes :
        java.util.List.of(
            Set.of("com/Alpha.class"),
            Set.of("com/Alpha.class", "com/Alpha$$Backport0.class", "com/Missing.class"))) {
      try {
        D8Utils.createSelectedDexProvider(archive, classes);
        fail("Expected incomplete selected inputs to be rejected");
      } catch (IOException expected) {
        assertTrue(expected.getMessage().contains("unit"));
      }
    }
  }

  @Test
  public void testTruncatedUnitHeaderHasDiagnostic() {
    try {
      new DexUnitInfo(ByteDataView.of(new byte[91]), Set.of("Lcom/Alpha;"));
      fail("Expected a truncated header to be rejected");
    } catch (IllegalArgumentException expected) {
      assertTrue(expected.getMessage().contains("112 header bytes"));
    }
  }

  @Test
  public void testMissingSourceWeightUsesEmittedDexSize() {
    DexUnitInfo unit = new DexUnitInfo(ByteDataView.of(new byte[120]), Set.of("Lcom/Generated;"));
    assertEquals(
        java.util.List.of("com/Generated\t240\t0\t0\t0\tcom/Generated"),
        D8ExecutableMain.toDexUnitLines(Map.of("com/Generated", unit), Map.of(), 2));
  }

  @Test
  public void testUnitMetadataIncludesEveryCompanionAndMeasuredRefs() {
    byte[] dex = new byte[120];
    ByteBuffer header = ByteBuffer.wrap(dex).order(ByteOrder.LITTLE_ENDIAN);
    header.putInt(88, 9);
    header.putInt(80, 4);
    header.putInt(64, 12);
    DexUnitInfo unit =
        new DexUnitInfo(
            ByteDataView.of(dex),
            Set.of("Lcom/Alpha;", "Lcom/Alpha$$Backport0;", "Lcom/Alpha$$Backport1;"));
    assertEquals(
        java.util.List.of(
            "com/Alpha\t200\t9\t4\t12\tcom/Alpha\tcom/Alpha$$Backport0\tcom/Alpha$$Backport1"),
        D8ExecutableMain.toDexUnitLines(Map.of("com/Alpha", unit), Map.of("com/Alpha", 100), 2));
  }
}
