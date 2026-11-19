/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources.strings;

import static com.facebook.buck.util.zip.ZipOutputStreams.HandleDuplicates.THROW_EXCEPTION;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.util.zip.CustomZipEntryWithPath;
import com.facebook.buck.util.zip.CustomZipOutputStream;
import com.facebook.buck.util.zip.Zip;
import com.facebook.buck.util.zip.ZipCompressionLevel;
import com.facebook.buck.util.zip.ZipOutputStreams;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import java.io.BufferedOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Map;
import java.util.TreeMap;
import java.util.function.Function;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;
import org.xml.sax.SAXException;

/** Entry point for packaging string resources as assets. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PackageStringsAsAssetsExecutableMain {
  private static final String STRING_ASSET_FILE_EXTENSION = ".fbstr";

  @Option(name = "--string-files-list", required = true)
  private String stringFilesList;

  @Option(name = "--r-dot-txt", required = true)
  private String rDotTxt;

  @Option(name = "--string-assets-dir", required = true)
  private String stringAssetsDir;

  @Option(name = "--string-assets-zip", required = true)
  private String stringAssetsZip;

  @Option(name = "--all-locales-string-assets-zip", required = true)
  private String allLocalesStringAssetsZip;

  @Option(name = "--locales")
  @Nullable
  private String localesString = null;

  public static void main(String[] args) throws IOException {
    PackageStringsAsAssetsExecutableMain main = new PackageStringsAsAssetsExecutableMain();
    CmdLineParser parser = new CmdLineParser(main);
    try {
      parser.parseArgument(args);
      main.run();
      System.exit(0);
    } catch (CmdLineException e) {
      System.err.println(e.toString());
      parser.printUsage(System.err);
      System.exit(1);
    }
  }

  private void run() throws IOException {
    AbsPath root = AbsPath.of(Paths.get(".").normalize().toAbsolutePath());
    ImmutableList<Path> stringFiles =
        Files.readAllLines(Paths.get(stringFilesList)).stream()
            .map(Paths::get)
            .collect(ImmutableList.toImmutableList());
    ImmutableSet<String> locales =
        localesString != null ? ImmutableSet.copyOf(localesString.split(",")) : ImmutableSet.of();

    Path stringAssetsDirPath = Paths.get(stringAssetsDir);
    Path stringsDir = stringAssetsDirPath.resolve("assets").resolve("strings");
    ProjectFilesystemUtils.mkdirs(root, stringsDir);
    Function<String, Path> assetPathBuilder =
        locale -> stringsDir.resolve(locale + STRING_ASSET_FILE_EXTENSION);

    CompileStrings compileStrings = new CompileStrings();
    try {
      compileStrings.compileStrings(root, stringFiles, Paths.get(rDotTxt), assetPathBuilder);
    } catch (SAXException e) {
      throw new RuntimeException(e);
    }

    Path pathToStringAssetsZip = Paths.get(stringAssetsZip);
    Path pathToAllLocalesStringAssetsZip = Paths.get(allLocalesStringAssetsZip);

    zipEntries(
        root,
        pathToStringAssetsZip,
        stringAssetsDirPath,
        locales.stream().map(assetPathBuilder).collect(ImmutableSet.toImmutableSet()));
    zipEntries(root, pathToAllLocalesStringAssetsZip, stringAssetsDirPath, ImmutableSet.of());

    System.exit(0);
  }

  private void zipEntries(
      AbsPath root, Path pathToZipFile, Path assetsDir, ImmutableSet<Path> pathsToInclude)
      throws IOException {
    Map<String, CustomZipEntryWithPath> entries = new TreeMap<>();

    try (BufferedOutputStream baseOut =
            new BufferedOutputStream(
                ProjectFilesystemUtils.newFileOutputStream(root, pathToZipFile));
        CustomZipOutputStream out = ZipOutputStreams.newOutputStream(baseOut, THROW_EXCEPTION)) {
      Zip.walkBaseDirectoryToCreateEntries(
          root,
          entries,
          assetsDir,
          ImmutableSet.of(),
          pathsToInclude,
          false,
          ZipCompressionLevel.MAX);
      Zip.writeEntriesToZip(root, out, entries);
    }
  }
}
