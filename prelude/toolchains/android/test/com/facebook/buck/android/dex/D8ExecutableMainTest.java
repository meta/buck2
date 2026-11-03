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
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.facebook.buck.android.dex.D8ExecutableMain.DexRefCounts;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/**
 * Tests for DEX ref count reading functionality in D8ExecutableMain.
 *
 * <p>The DEX file format header contains exact method_ids_size, field_ids_size, and type_ids_size
 * at well-known offsets. These counts are used for 64K-limit-aware secondary DEX splitting.
 */
public class D8ExecutableMainTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  // First 120 bytes of a real DEX header from main_r_dot_java.jar, produced by D8.
  // Expected ref counts (independently verified via Python struct.unpack_from):
  //   type_ids_size   (offset 64) = 17
  //   field_ids_size  (offset 80) = 10
  //   method_ids_size (offset 88) = 19
  private static final byte[] REAL_DEX_HEADER = {
    0x64,
    0x65,
    0x78,
    0x0a,
    0x30,
    0x33,
    0x35,
    0x00, // magic: "dex\n035\0"
    (byte) 0xbf,
    (byte) 0xe4,
    0x3e,
    0x77, // checksum
    (byte) 0xef,
    0x59,
    (byte) 0xa5,
    (byte) 0xaa, // signature[0..3]
    0x43,
    (byte) 0xc0,
    (byte) 0x80,
    (byte) 0x8c, // signature[4..7]
    (byte) 0xb2,
    0x7e,
    (byte) 0xa9,
    0x0c, // signature[8..11]
    0x32,
    (byte) 0x9e,
    0x18,
    0x67, // signature[12..15]
    0x12,
    0x3b,
    (byte) 0xaa,
    (byte) 0xe6, // signature[16..19]
    0x64,
    0x0b,
    0x00,
    0x00, // file_size = 2916
    0x70,
    0x00,
    0x00,
    0x00, // header_size = 112
    0x78,
    0x56,
    0x34,
    0x12, // endian_tag = ENDIAN_CONSTANT
    0x00,
    0x00,
    0x00,
    0x00, // link_size = 0
    0x00,
    0x00,
    0x00,
    0x00, // link_off = 0
    (byte) 0x94,
    0x0a,
    0x00,
    0x00, // map_off
    0x2e,
    0x00,
    0x00,
    0x00, // string_ids_size = 46
    0x70,
    0x00,
    0x00,
    0x00, // string_ids_off
    0x11,
    0x00,
    0x00,
    0x00, // type_ids_size = 17      <-- offset 64
    0x28,
    0x01,
    0x00,
    0x00, // type_ids_off
    0x04,
    0x00,
    0x00,
    0x00, // proto_ids_size = 4
    0x6c,
    0x01,
    0x00,
    0x00, // proto_ids_off
    0x0a,
    0x00,
    0x00,
    0x00, // field_ids_size = 10     <-- offset 80
    (byte) 0x9c,
    0x01,
    0x00,
    0x00, // field_ids_off
    0x13,
    0x00,
    0x00,
    0x00, // method_ids_size = 19    <-- offset 88
    (byte) 0xec,
    0x01,
    0x00,
    0x00, // method_ids_off
    0x07,
    0x00,
    0x00,
    0x00, // class_defs_size = 7
    (byte) 0x84,
    0x02,
    0x00,
    0x00, // class_defs_off
    0x00,
    0x08,
    0x00,
    0x00, // data_size
    0x64,
    0x03,
    0x00,
    0x00, // data_off
    0x7e,
    0x05,
    0x00,
    0x00, // (beyond v40 header)
    (byte) 0x88,
    0x05,
    0x00,
    0x00, // (beyond v40 header)
  };

  @Test
  public void testReadDexRefCounts_realDexHeader() throws Exception {
    DexRefCounts counts =
        D8ExecutableMain.readDexRefCounts(new ByteArrayInputStream(REAL_DEX_HEADER));
    assertEquals("method_ids_size", 19, counts.methodIds);
    assertEquals("field_ids_size", 10, counts.fieldIds);
    assertEquals("type_ids_size", 17, counts.typeIds);
  }

  @Test
  public void testReadDexRefCounts_truncatedStream() throws Exception {
    byte[] truncated = new byte[50]; // less than 120-byte header
    try {
      D8ExecutableMain.readDexRefCounts(new ByteArrayInputStream(truncated));
      fail("Expected IOException for truncated stream");
    } catch (IOException e) {
      if (!e.getMessage().contains("header bytes")) {
        fail("Error message should mention header bytes, got: " + e.getMessage());
      }
    }
  }

  @Test
  public void testReadDexRefCounts_emptyStream() throws Exception {
    try {
      D8ExecutableMain.readDexRefCounts(new ByteArrayInputStream(new byte[0]));
      fail("Expected IOException for empty stream");
    } catch (IOException e) {
      // Expected
    }
  }

  /**
   * Builds a minimal 120-byte DEX header with the given ref counts. The header has valid magic,
   * endian tag, and the specified method/field/type ID sizes at their canonical offsets.
   */
  private static byte[] makeDexHeader(int methodIds, int fieldIds, int typeIds) {
    ByteBuffer buf = ByteBuffer.allocate(120).order(ByteOrder.LITTLE_ENDIAN);
    // magic: "dex\n035\0"
    buf.put(new byte[] {0x64, 0x65, 0x78, 0x0a, 0x30, 0x33, 0x35, 0x00});
    // Fill up to offset 64 (type_ids_size) with zeros (checksum, signature, etc.)
    buf.position(64);
    buf.putInt(typeIds); // type_ids_size at offset 64
    buf.putInt(0); // type_ids_off at offset 68
    buf.putInt(0); // proto_ids_size at offset 72
    buf.putInt(0); // proto_ids_off at offset 76
    buf.putInt(fieldIds); // field_ids_size at offset 80
    buf.putInt(0); // field_ids_off at offset 84
    buf.putInt(methodIds); // method_ids_size at offset 88
    buf.putInt(0); // method_ids_off at offset 92
    // Fill remaining bytes to reach 120
    buf.position(120);
    return buf.array();
  }

  @Test
  public void testReadDexRefCountsFromJar_multiDex() throws Exception {
    // Create a temporary .dex.jar with two .dex entries
    Path jarPath = Files.createTempFile("test", ".dex.jar");
    try {
      try (ZipOutputStream zos = new ZipOutputStream(Files.newOutputStream(jarPath))) {
        zos.putNextEntry(new ZipEntry("classes.dex"));
        zos.write(makeDexHeader(100, 200, 300));
        zos.closeEntry();

        zos.putNextEntry(new ZipEntry("classes2.dex"));
        zos.write(makeDexHeader(40, 50, 60));
        zos.closeEntry();

        // Non-dex entry should be ignored
        zos.putNextEntry(new ZipEntry("META-INF/MANIFEST.MF"));
        zos.write("Manifest-Version: 1.0\n".getBytes());
        zos.closeEntry();
      }

      try (ZipFile dexJar = new ZipFile(jarPath.toFile())) {
        DexRefCounts counts = D8ExecutableMain.readDexRefCountsFromJar(dexJar);
        assertEquals("method_ids_size sum", 140, counts.methodIds);
        assertEquals("field_ids_size sum", 250, counts.fieldIds);
        assertEquals("type_ids_size sum", 360, counts.typeIds);
      }
    } finally {
      Files.deleteIfExists(jarPath);
    }
  }

  private static final DexRefCounts OVER_FIELD_LIMIT = new DexRefCounts(124, 72367, 53);

  @Test
  public void testWantsClassRefCounts_flagOff_noneEvenOverTheLimit() {
    assertFalse(D8ExecutableMain.wantsClassRefCounts(false, OVER_FIELD_LIMIT, ImmutableMap.of()));
  }

  @Test
  public void testWantsClassRefCounts_flagOn_overTheLimit() {
    assertTrue(D8ExecutableMain.wantsClassRefCounts(true, OVER_FIELD_LIMIT, ImmutableMap.of()));
    assertTrue(
        D8ExecutableMain.wantsClassRefCounts(
            true, new DexRefCounts(65537, 0, 1), ImmutableMap.of()));
    assertTrue(
        D8ExecutableMain.wantsClassRefCounts(
            true, new DexRefCounts(0, 0, 65537), ImmutableMap.of()));
  }

  @Test
  public void testWantsClassRefCounts_flagOn_noneAtOrUnderTheLimit() {
    assertFalse(
        D8ExecutableMain.wantsClassRefCounts(
            true, new DexRefCounts(65536, 65536, 65536), ImmutableMap.of()));
  }

  @Test
  public void testWantsClassRefCounts_flagOn_noneWithSyntheticClasses() {
    assertFalse(
        D8ExecutableMain.wantsClassRefCounts(
            true, OVER_FIELD_LIMIT, ImmutableMap.of("a/B$$Lambda0", "a/B")));
  }

  @Test
  public void testGetClassRefCountLines_oneLinePerClass() throws Exception {
    Path dir = tmp.newFolder("per-class").toPath();
    writeDexFile(dir.resolve("com/example/P3.dex"), makeDexHeader(2, 16405, 4));
    writeDexFile(dir.resolve("com/example/B$C.dex"), makeDexHeader(13, 4, 12));
    Files.write(dir.resolve("com/example/notes.txt"), new byte[] {1});
    assertEquals(
        ImmutableList.of("13 4 12 com/example/B$C", "2 16405 4 com/example/P3"),
        D8ExecutableMain.getClassRefCountLines(dir));
  }

  @Test
  public void testGetClassRefCountLines_noneWhenAClassNameHoldsASpace() throws Exception {
    Path dir = tmp.newFolder("per-class").toPath();
    writeDexFile(dir.resolve("com/example/P3.dex"), makeDexHeader(2, 16405, 4));
    writeDexFile(dir.resolve("com/example/B\u00a0C.dex"), makeDexHeader(13, 4, 12));
    assertEquals(ImmutableList.of(), D8ExecutableMain.getClassRefCountLines(dir));
  }

  private static void writeDexFile(Path path, byte[] header) throws IOException {
    Files.createDirectories(path.getParent());
    Files.write(path, header);
  }

  @Test
  public void testToInternalClassNames() {
    // "Foo-IA" is the nest access constructor marker D8 synthesizes for target 11+ bytecode; it
    // reaches us only through the output consumer, since it has no .class counterpart.
    assertEquals(
        ImmutableList.of("com/example/Foo", "com/example/Foo$Bar", "com/example/Foo-IA"),
        D8ExecutableMain.toInternalClassNames(
            ImmutableList.of(
                "Lcom/example/Foo$Bar;", "Lcom/example/Foo-IA;", "Lcom/example/Foo;")));
  }

  @Test
  public void testToInternalClassNames_skipsNonClassDescriptors() {
    assertEquals(
        ImmutableList.of("com/example/Foo"),
        D8ExecutableMain.toInternalClassNames(
            ImmutableList.of("Lcom/example/Foo;", "[Lcom/example/Foo;", "I", "L;")));
  }
}
