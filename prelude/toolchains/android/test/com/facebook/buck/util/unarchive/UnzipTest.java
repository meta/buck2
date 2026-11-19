/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.unarchive;

import static org.hamcrest.MatcherAssert.assertThat;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.file.MorePosixFilePermissions;
import com.facebook.buck.io.file.MostFiles;
import com.facebook.buck.testutil.TemporaryPaths;
import com.facebook.buck.testutil.ZipArchive;
import com.facebook.buck.util.zip.ZipConstants;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermission;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.Set;
import java.util.zip.ZipEntry;
import org.apache.commons.compress.archivers.zip.ZipArchiveEntry;
import org.apache.commons.compress.archivers.zip.ZipArchiveOutputStream;
import org.hamcrest.Matchers;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;

public class UnzipTest {
  private static final byte[] DUMMY_FILE_CONTENTS = "BUCK Unzip Test String!\nNihao\n".getBytes();

  @Rule public TemporaryPaths tmpFolder = new TemporaryPaths();

  private AbsPath zipFile;

  @Before
  public void setUp() {
    zipFile = tmpFolder.getRoot().resolve("tmp.zip");
  }

  @Test
  public void testExtractZipFile() throws IOException {
    try (ZipArchive zipArchive = new ZipArchive(this.zipFile, true)) {
      zipArchive.add("1.bin", DUMMY_FILE_CONTENTS);
      zipArchive.add("subdir/2.bin", DUMMY_FILE_CONTENTS);
      zipArchive.addDir("emptydir");
    }

    AbsPath extractFolder = tmpFolder.newFolder();
    ImmutableList<Path> result =
        Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    assertTrue(Files.exists(extractFolder.resolve("1.bin").getPath()));
    AbsPath bin2 = extractFolder.resolve("subdir/2.bin");
    assertTrue(Files.exists(bin2.getPath()));
    assertTrue(Files.isDirectory(extractFolder.resolve("emptydir").getPath()));
    try (InputStream input = Files.newInputStream(bin2.getPath())) {
      byte[] buffer = new byte[DUMMY_FILE_CONTENTS.length];
      int bytesRead = input.read(buffer, 0, DUMMY_FILE_CONTENTS.length);
      assertEquals(DUMMY_FILE_CONTENTS.length, bytesRead);
      for (int i = 0; i < DUMMY_FILE_CONTENTS.length; i++) {
        assertEquals(DUMMY_FILE_CONTENTS[i], buffer[i]);
      }
    }
    assertEquals(
        ImmutableList.of(
            extractFolder.resolve("1.bin").getPath(),
            extractFolder.resolve("subdir/2.bin").getPath()),
        result);
  }

  @Test
  public void testExtractZipFilePreservesExecutePermissionsAndModificationTime()
      throws IOException {

    // getFakeTime returs time with some non-zero millis. By doing division and multiplication by
    // 1000 we get rid of that.
    long time = ZipConstants.getFakeTime() / 1000 * 1000;

    // Create a simple zip archive using apache's commons-compress to store executable info.
    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      ZipArchiveEntry entry = new ZipArchiveEntry("test.exe");
      entry.setUnixMode(
          (int) MorePosixFilePermissions.toMode(PosixFilePermissions.fromString("r-x------")));
      entry.setSize(DUMMY_FILE_CONTENTS.length);
      entry.setMethod(ZipEntry.STORED);
      entry.setTime(time);
      zip.putArchiveEntry(entry);
      zip.write(DUMMY_FILE_CONTENTS);
      zip.closeArchiveEntry();
    }

