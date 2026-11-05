/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.bundle;

import com.android.tools.build.bundletool.commands.BuildBundleCommand;
import com.facebook.buck.android.apk.sdk.ApkCreationException;
import com.facebook.buck.android.apk.sdk.DuplicateFileException;
import com.facebook.buck.android.apk.sdk.SealedApkException;
import com.facebook.buck.util.zip.ZipScrubber;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableMultimap;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashSet;
import java.util.Objects;
import java.util.Set;
import java.util.function.Function;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Main entry point for building an Android bundle. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class AndroidBundleBuilderExecutableMain {
  @Option(name = "--output-bundle", required = true)
  private String outputBundle;

  @Option(name = "--path-to-bundle-config-file")
  @Nullable
  private String pathToBundleConfigFile = null;

  @Option(name = "--resource-apk", required = true)
  private String resourceApk;

  @Option(name = "--dex-file", required = true)
  private String dexFile;

  @Option(name = "--root-module-asset-directories-list", required = true)
  private String rootModuleAssetDirectoriesList;

  @Option(name = "--non-root-module-asset-directories-list", required = true)
  private String nonRootModuleAssetDirectoriesList;

  @Option(name = "--non-root-module-asset-native-lib-directories-list", required = true)
  private String nonRootModuleAssetNativeLibDirectoriesList;

  @Option(name = "--native-libraries-directories-list", required = true)
  private String nativeLibrariesDirectoriesList;

  @Option(name = "--zip-files-list", required = true)
  private String zipFilesList;

  @Option(name = "--jar-files-that-may-contain-resources-list", required = true)
  private String jarFilesThatMayContainResourcesList;

  @Option(name = "--package-meta-inf-version-files")
  private boolean packageMetaInfVersionFiles;

  @Option(name = "--module-assets-dir")
  @Nullable
  private Path moduleAssetsDir = null;

  @Option(name = "--excluded-resources")
  @Nullable
  private String excludedResourcesList = null;

  public static void main(String[] args) throws IOException {
    AndroidBundleBuilderExecutableMain main = new AndroidBundleBuilderExecutableMain();
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
    ImmutableMap<Path, String> rootModuleAssetDirectories =
        Files.readAllLines(Paths.get(rootModuleAssetDirectoriesList)).stream()
            .map(Paths::get)
            .collect(ImmutableMap.toImmutableMap(Function.identity(), x -> ""));

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

    ImmutableList.Builder<Path> modulePaths = ImmutableList.builder();

    String scratchPath = System.getenv("BUCK_SCRATCH_PATH");
    if (scratchPath == null || scratchPath.isEmpty()) {
      throw new IllegalStateException(
          "BUCK_SCRATCH_PATH environment variable must be set and non-empty");
    }
    Path scratchDir = Paths.get(scratchPath);
    Path tempDir = Files.createTempDirectory(scratchDir, "bundleTempDir");
    Path rootModuleZip = tempDir.resolve("base.zip");
    modulePaths.add(rootModuleZip);
    Path fakeResourcesApk = Files.createFile(tempDir.resolve("fake.txt"));
    Set<String> perModuleAddedFiles = new HashSet<>();
    Set<Path> addedSourceFiles = new HashSet<>();
    try {
      AndroidBundleUtils.addModule(
          new ThrowingDuplicateFileListener(),
          rootModuleZip.toFile(),
          fakeResourcesApk.toFile(),
          packageMetaInfVersionFiles,
          null,
          /* isBaseModule */ true,
          Paths.get(resourceApk),
          ImmutableSet.of(Paths.get(dexFile)),
          rootModuleAssetDirectories,
          nativeLibraryDirectories,
          zipFiles,
          jarFilesThatMayContainResources,
          perModuleAddedFiles,
          addedSourceFiles,
          excludedResources);
    } catch (ApkCreationException | SealedApkException e) {
      throw new RuntimeException(e);
    } catch (DuplicateFileException e) {
      throw new RuntimeException(
          String.format(
              "Found duplicate file for APK: %1$s\nOrigin 1: %2$s\nOrigin 2: %3$s",
              e.getArchivePath(), e.getFile1(), e.getFile2()));
    }

    ImmutableList<Path> nonRootModuleAssetDirectories =
        Files.readAllLines(Paths.get(nonRootModuleAssetDirectoriesList)).stream()
            .map(Paths::get)
            .collect(ImmutableList.toImmutableList());
    ImmutableList<Path> nonRootModuleAssetNativeLibDirectories =
        Files.readAllLines(Paths.get(nonRootModuleAssetNativeLibDirectoriesList)).stream()
            .map(Paths::get)
            .collect(ImmutableList.toImmutableList());

    ImmutableMap.Builder<String, Path> moduleToAssetsApkBuilder = ImmutableMap.builder();
    if (moduleAssetsDir != null) {
      for (String module : Objects.requireNonNull(moduleAssetsDir.toFile().list())) {
        Path assetsApk = moduleAssetsDir.resolve(module).resolve("assets.ap_");
        Preconditions.checkState(Files.exists(assetsApk));
        moduleToAssetsApkBuilder.put(module, assetsApk);
      }
    }
    ImmutableMap<String, Path> moduleToAssetsApk = moduleToAssetsApkBuilder.build();

    ImmutableMultimap.Builder<String, Path> moduleToAssetDirBuilder = ImmutableMultimap.builder();
    ImmutableMultimap.Builder<String, Path> moduleToNativeLibDirBuilder =
        ImmutableMultimap.builder();

    for (Path nonRootModuleAssetDir : nonRootModuleAssetDirectories) {
      Path assetsDir = nonRootModuleAssetDir.resolve("assets");
      if (!assetsDir.toFile().exists()) {
        continue;
      }
      for (String module : Objects.requireNonNull(assetsDir.toFile().list())) {
        moduleToAssetDirBuilder.put(module, assetsDir.resolve(module));
      }
    }
    /**
     * Directory structure of nonRootModuleAssetNativeLibDirectories is as follows
     *
     * <pre>
     * assets/
     *     <module>/
     *         assets/
     *             <asset native libs>
     *         lib/
     *             <non asset native libs>
     * </pre>
     */
    for (Path nonRootModuleAssetNativeLibDir : nonRootModuleAssetNativeLibDirectories) {
      Path libDir = nonRootModuleAssetNativeLibDir.resolve("assets");
      if (!libDir.toFile().exists()) {
        continue;
      }
      for (String module : Objects.requireNonNull(libDir.toFile().list())) {
        Path assetsDir = libDir.resolve(module).resolve("assets");
        if (assetsDir.toFile().exists()) {
          moduleToAssetDirBuilder.put(module, assetsDir);
        }
        Path nativeLibDir = libDir.resolve(module).resolve("lib");
        if (nativeLibDir.toFile().exists()) {
          moduleToNativeLibDirBuilder.put(module, nativeLibDir);
        }
      }
    }

    ImmutableMultimap<String, Path> moduleToAssetDir = moduleToAssetDirBuilder.build();
    ImmutableMultimap<String, Path> moduleToNativeLibDir = moduleToNativeLibDirBuilder.build();

    for (String moduleName : moduleToAssetDir.keySet()) {
      Path outputZip = tempDir.resolve(moduleName + ".zip");
      modulePaths.add(outputZip);
      try {
        perModuleAddedFiles.clear();
        AndroidBundleUtils.addModule(
            new ThrowingDuplicateFileListener(),
            outputZip.toFile(),
            moduleToAssetsApk.containsKey(moduleName)
                ? moduleToAssetsApk.get(moduleName).toFile()
                : fakeResourcesApk.toFile(),
            false,
            null,
            /* isBaseModule */ false,
            /* resourceApk */ null,
            /* dexFile */ ImmutableSet.of(),
            moduleToAssetDir.get(moduleName).stream()
                .collect(
                    ImmutableMap.toImmutableMap(
                        p -> p, p -> String.format("assets/%s", moduleName))),
            moduleToNativeLibDir.get(moduleName).stream().collect(ImmutableSet.toImmutableSet()),
            /* zipFiles */ ImmutableSet.of(),
            /* jarFilesThatMayContainResources */ ImmutableSet.of(),
            perModuleAddedFiles,
            addedSourceFiles,
            excludedResources);
      } catch (ApkCreationException | SealedApkException e) {
        throw new RuntimeException(e);
      } catch (DuplicateFileException e) {
        throw new RuntimeException(
            String.format(
                "Found duplicate file for APK: %1$s\nOrigin 1: %2$s\nOrigin 2: %3$s",
                e.getArchivePath(), e.getFile1(), e.getFile2()));
      }
    }

    Path outputPath = Paths.get(outputBundle);
    BuildBundleCommand.Builder bundleBuilder =
        BuildBundleCommand.builder()
            .setOutputPath(outputPath)
            .setOverwriteOutput(true)
            .setModulesPaths(modulePaths.build());

    if (pathToBundleConfigFile != null) {
      bundleBuilder.setBundleConfig(Paths.get(pathToBundleConfigFile));
    }
    bundleBuilder.build().execute();

    ZipScrubber.scrubZip(outputPath);
  }

  private static class ThrowingDuplicateFileListener
      implements AndroidBundleUtils.DuplicateFileListener {

    @Override
    public void onDuplicateFileAdded(String file) {
      if (!file.endsWith("/")) {
        throw new RuntimeException(String.format("File %s has already been added!", file));
      }
    }

    @Override
    public void onDuplicateSourceFileAdded(Path sourceFile) {
      throw new RuntimeException(
          String.format("Source file %s has already been added!", sourceFile));
    }
  }
}
