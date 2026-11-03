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

import com.facebook.buck.android.apk.sdk.ApkBuilder;
import com.facebook.buck.android.apk.sdk.ApkCreationException;
import com.facebook.buck.android.apk.sdk.DuplicateFileException;
import com.facebook.buck.android.apk.sdk.SealedApkException;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.io.PrintStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.util.function.Predicate;
import org.jetbrains.annotations.Nullable;

/** A class that provides methods useful for building an APK. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ApkBuilderUtils {

  /**
   * The type of a keystore created via the {@code jarsigner} command in Sun/Oracle Java. See
   * http://docs.oracle.com/javase/7/docs/technotes/guides/security/StandardNames.html#KeyStore.
   */
  public static void buildApk(
      Path resourceApk,
      Path pathToOutputApkFile,
      Path dexFile,
      ImmutableSet<Path> assetDirectories,
      ImmutableSet<Path> nativeLibraryDirectories,
      ImmutableSet<Path> zipFiles,
      ImmutableSet<Path> jarFilesThatMayContainResources,
      Path pathToKeystore,
      KeystoreProperties keystoreProperties,
      boolean packageMetaInfVersionFiles,
      @Nullable PrintStream output,
      ImmutableSet<String> excludedResources,
      Predicate<String> storedPaths)
      throws IOException,
          ApkCreationException,
          KeyStoreException,
          NoSuchAlgorithmException,
          SealedApkException,
          UnrecoverableKeyException,
          DuplicateFileException {
    ApkBuilder builder =
        new ApkBuilder(
            pathToOutputApkFile.toFile(),
            resourceApk.toFile(),
            dexFile.toFile(),
            packageMetaInfVersionFiles,
            output,
            excludedResources,
            storedPaths);
    for (Path nativeLibraryDirectory : nativeLibraryDirectories.stream().sorted().toList()) {
      builder.addNativeLibraries(nativeLibraryDirectory.toFile());
    }
    for (Path assetDirectory : assetDirectories.stream().sorted().toList()) {
      builder.addSourceFolder(assetDirectory.toFile());
    }
    for (Path zipFile : zipFiles.stream().sorted().toList()) {
      // TODO(natthu): Skipping silently is bad. These should really be assertions.
      if (Files.exists(zipFile) && Files.isRegularFile(zipFile)) {
        builder.addZipFile(zipFile.toFile());
      }
    }
    for (Path jarFileThatMayContainResources :
        jarFilesThatMayContainResources.stream().sorted().toList()) {
      builder.addResourcesFromJar(jarFileThatMayContainResources.toFile());
    }
    builder.sealApk();
  }
}
