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

import static java.nio.file.StandardCopyOption.REPLACE_EXISTING;

import com.android.tools.build.bundletool.model.BundleModule;
import com.facebook.buck.android.apk.sdk.ApkBuilder;
import com.facebook.buck.android.apk.sdk.ApkCreationException;
import com.facebook.buck.android.apk.sdk.ApkJarBuilder;
import com.facebook.buck.android.apk.sdk.DuplicateFileException;
import com.facebook.buck.android.apk.sdk.IArchiveBuilder;
import com.facebook.buck.android.apk.sdk.SealedApkException;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.PrintStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Enumeration;
import java.util.Objects;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import org.jetbrains.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class AndroidBundleUtils {

  public interface DuplicateFileListener {
    void onDuplicateFileAdded(String file);

    void onDuplicateSourceFileAdded(Path sourceFile);
  }

  public static void addModule(
      DuplicateFileListener duplicateFileListener,
      File moduleZipOutputFile,
      File fakeResourceApkFile,
      boolean packageMetaInfVersionFiles,
      @Nullable PrintStream verboseStream,
      boolean isBaseModule,
      @Nullable Path resourceApk,
      ImmutableSet<Path> dexFiles,
      ImmutableMap<Path, String> assetDirectories,
      ImmutableSet<Path> nativeLibraryDirectories,
      ImmutableSet<Path> zipFiles,
      ImmutableSet<Path> jarFilesThatMayContainResources,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      Set<String> excludedResources)
      throws ApkCreationException, SealedApkException, DuplicateFileException, IOException {

    ApkBuilder moduleBuilder =
        new ApkBuilder(
            moduleZipOutputFile,
            fakeResourceApkFile,
            null,
            packageMetaInfVersionFiles,
            verboseStream,
            excludedResources);
    addModuleFiles(
        moduleBuilder,
        isBaseModule,
        resourceApk,
        dexFiles,
        assetDirectories,
        nativeLibraryDirectories,
        zipFiles,
        jarFilesThatMayContainResources,
        addedFiles,
        addedSourceFiles,
        duplicateFileListener,
        excludedResources);
    // Build the APK
    moduleBuilder.sealApk();
  }

  private static void addModuleFiles(
      ApkBuilder moduleBuilder,
      boolean isBaseModule,
      @Nullable Path resourceApk,
      ImmutableSet<Path> dexFiles,
      ImmutableMap<Path, String> assetDirectories,
      ImmutableSet<Path> nativeLibraryDirectories,
      ImmutableSet<Path> zipFiles,
      ImmutableSet<Path> jarFilesThatMayContainResources,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      DuplicateFileListener duplicateFileListener,
      Set<String> excludedResources)
      throws ApkCreationException, DuplicateFileException, SealedApkException, IOException {

    for (Path dexFile : dexFiles) {
      addFile(
          moduleBuilder,
          dexFile,
          Paths.get(BundleModule.DEX_DIRECTORY.toString())
              .resolve(dexFile.getFileName())
              .toString(),
          addedFiles,
          addedSourceFiles,
          duplicateFileListener);
    }
    if (resourceApk != null) {
      if (isBaseModule) {
        packageFile(
            moduleBuilder,
            resourceApk.toFile(),
            addedFiles,
            addedSourceFiles,
            duplicateFileListener,
            excludedResources);
      } else {
        processFileForResource(
            moduleBuilder,
            resourceApk.toFile(),
            "",
            addedFiles,
            addedSourceFiles,
            duplicateFileListener);
      }
    }
    for (Path nativeLibraryDirectory : nativeLibraryDirectories) {
      addNativeLibraries(
          moduleBuilder,
          nativeLibraryDirectory.toFile(),
          addedFiles,
          addedSourceFiles,
          duplicateFileListener);
    }
    for (Path assetDirectory : assetDirectories.keySet()) {
      String subFolderName = assetDirectories.get(assetDirectory);
      addSourceFolder(
          moduleBuilder,
          assetDirectory.toFile(),
          // infer has a hard time detecting that subFolderName can't be nonnull
          subFolderName == null || subFolderName.isEmpty() ? "" : subFolderName,
          addedFiles,
          addedSourceFiles,
          duplicateFileListener);
    }
    for (Path zipFile : zipFiles) {
      if (!Files.exists(zipFile) || !Files.isRegularFile(zipFile)) {
        continue;
      }
      packageFile(
          moduleBuilder,
          zipFile.toFile(),
          addedFiles,
          addedSourceFiles,
          duplicateFileListener,
          excludedResources);
    }
    for (Path jarFile : jarFilesThatMayContainResources) {
      packageFile(
          moduleBuilder,
          jarFile.toFile(),
          addedFiles,
          addedSourceFiles,
          duplicateFileListener,
          excludedResources);
    }
  }

  private static void addFile(
      IArchiveBuilder builder,
      Path file,
      String destination,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      DuplicateFileListener duplicateFileListener)
      throws SealedApkException, DuplicateFileException, ApkCreationException {
    if (addedSourceFiles.contains((file))) {
      duplicateFileListener.onDuplicateSourceFileAdded(file);
      return;
    }

    if (addedFiles.contains(destination)) {
      duplicateFileListener.onDuplicateFileAdded(destination);
      return;
    }

    builder.addFile(file.toFile(), destination);
    addedFiles.add(destination);
    addedSourceFiles.add(file);
  }

  private static void addSourceFolder(
      ApkBuilder builder,
      File sourceFolder,
      String destination,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      DuplicateFileListener duplicateFileListener)
      throws ApkCreationException, DuplicateFileException, SealedApkException {
    if (sourceFolder.isDirectory()) {
      File[] files = sourceFolder.listFiles();
      if (files == null) {
        return;
      }
      for (File file : files) {
        processFileForResource(
            builder, file, destination, addedFiles, addedSourceFiles, duplicateFileListener);
      }
    } else {
      if (sourceFolder.exists()) {
        throw new ApkCreationException("%s is not a folder", sourceFolder);
      } else {
        throw new ApkCreationException("%s does not exist", sourceFolder);
      }
    }
  }

  private static void processFileForResource(
      IArchiveBuilder builder,
      File file,
      String path,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      DuplicateFileListener duplicateFileListener)
      throws DuplicateFileException, ApkCreationException, SealedApkException {
    path = Paths.get(path).resolve(file.getName()).toString();
    if (file.isDirectory() && ApkBuilder.checkFolderForPackaging(file.getName())) {
      if (file.getName().equals(BundleModule.RESOURCES_DIRECTORY.toString())) {
        path = BundleModule.RESOURCES_DIRECTORY.toString();
      }
      File[] files = file.listFiles();
      if (files == null) {
        return;
      }
      for (File contentFile : files) {
        processFileForResource(
            builder, contentFile, path, addedFiles, addedSourceFiles, duplicateFileListener);
      }
    } else if (!file.isDirectory() && ApkBuilder.checkFileForPackaging(file.getName())) {
      if (file.getName().startsWith("classes") && file.getName().endsWith(".dex")) {
        addFile(
            builder,
            file.toPath(),
            Paths.get(BundleModule.DEX_DIRECTORY.toString()).resolve(file.getName()).toString(),
            addedFiles,
            addedSourceFiles,
            duplicateFileListener);
      } else if (file.getName().equals(BundleModule.MANIFEST_FILENAME)) {
        addFile(
            builder,
            file.toPath(),
            Paths.get(BundleModule.MANIFEST_DIRECTORY.toString())
                .resolve(file.getName())
                .toString(),
            addedFiles,
            addedSourceFiles,
            duplicateFileListener);
      } else if (file.getName()
          .equals(BundleModule.SpecialModuleEntry.RESOURCE_TABLE.getPath().toString())) {
        addFile(
            builder,
            file.toPath(),
            Paths.get(file.getName()).toString(),
            addedFiles,
            addedSourceFiles,
            duplicateFileListener);
      } else {
        addFile(builder, file.toPath(), path, addedFiles, addedSourceFiles, duplicateFileListener);
      }
    }
  }

  private static void addNativeLibraries(
      ApkBuilder builder,
      File nativeFolder,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      DuplicateFileListener duplicateFileListener)
      throws ApkCreationException, SealedApkException, DuplicateFileException {
    if (!nativeFolder.isDirectory()) {
      if (nativeFolder.exists()) {
        throw new ApkCreationException("%s is not a folder", nativeFolder);
      } else {
        throw new ApkCreationException("%s does not exist", nativeFolder);
      }
    }
    File[] abiList = nativeFolder.listFiles();
    if (abiList == null) {
      return;
    }
    for (File abi : abiList) {
      if (!abi.isDirectory()) {
        continue;
      }
      File[] libs = abi.listFiles();
      if (libs == null) {
        continue;
      }
      for (File lib : libs) {
        if (!builder.shouldAddNativeLib(lib)) {
          continue;
        }
        Path libPath =
            Paths.get(BundleModule.LIB_DIRECTORY.toString())
                .resolve(abi.getName())
                .resolve(lib.getName());

        addFile(
            builder,
            lib.toPath(),
            libPath.toString(),
            addedFiles,
            addedSourceFiles,
            duplicateFileListener);
      }
    }
  }

  private static void packageFile(
      ApkBuilder builder,
      File original,
      Set<String> addedFiles,
      Set<Path> addedSourceFiles,
      DuplicateFileListener duplicateFileListener,
      Set<String> excludedResources)
      throws IOException, ApkCreationException, DuplicateFileException, SealedApkException {
    if (addedSourceFiles.contains(original.toPath())) {
      duplicateFileListener.onDuplicateSourceFileAdded(original.toPath());
      return;
    }
    try (ZipFile zipFile = new ZipFile(original)) {
      Enumeration<? extends ZipEntry> zipEntryEnumeration = zipFile.entries();
      while (zipEntryEnumeration.hasMoreElements()) {
        ZipEntry entry = zipEntryEnumeration.nextElement();

        if (!excludedResources.contains(entry.getName()) && isEntryPackageable(entry)) {
          String location = resolveFileInModule(entry);
          String destination = Paths.get(location).resolve(entry.getName()).toString();
          if (entry.isDirectory()) {
            destination += "/";
          }
          addFile(
              builder,
              convertZipEntryToFile(zipFile, entry).toPath(),
              destination,
              addedFiles,
              addedSourceFiles,
              duplicateFileListener);
        }
      }
    }
  }

  /**
   * Checks whether a folder and its content is valid for packaging into the .apk as standard Java
   * resource.
   *
   * @param folderName the name of the folder.
   */
  private static boolean checkFolderForPackaging(String folderName) {
    return !folderName.equalsIgnoreCase("nonJvmMain")
        // nonJvmMain is only useful when build multiplatform and we are building an APK here
        // commonMain folder is part of multiple AndroidX libraries, exclude it to avoid
        // duplicate file error
        && !folderName.equalsIgnoreCase("commonMain")
        // nativeMain holds platform-specific code for native environments like iOS and macOS,
        // exclude it to avoid duplicate file error
        // see https://kotlinlang.org/docs/multiplatform-discover-project.html
        && !folderName.equalsIgnoreCase("nativeMain");
  }

  /**
   * Defines if a zip entry should be packaged in the final bundle.
   *
   * @param entry
   * @return true if entry should be packaged
   */
  private static boolean isEntryPackageable(ZipEntry entry) {
    // split the path into segments.
    String[] segments = entry.getName().split("/");

    // empty path? skip to next entry.
    if (segments.length == 0) {
      return false;
    }

    // Check each folders to make sure they should be included.
    // Folders like CVS, .svn, etc.. should already have been excluded from the
    // jar file, but we need to exclude some other folder (like /META-INF) so
    // we check anyway.
    for (int i = 0; i < segments.length - 1; i++) {
      if (!checkFolderForPackaging(segments[i])) {
        return false;
      }
    }

    return isFileEntryPackageable(entry);
  }

  private static boolean isFileEntryPackageable(ZipEntry entry) {
    if (!ApkBuilder.checkFileForPackaging(entry.getName())) {
      return false;
    }

    if (entry.getName().startsWith("META-INF")) {
      return ApkJarBuilder.ALLOWLISTED_META_INF_SERVICES_FILES.contains(entry.getName());
    } else {
      return true;
    }
  }

  private static String resolveFileInModule(ZipEntry entry) {
    String location;
    String fileSeparator = "/";
    String empty = "";

    if (entry.getName().equals(BundleModule.MANIFEST_FILENAME)) {
      location = BundleModule.MANIFEST_DIRECTORY.toString();

    } else if (entry.getName().startsWith(BundleModule.LIB_DIRECTORY.toString() + fileSeparator)
        || entry.getName().startsWith(BundleModule.RESOURCES_DIRECTORY.toString() + fileSeparator)
        || entry.getName().startsWith(BundleModule.ASSETS_DIRECTORY.toString() + fileSeparator)
        || entry.getName().startsWith(BundleModule.DEX_DIRECTORY.toString() + fileSeparator)
        || entry.getName().endsWith(".pb")) {
      // They are already in the right folder
      location = empty;

    } else {
      location = BundleModule.ROOT_DIRECTORY.toString();
    }

    return location;
  }

  private static File convertZipEntryToFile(ZipFile zipFile, ZipEntry ze) throws IOException {
    Path tempFilePath = Files.createTempFile("tempRes", ".txt");
    File tempFile = tempFilePath.toFile();
    tempFile.deleteOnExit();
    try (InputStream in = zipFile.getInputStream(ze)) {
      Files.copy(Objects.requireNonNull(in), tempFilePath, REPLACE_EXISTING);
    }
    return tempFile;
  }
}
