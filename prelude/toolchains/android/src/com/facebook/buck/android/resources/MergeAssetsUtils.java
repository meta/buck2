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

import com.facebook.buck.android.apkmodule.APKModule;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.io.Files;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.FileVisitOption;
import java.nio.file.FileVisitResult;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.Collections;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.regex.Pattern;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/** Utils for merging assets into an apk. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class MergeAssetsUtils {
  public enum BinaryType {
    APK,
    AAB,
  }

  // See
  // https://android.googlesource.com/platform/frameworks/base.git/+/nougat-release/tools/aapt/Package.cpp
  static final ImmutableSet<String> DEFAULT_NO_COMPRESS_EXTENSIONS =
      ImmutableSet.of(
          "jpg", "jpeg", "png", "gif", "wav", "mp2", "mp3", "ogg", "aac", "mpg", "mpeg", "mid",
          "midi", "smf", "jet", "rtttl", "imy", "xmf", "mp4", "m4a", "m4v", "3gp", "3gpp", "3g2",
          "3gpp2", "amr", "awb", "wma", "wmv", "webm", "mkv", "tflite");

  /**
   * Construct an APK containing assets. If a "baseApk" was provided, also include everything from
   * that APK.
   *
   * @param outputApk path to the output APK file
   * @param moduleAssetsOutputDir optional directory for module assets APKs (used for AAB builds)
   * @param baseApk optional path to existing APK containing resources to merge
   * @param root absolute path to the project root directory
   * @param moduleToAssetsDirectories map of module names to sets of asset directory paths
   * @param extraNoCompressExtensions set of file extensions or path patterns that should not be
   *     compressed (e.g., "zip", "bank/bank")
   * @param noCompressRegex optional regex pattern used to match asset paths that should not be
   *     compressed. This lets us support the same --no-compress-regex pattern used by the Android
   *     SDK aapt2 C++ tool upstream when building the base APK. Note that C++ uses std::regex, so
   *     this pattern is going to be ECMAScript regex.
   * @param binaryType type of binary being built (APK or AAB)
   * @param compressionLevel deflate level, from 1 to 9, of the entries that are compressed
   * @throws IOException if an error occurs during asset merging
   */
  public static void mergeAssets(
      Path outputApk,
      Optional<Path> moduleAssetsOutputDir,
      Optional<Path> baseApk,
      AbsPath root,
      ImmutableMap<String, ImmutableSet<RelPath>> moduleToAssetsDirectories,
      ImmutableSet<String> extraNoCompressExtensions,
      Optional<String> noCompressRegex,
      BinaryType binaryType,
      int compressionLevel)
      throws IOException {
    Preconditions.checkArgument(
        compressionLevel >= Deflater.BEST_SPEED && compressionLevel <= Deflater.BEST_COMPRESSION,
        "Compression level must be between %s and %s, got %s",
        Deflater.BEST_SPEED,
        Deflater.BEST_COMPRESSION,
        compressionLevel);
    ImmutableSet<String> allNoCompressExtensions =
        new ImmutableSet.Builder<String>()
            .addAll(DEFAULT_NO_COMPRESS_EXTENSIONS)
            .addAll(extraNoCompressExtensions)
            .build();
    Optional<Pattern> noCompressPattern = getNoCompressPattern(noCompressRegex);
    if (moduleAssetsOutputDir.isPresent()) {
      Preconditions.checkState(binaryType == BinaryType.AAB);
      java.nio.file.Files.createDirectories(moduleAssetsOutputDir.get());
    }

    try (ResourcesZipBuilder outputApkResources =
        new ResourcesZipBuilder(outputApk, /* addManifestIfMissing */ true)) {
      if (baseApk.isPresent()) {
        try (ZipFile base = new ZipFile(baseApk.get().toFile())) {
          for (ZipEntry inputEntry : Collections.list(base.entries())) {
            String apkPath = inputEntry.getName();
            // Only compress if aapt compressed it and the extension looks compressible.
            // This is a workaround for aapt2 compressing everything.
            boolean shouldCompress =
                inputEntry.getMethod() != ZipEntry.STORED
                    && !isNoCompress(apkPath, allNoCompressExtensions, noCompressPattern);
            try (InputStream stream = Objects.requireNonNull(base.getInputStream(inputEntry))) {
              outputApkResources.addEntry(
                  stream,
                  inputEntry.getSize(),
                  inputEntry.getCrc(),
                  inputEntry.getName(),
                  shouldCompress ? compressionLevel : 0,
                  inputEntry.isDirectory());
            }
          }
        }
      }

      for (Map.Entry<String, ImmutableSet<RelPath>> entry : moduleToAssetsDirectories.entrySet()) {
        String moduleName = entry.getKey();
        ImmutableMap<Path, Path> assets = getAllAssets(root, entry.getValue());
        boolean isRootModule = APKModule.isRootModule(moduleName);
        if (isRootModule || binaryType == BinaryType.APK) {
          Path assetsZipRoot =
              isRootModule
                  ? Paths.get("assets")
                  : Paths.get("assets").resolve(moduleName).resolve("assets");
          addAllAssets(
              outputApkResources,
              assets,
              assetsZipRoot,
              allNoCompressExtensions,
              noCompressPattern,
              compressionLevel);
        } else {
          Path moduleDir = moduleAssetsOutputDir.orElseThrow().resolve(moduleName);
          java.nio.file.Files.createDirectory(moduleDir);
          try (ResourcesZipBuilder moduleOutputApk =
              new ResourcesZipBuilder(
                  moduleDir.resolve("assets.ap_"), /* addManifestIfMissing */ false)) {
            Path assetsZipRoot = Paths.get("assets").resolve(moduleName);
            addAllAssets(
                moduleOutputApk,
                assets,
                assetsZipRoot,
                allNoCompressExtensions,
                noCompressPattern,
                compressionLevel);
          }
        }
      }
    }
  }

  /**
   * Compile the optional --no-compress-regex pattern for matching complex asset paths.
   *
   * @param noCompressRegex optional raw regex pattern from additional_aapt_params
   *     (--no-compress-regex)
   * @return optional compiled pattern, or empty if no regex provided
   */
  private static Optional<Pattern> getNoCompressPattern(Optional<String> noCompressRegex) {
    if (noCompressRegex.isPresent() && !noCompressRegex.get().isEmpty()) {
      String regex = noCompressRegex.get();
      try {
        return Optional.of(Pattern.compile(regex));
      } catch (java.util.regex.PatternSyntaxException e) {
        throw new IllegalArgumentException(
            String.format(
                "Invalid regex pattern in additional_aapt_params --no-compress-regex: '%s'. Error:"
                    + " %s",
                regex, e.toString()),
            e);
      }
    }
    return Optional.empty();
  }

  /**
   * Checks if an asset path should not be compressed.
   *
   * @param apkPath the normalized path of the asset in the APK (using forward slashes)
   * @param allNoCompressExtensions set of all file extensions that should not be compressed
   * @param noCompressPattern optional compiled regex pattern for complex path matching
   * @return true if the asset should not be compressed, false otherwise
   */
  static boolean isNoCompress(
      String apkPath,
      ImmutableSet<String> allNoCompressExtensions,
      Optional<Pattern> noCompressPattern) {
    String extension = Files.getFileExtension(apkPath);
    if (allNoCompressExtensions.contains(extension)) {
      return true;
    }

    if (noCompressPattern.isPresent()) {
      return noCompressPattern.get().matcher(apkPath).find();
    }

    return false;
  }

  private static ImmutableMap<Path, Path> getAllAssets(
      AbsPath root, ImmutableSet<RelPath> assetsDirectories) throws IOException {
    ImmutableMap.Builder<Path, Path> assets = ImmutableMap.builder();

    for (RelPath assetDirectory : assetsDirectories) {
      AbsPath absolutePath = ProjectFilesystemUtils.getAbsPathForRelativePath(root, assetDirectory);

      ProjectFilesystemUtils.walkFileTree(
          root,
          assetDirectory.getPath(),
          ImmutableSet.of(FileVisitOption.FOLLOW_LINKS),
          new SimpleFileVisitor<>() {
            @Override
            public FileVisitResult visitFile(Path file, BasicFileAttributes attrs)
                throws IOException {
              Preconditions.checkState(
                  !Files.getFileExtension(file.toString()).equals("gz"),
                  "BUCK doesn't support adding .gz files to assets (%s).",
                  file);
              Path normalized = file.normalize();
              assets.put(absolutePath.getPath().relativize(normalized), normalized);
              return super.visitFile(file, attrs);
            }
          },
          ProjectFilesystemUtils.getEmptyIgnoreFilter());
    }

    return assets.build();
  }

  private static void addAllAssets(
      ResourcesZipBuilder output,
      ImmutableMap<Path, Path> assets,
      Path assetsZipRoot,
      ImmutableSet<String> allNoCompressExtensions,
      Optional<Pattern> noCompressPattern,
      int compressionLevel)
      throws IOException {
    for (Map.Entry<Path, Path> assetPaths : assets.entrySet()) {
      Path packagingPathForAsset = assetPaths.getKey();
      Path fullPathToAsset = assetPaths.getValue();
      int compression =
          isNoCompress(packagingPathForAsset.toString(), allNoCompressExtensions, noCompressPattern)
              ? 0
              : compressionLevel;
      byte[] assetBytes = java.nio.file.Files.readAllBytes(fullPathToAsset);
      CRC32 crc32 = new CRC32();
      crc32.update(assetBytes);
      try (InputStream assetStream = new ByteArrayInputStream(assetBytes)) {
        output.addEntry(
            assetStream,
            assetBytes.length,
            crc32.getValue(),
            assetsZipRoot.resolve(packagingPathForAsset).toString(),
            compression,
            false);
      }
    }
  }
}
