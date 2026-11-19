/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.testutil.TemporaryPaths;
import com.facebook.buck.testutil.integration.TestDataHelper;
import com.facebook.buck.testutil.integration.ZipInspector;
import com.google.common.collect.ImmutableList;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.PrintStream;
import java.nio.IntBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;

public class ExoResourcesRewriterTest {
  @Rule public TemporaryPaths tmp = new TemporaryPaths();

  private static final String APK_NAME = "example.apk";

  private Path apkPath;
  private Path testDataDir;

  @Before
  public void setUp() {
    testDataDir = TestDataHelper.getTestDataDirectory(this).resolve("aapt_dump");
    apkPath = testDataDir.resolve(APK_NAME).toAbsolutePath();
  }

  @Test
  public void testRewriteResources() throws IOException {
    AbsPath primaryOutput = tmp.getRoot().resolve("primary.apk");
    AbsPath exoOutput = tmp.getRoot().resolve("exo.apk");
    ExoResourcesRewriter.rewriteResources(
        tmp.getRoot(),
        tmp.getRoot().relativize(apkPath),
        tmp.getRoot().relativize(primaryOutput),
        exoOutput.getPath());

    ZipInspector primaryApkInspector = new ZipInspector(primaryOutput);
    assertEquals(
        ImmutableList.of(
            "resources.arsc",
            "AndroidManifest.xml",
            "res/drawable-nodpi-v4/exo_icon.png",
            "res/xml/meta_xml.xml"),
        primaryApkInspector.getZipFileEntries());
    ZipInspector baseApkInspector = new ZipInspector(apkPath);
    ZipInspector exoApkInspector = new ZipInspector(exoOutput);
    assertEquals(baseApkInspector.getZipFileEntries(), exoApkInspector.getZipFileEntries());

    assertArrayEquals(
        primaryApkInspector.getFileContents("AndroidManifest.xml"),
        exoApkInspector.getFileContents("AndroidManifest.xml"));

    assertArrayEquals(
        primaryApkInspector.getFileContents("res/xml/meta_xml.xml"),
        exoApkInspector.getFileContents("res/xml/meta_xml.xml"));

    ResourceTable primaryResourceTable =
        ResourceTable.get(ResChunk.wrap(primaryApkInspector.getFileContents("resources.arsc")));

    ByteArrayOutputStream baos = new ByteArrayOutputStream();
    primaryResourceTable.dump(new PrintStream(baos));
    String content = new String(baos.toByteArray(), StandardCharsets.UTF_8);
    Path expectedPath = testDataDir.resolve(APK_NAME + ".resources.primary");
    String expected = Files.readString(expectedPath);

    assertEquals(expected, content);

    ResourceTable exoResourceTable =
        ResourceTable.get(ResChunk.wrap(exoApkInspector.getFileContents("resources.arsc")));

    baos = new ByteArrayOutputStream();
    exoResourceTable.dump(new PrintStream(baos));
    content = new String(baos.toByteArray(), StandardCharsets.UTF_8);
    expectedPath = testDataDir.resolve(APK_NAME + ".resources.exo");
    expected = Files.readString(expectedPath);

    assertEquals(expected, content);
  }

  @Test
  public void testRewriteRTxt() throws IOException {
    RelPath inputRTxt = RelPath.of(Paths.get("input.R.txt"));
    String rtxtContent =
        "int style Widget_AppCompat_Light_PopupMenu 0x7f0b0025\n"
            + "int style Widget_AppCompat_Light_PopupMenu_Overflow 0x7f0b0023\n"
            + "int[] styleable ActionMode { 0x7f010000, 0x7f01005a, 0x7f01005b, 0x7f01005e,"
            + " 0x7f010060, 0x7f01006e }\n"
            + "int styleable ActionMode_background 3\n"
            + "int styleable ActionMode_backgroundSplit 4\n"
            + "int styleable ActionMode_closeItemLayout 5\n"
            + "int styleable ActionMode_height 0\n"
            + "int styleable ActionMode_subtitleTextStyle 2\n"
            + "int styleable ActionMode_titleTextStyle 1\n";
    String expectedOutput =
        "int style Widget_AppCompat_Light_PopupMenu 0x7f0b0001\n"
            + "int style Widget_AppCompat_Light_PopupMenu_Overflow 0x7f0b0023\n"
            + "int[] styleable ActionMode { 0x7f010000, 0x7f010001, 0x7f010002, 0x7f01005a,"
            + " 0x7f01005e, 0x7f01006e }\n"
            + "int styleable ActionMode_background 4\n"
            + "int styleable ActionMode_backgroundSplit 1\n"
            + "int styleable ActionMode_closeItemLayout 5\n"
            + "int styleable ActionMode_height 0\n"
            + "int styleable ActionMode_subtitleTextStyle 2\n"
            + "int styleable ActionMode_titleTextStyle 3\n";
    Files.writeString(tmp.getRoot().resolve(inputRTxt).getPath(), rtxtContent);

    RelPath outputRTxt = RelPath.of(Paths.get("output.R.txt"));
    ExoResourcesRewriter.rewriteRDotTxt(
        tmp.getRoot(),
        new ReferenceMapper() {
          @Override
          public int map(int id) {
            switch (id) {
              case 0x7f0b0025:
                return 0x7f0b0001;
              case 0x7f01005b:
                return 0x7f010002;
              case 0x7f010060:
                return 0x7f010001;
            }
            return id;
          }

          @Override
          public void rewrite(int type, IntBuffer buf) {
            throw new UnsupportedOperationException();
          }
        },
        inputRTxt,
        outputRTxt);

    assertEquals(expectedOutput, Files.readString(tmp.getRoot().resolve(outputRTxt).getPath()));
  }
}
