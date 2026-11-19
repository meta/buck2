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

import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Collections;
import java.util.Objects;
import java.util.regex.Pattern;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/**
 * Digests of local content in the forms an Android device reports for its own copy: the MD5 that
 * {@code md5sum} prints, and the manifest digest from an APK's signature file.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ContentDigests {
  /**
   * MD5 of {@code file} as lowercase hex, comparable with {@link
   * com.facebook.buck.android.exopackage.AndroidDevice#getContentHash}.
   */
  public static String getContentHash(File file) throws IOException {
    MessageDigest digest = md5();
    try (InputStream input = new FileInputStream(file)) {
      byte[] buffer = new byte[8192];
      int read;
      while ((read = input.read(buffer)) >= 0) {
        digest.update(buffer, 0, read);
      }
    }
    return toHex(digest.digest());
  }

  /**
   * MD5 of {@code contents} as lowercase hex, comparable with {@link
   * com.facebook.buck.android.exopackage.AndroidDevice#getContentHash}.
   */
  public static String getContentHash(byte[] contents) {
    return toHex(md5().digest(contents));
  }

  private static MessageDigest md5() {
    try {
      return MessageDigest.getInstance("MD5");
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException(e);
    }
  }

  private static String toHex(byte[] bytes) {
    StringBuilder hex = new StringBuilder();
    for (byte b : bytes) {
      hex.append(String.format("%02x", b & 0xFF));
    }
    return hex.toString();
  }

  public static String getJarManifestDigest(String packagePath) throws IOException {
    Pattern signatureFilePattern = Pattern.compile("META-INF/[A-Z]+\\.SF");

    ZipFile packageZip = null;
    try {
      packageZip = new ZipFile(packagePath);
      // For each file in the zip.
      for (ZipEntry entry : Collections.list(packageZip.entries())) {
        // Ignore non-signature files.
        if (!signatureFilePattern.matcher(entry.getName()).matches()) {
          continue;
        }

        BufferedReader sigContents = null;
        try {
          sigContents =
              new BufferedReader(
                  new InputStreamReader(Objects.requireNonNull(packageZip.getInputStream(entry))));
          // For each line in the signature file.
          while (true) {
            String line = sigContents.readLine();
            if (line == null || line.equals("")) {
              throw new IllegalArgumentException(
                  "Failed to find manifest digest in " + entry.getName());
            }
            String prefix_sha1 = "SHA1-Digest-Manifest: ";
            String prefix_sha256 = "SHA-256-Digest-Manifest: ";
            if (line.startsWith(prefix_sha1)) {
              return line.substring(prefix_sha1.length());
            } else if (line.startsWith(prefix_sha256)) {
              return line.substring(prefix_sha256.length());
            }
          }
        } finally {
          if (sigContents != null) {
            sigContents.close();
          }
        }
      }
    } finally {
      if (packageZip != null) {
        packageZip.close();
      }
    }

    throw new IllegalArgumentException("Failed to find signature file.");
  }
}
