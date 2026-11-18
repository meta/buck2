/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android;

import static org.junit.Assert.assertEquals;

import com.facebook.buck.android.exopackage.ExopackageInstaller;
import com.facebook.buck.android.exopackage.NativeExoHelper;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.base.Strings;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMultimap;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.ExpectedException;

public class ExopackageInstallerTest {

  @Rule public ExpectedException thrown = ExpectedException.none();

  @Rule public final TemporaryPaths tmpDir = new TemporaryPaths();

  @Test
  public void testFilterLibrariesForAbi() {
    AbsPath libsDir = RelPath.get("example").toAbsolutePath();
    ImmutableMultimap<String, Path> allLibs =
        ImmutableMultimap.of(
            Strings.repeat("a", 40), libsDir.resolve("libs/armeabi-v7a/libmy1.so").getPath(),
            Strings.repeat("b", 40), libsDir.resolve("libs/armeabi-v7a/libmy2.so").getPath(),
            Strings.repeat("c", 40), libsDir.resolve("libs/armeabi/libmy2.so").getPath(),
            Strings.repeat("d", 40), libsDir.resolve("libs/armeabi/libmy3.so").getPath(),
            Strings.repeat("e", 40), libsDir.resolve("libs/x86/libmy1.so").getPath());

    assertEquals(
        ImmutableSet.of(Strings.repeat("a", 40), Strings.repeat("b", 40)),
        NativeExoHelper.filterLibrariesForAbi(libsDir, allLibs, "armeabi-v7a", ImmutableSet.of())
            .keySet());

    assertEquals(
        ImmutableSet.of(Strings.repeat("d", 40)),
        NativeExoHelper.filterLibrariesForAbi(
                libsDir, allLibs, "armeabi", ImmutableSet.of("libmy1.so", "libmy2.so"))
            .keySet());
  }

  @Test
  public void testParseExopackageInfoMetadata() throws IOException {
    String illegalLine = "no_space_in_this_line_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    thrown.expectMessage("Illegal line in metadata file: " + illegalLine);

    Path baseDir = Paths.get("basedir");
    AbsPath rootPath = tmpDir.getRoot();
    Path metadataPath = Paths.get("metadata.txt");
    ProjectFilesystemUtils.writeLinesToPath(
        rootPath,
        ImmutableList.of(
            ".some_config",
            ".more_config with_spaces",
            "filename.jar aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "dir/anotherfile.jar bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        metadataPath);

    assertEquals(
        ImmutableMultimap.of(
            Strings.repeat("a", 40), Paths.get("basedir/filename.jar"),
            Strings.repeat("b", 40), Paths.get("basedir/dir/anotherfile.jar")),
        ExopackageInstaller.parseExopackageInfoMetadata(metadataPath, baseDir, rootPath));

    ProjectFilesystemUtils.writeLinesToPath(
        rootPath,
        ImmutableList.of("filename.jar aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", illegalLine),
        metadataPath);

    ExopackageInstaller.parseExopackageInfoMetadata(metadataPath, baseDir, rootPath);
  }
}
