/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk;

import com.android.apksig.ApkSigner;
import com.facebook.buck.android.apk.sdk.ApkCreationException;
import com.facebook.buck.android.apk.sdk.DuplicateFileException;
import com.facebook.buck.android.apk.sdk.SealedApkException;
import com.facebook.buck.android.zipalign.ZipAlign;
import com.facebook.buck.util.zip.RepackZipEntries;
import com.facebook.buck.util.zip.ZipCompressionLevel;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import com.google.common.io.MoreFiles;
import com.google.common.io.RecursiveDeleteOption;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Pattern;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Main entry point for executing {@link ApkBuilderUtils} calls. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ApkBuilderExecutableMain {
  @Option(name = "--output-apk", required = true)
  private String outputApk;

  @Option(name = "--resource-apk", required = true)
  private String resourceApk;

  @Option(name = "--dex-file", required = true)
  private String dexFile;

  @Option(name = "--keystore-path", required = true)
  private String keystore;

  @Option(name = "--keystore-properties-path", required = true)
  private String keystoreProperties;

  @Option(name = "--asset-directories-list", required = true)
  private String assetDirectoriesList;

  @Option(name = "--native-libraries-directories-list", required = true)
  private String nativeLibrariesDirectoriesList;

  @Option(name = "--zip-files-list", required = true)
  private String zipFilesList;

  @Option(name = "--jar-files-that-may-contain-resources-list", required = true)
  private String jarFilesThatMayContainResourcesList;

  @Option(name = "--zipalign_tool", required = true)
  private String zipalignTool;

  @Option(name = "--compress-resources-dot-arsc")
  private boolean compressResourcesDotArsc;

  @Option(name = "--package-meta-inf-version-files")
  private boolean packageMetaInfVersionFiles;

  @Option(name = "--excluded-resources")
  @Nullable
  private String excludedResourcesList = null;

  @Option(name = "--uncompressed-files")
  @Nullable
  private String uncompressedFilesList = null;

  @Option(name = "--build-info-generator-args")
  @Nullable
  private String buildInfoGeneratorArgs = null;

  public static void main(String[] args) throws IOException {
    ApkBuilderExecutableMain main = new ApkBuilderExecutableMain();
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
    String scratchPath = System.getenv("BUCK_SCRATCH_PATH");
    if (scratchPath == null || scratchPath.isEmpty()) {
      throw new IllegalStateException(
          "BUCK_SCRATCH_PATH environment variable must be set and non-empty");
    }
    Path scratchDir = Paths.get(scratchPath);

    ImmutableSet<Path> assetDirectories =
        Files.readAllLines(Paths.get(assetDirectoriesList)).stream()
            .map(Paths::get)
            .collect(ImmutableSet.toImmutableSet());

    Path buildInfoDir = null;
    if (buildInfoGeneratorArgs != null) {
      buildInfoDir = generateBuildInfoAssetDir(buildInfoGeneratorArgs, scratchDir);
      assetDirectories =
          ImmutableSet.<Path>builder().addAll(assetDirectories).add(buildInfoDir).build();
    }

    ImmutableSet<Path> nativeLibraryDirectories =
        Files.readAllLines(Paths.get(nativeLibrariesDirectoriesList)).stream()
            .map(Paths::get)
            .collect(ImmutableSet.toImmutableSet());

    ImmutableSet<Path> zipFiles =
        Files.readAllLines(Paths.get(zipFilesList)).stream()
            .map(Paths::get)
            .collect(ImmutableSet.toImmutableSet());

    ImmutableSet<Path> jarFilesThatMayContainResources =
        Files.readAllLines(Paths.get(jarFilesThatMayContainResourcesList)).stream()
            .map(Paths::get)
            .collect(ImmutableSet.toImmutableSet());

    ImmutableSet<String> excludedResources =
        excludedResourcesList == null
            ? ImmutableSet.of()
            : ImmutableSet.copyOf(Files.readAllLines(Paths.get(excludedResourcesList)));

    ImmutableSet<Pattern> uncompressedFilePatterns =
        uncompressedFilesList == null
            ? ImmutableSet.of()
            : Files.readAllLines(Paths.get(uncompressedFilesList)).stream()
                .map(Pattern::compile)
                .collect(ImmutableSet.toImmutableSet());

    Path keystorePath = Paths.get(keystore);
    Path keystorePropertiesPath = Paths.get(keystoreProperties);

    Path tempDir = Files.createTempDirectory(scratchDir, "apk_builder");
    Path intermediateApk = tempDir.resolve("intermediate.apk");
    Path zipalignApk = tempDir.resolve("zipalign.apk");
    KeystoreProperties keystoreProperties =
        KeystoreProperties.createFromPropertiesFile(keystorePath, keystorePropertiesPath);

    try {
      ApkBuilderUtils.buildApk(
          Paths.get(resourceApk),
          intermediateApk,
          Paths.get(dexFile),
          assetDirectories,
          nativeLibraryDirectories,
          zipFiles,
          jarFilesThatMayContainResources,
          keystorePath,
          keystoreProperties,
          packageMetaInfVersionFiles,
          null,
          excludedResources,
          archivePath -> matchesAny(uncompressedFilePatterns, archivePath));

      if (compressResourcesDotArsc) {
        Path intermediateApkWithCompressedResources =
            tempDir.resolve("intermediate_with_compressed_resources.apk");
        RepackZipEntries.repack(
            intermediateApk,
            intermediateApkWithCompressedResources,
            ImmutableSet.of("resources.arsc"),
            ZipCompressionLevel.MAX);
        intermediateApk = intermediateApkWithCompressedResources;
      }

      if (!uncompressedFilePatterns.isEmpty()) {
        ImmutableSet<String> compressedUncompressedFiles =
            findCompressedEntries(intermediateApk, uncompressedFilePatterns);
        if (!compressedUncompressedFiles.isEmpty()) {
          Path intermediateApkWithUncompressedFiles =
              tempDir.resolve("intermediate_with_uncompressed_files.apk");
          RepackZipEntries.repack(
              intermediateApk,
              intermediateApkWithUncompressedFiles,
              compressedUncompressedFiles,
              ZipCompressionLevel.NONE);
          intermediateApk = intermediateApkWithUncompressedFiles;
        }
      }

      ZipAlign zipAlign =
          new ZipAlign(zipalignTool, intermediateApk.toString(), zipalignApk.toString());
      zipAlign.run();

      ImmutableList<ApkSigner.SignerConfig> signerConfigs =
          ApkSignerUtils.getSignerConfigs(keystoreProperties, Files.newInputStream(keystorePath));
      ApkSignerUtils.signApkFile(
          zipalignApk.toFile(), Paths.get(outputApk).toFile(), signerConfigs);
    } catch (UnrecoverableKeyException
        | NoSuchAlgorithmException
        | ApkCreationException
        | SealedApkException
        | KeyStoreException
        | InterruptedException e) {
      throw new RuntimeException(e);
    } catch (DuplicateFileException e) {
      throw new RuntimeException(
          String.format(
              "Found duplicate file for APK: %1$s\nOrigin 1: %2$s\nOrigin 2: %3$s",
              e.getArchivePath(), e.getFile1(), e.getFile2()));
    } finally {
      deleteRecursivelyQuietly(tempDir);
      if (buildInfoDir != null) {
        // Clean up the synthesized build-info temp dir (the bundler does the same on iOS); the
        // action sandbox would reclaim it anyway.
        deleteRecursivelyQuietly(buildInfoDir);
      }
    }

    System.exit(0);
  }

  /**
   * Runs the toolchain's build-info generator into a temp dir and returns it for inclusion as an
   * APK asset. {@code argsFile} holds the generator's run command, one token per line; the
   * generator reads {@code BUCK_BUILD_ID} from this action's environment, inherited by the
   * subprocess. Keeping the build-info-writing logic in that separate toolchain tool, but running
   * it from the apk packaging action, ties the baked id to the APK content (re-baked only when the
   * APK changes).
   */
  private static Path generateBuildInfoAssetDir(String argsFile, Path scratchDir)
      throws IOException {
    List<String> command = new ArrayList<>(Files.readAllLines(Paths.get(argsFile)));
    Path dir = Files.createTempDirectory(scratchDir, "build_info");
    command.add("--output-dir");
    command.add(dir.toString());
    Process process = new ProcessBuilder(command).inheritIO().start();
    int exitCode;
    try {
      exitCode = process.waitFor();
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new RuntimeException("Interrupted while running build-info generator", e);
    }
    if (exitCode != 0) {
      throw new RuntimeException("Build-info generator failed with exit code " + exitCode);
    }
    return dir;
  }

  private static void deleteRecursivelyQuietly(Path path) {
    try {
      MoreFiles.deleteRecursively(path, RecursiveDeleteOption.ALLOW_INSECURE);
    } catch (IOException e) {
      // Best-effort cleanup.
    }
  }

  private static boolean matchesAny(ImmutableSet<Pattern> patterns, String archivePath) {
    for (Pattern pattern : patterns) {
      if (pattern.matcher(archivePath).matches()) {
        return true;
      }
    }
    return false;
  }

  /**
   * Returns the entries of the APK that match one of the regex patterns but are still compressed.
   * Files the {@link com.facebook.buck.android.apk.sdk.ApkBuilder} adds itself are already stored
   * when they match; entries copied from other archives keep their original compression method.
   *
   * @param apkPath Path to the APK file to scan for matching entries
   * @param patterns Set of regex patterns to match against APK entries
   * @return Set of names of compressed entries that match the patterns
   */
  static ImmutableSet<String> findCompressedEntries(Path apkPath, ImmutableSet<Pattern> patterns)
      throws IOException {
    try (ZipFile zipFile = new ZipFile(apkPath.toFile())) {
      return zipFile.stream()
          .filter(entry -> entry.getMethod() != ZipEntry.STORED)
          .map(ZipEntry::getName)
          .filter(name -> matchesAny(patterns, name))
          .collect(ImmutableSet.toImmutableSet());
    }
  }
}
