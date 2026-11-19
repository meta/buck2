/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apk.sdk;

import com.facebook.infer.annotation.Nullsafe;
import java.io.File;

@Nullsafe(Nullsafe.Mode.LOCAL)
public interface IArchiveBuilder {

  /**
   * Adds a file to the archive at a given path
   *
   * @param file the file to add
   * @param archivePath the path of the file inside the APK archive.
   * @throws ApkCreationException if an error occurred
   * @throws SealedApkException if the APK is already sealed.
   * @throws DuplicateFileException if a file conflicts with another already added to the APK at the
   *     same location inside the APK archive.
   */
  void addFile(File file, String archivePath)
      throws ApkCreationException, SealedApkException, DuplicateFileException;
}