    // Now run `Unzip.extractZipFile` on our test zip and verify that the file is executable.
    AbsPath extractFolder = tmpFolder.newFolder();
    ImmutableList<Path> result =
        Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    AbsPath exe = extractFolder.resolve("test.exe");
    assertTrue(Files.exists(exe.getPath()));
    assertThat(Files.getLastModifiedTime(exe.getPath()).toMillis(), Matchers.equalTo(time));
    assertTrue(Files.isExecutable(exe.getPath()));
    assertEquals(ImmutableList.of(extractFolder.resolve("test.exe").getPath()), result);
  }

  @Test
  public void testExtractSymlink() throws IOException {
    // Create a simple zip archive using apache's commons-compress to store executable info.
    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      ZipArchiveEntry entry = new ZipArchiveEntry("link.txt");
      entry.setUnixMode((int) MostFiles.S_IFLNK);
      String target = "target.txt";
      entry.setSize(target.getBytes(StandardCharsets.UTF_8).length);
      entry.setMethod(ZipEntry.STORED);
      zip.putArchiveEntry(entry);
      zip.write(target.getBytes(StandardCharsets.UTF_8));
      zip.closeArchiveEntry();
    }

    AbsPath extractFolder = tmpFolder.newFolder();

    Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    AbsPath link = extractFolder.resolve("link.txt");
    assertTrue(Files.isSymbolicLink(link.getPath()));
    assertThat(Files.readSymbolicLink(link.getPath()).toString(), Matchers.equalTo("target.txt"));
  }

  @Test
  public void testExtractBrokenSymlinkWithOwnerExecutePermissions() throws IOException {
    // Create a simple zip archive using apache's commons-compress to store executable info.
    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      ZipArchiveEntry entry = new ZipArchiveEntry("link.txt");
      entry.setUnixMode((int) MostFiles.S_IFLNK);
      String target = "target.txt";
      entry.setSize(target.getBytes(StandardCharsets.UTF_8).length);
      entry.setMethod(ZipEntry.STORED);

      // Mark the file as being executable.
      Set<PosixFilePermission> filePermissions = ImmutableSet.of(PosixFilePermission.OWNER_EXECUTE);

      long externalAttributes =
          entry.getExternalAttributes() + (MorePosixFilePermissions.toMode(filePermissions) << 16);
      entry.setExternalAttributes(externalAttributes);

      zip.putArchiveEntry(entry);
      zip.write(target.getBytes(StandardCharsets.UTF_8));
      zip.closeArchiveEntry();
    }

    AbsPath extractFolder = tmpFolder.newFolder();

    Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    AbsPath link = extractFolder.resolve("link.txt");
    assertTrue(Files.isSymbolicLink(link.getPath()));
    assertThat(Files.readSymbolicLink(link.getPath()).toString(), Matchers.equalTo("target.txt"));
  }

  @Test
  public void testExtractWeirdIndex() throws IOException {

    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      zip.putArchiveEntry(new ZipArchiveEntry("foo/bar/baz"));
      zip.write(DUMMY_FILE_CONTENTS, 0, DUMMY_FILE_CONTENTS.length);
      zip.closeArchiveEntry();
      zip.putArchiveEntry(new ZipArchiveEntry("foo/"));
      zip.closeArchiveEntry();
      zip.putArchiveEntry(new ZipArchiveEntry("qux/"));
      zip.closeArchiveEntry();
    }

    AbsPath extractFolder = tmpFolder.newFolder();
    ImmutableList<Path> result =
        Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    assertTrue(Files.exists(extractFolder.resolve("foo").getPath()));
    assertTrue(Files.exists(extractFolder.resolve("foo/bar/baz").getPath()));

    assertEquals(ImmutableList.of(extractFolder.resolve("foo/bar/baz").getPath()), result);
  }

  @Test
  public void testNonCanonicalPaths() throws IOException {
    String[] names = {
      "foo/./", "foo/./bar/", "foo/./bar/baz.cpp", "foo/./bar/baz.h",
    };

    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      for (String name : names) {
        zip.putArchiveEntry(new ZipArchiveEntry(name));
        zip.closeArchiveEntry();
      }
    }

    AbsPath extractFolder = tmpFolder.newFolder();

    Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    for (String name : names) {
      assertTrue(Files.exists(extractFolder.resolve(name).getPath()));
    }
    Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    for (String name : names) {
      assertTrue(Files.exists(extractFolder.resolve(name).getPath()));
    }
  }

  @Test
  public void testParentDirPaths() throws IOException {

    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      // It seems very unlikely that a zip file would contain ".." paths, but handle it anyways.
      zip.putArchiveEntry(new ZipArchiveEntry("foo/bar/"));
      zip.closeArchiveEntry();
      zip.putArchiveEntry(new ZipArchiveEntry("foo/bar/../"));
      zip.closeArchiveEntry();
    }

    AbsPath extractFolder = tmpFolder.newFolder();

    Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    assertTrue(Files.exists(extractFolder.resolve("foo").getPath()));
    assertTrue(Files.exists(extractFolder.resolve("foo/bar").getPath()));
  }

  @Test
  public void testDirectoryPathsOverwriteFiles() throws IOException {
    try (ZipArchiveOutputStream zip = new ZipArchiveOutputStream(zipFile.toFile())) {
      // It seems very unlikely that a zip file would contain ".." paths, but handle it anyways.
      zip.putArchiveEntry(new ZipArchiveEntry("foo/bar"));
      zip.closeArchiveEntry();
    }

    AbsPath extractFolder = tmpFolder.newFolder();
    Files.write(extractFolder.resolve("foo").getPath(), ImmutableList.of("whatever"));

    Unzip.extractArchive(extractFolder, zipFile.getPath(), extractFolder.getPath());
    assertTrue(Files.exists(extractFolder.resolve("foo").getPath()));
    assertTrue(Files.exists(extractFolder.resolve("foo/bar").getPath()));
  }
}
