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

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;

import com.facebook.buck.android.apk.ApkSignerUtils;
import com.facebook.buck.android.apk.KeystoreProperties;
import java.io.File;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.util.Arrays;
import java.util.Collections;
import java.util.Map;
import java.util.TreeMap;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/**
 * The manifest digest that the installer compares, for apks signed as Buck signs them: the same apk
 * keeps its digest, and a changed resource changes it. The unsigned apks in {@code
 * manifest_digest/} come from aapt2 and differ from {@code base.apk} in one resource each (see
 * BUCK).
 */
public class ContentDigestsTest {

  @Rule public TemporaryFolder temp = new TemporaryFolder();

  @Test
  public void theSameApkSignedAgainHasTheSameManifestDigest() throws Exception {
    assertEquals(manifestDigest("base.apk"), manifestDigest("base.apk"));
  }

  @Test
  public void aChangedStringChangesTheManifestDigest() throws Exception {
    assertOnlyEntryDiffers("base.apk", "string.apk", "resources.arsc");
    assertNotEquals(manifestDigest("base.apk"), manifestDigest("string.apk"));
  }

  @Test
  public void aChangedDrawableChangesTheManifestDigest() throws Exception {
    assertOnlyEntryDiffers("base.apk", "drawable.apk", "res/drawable/icon.png");
    assertNotEquals(manifestDigest("base.apk"), manifestDigest("drawable.apk"));
  }

  /** Signs {@code apk} with {@link ApkSignerUtils}, as Buck signs apks, and reads its digest. */
  private String manifestDigest(String apk) throws Exception {
    File signed = temp.newFile();
    try (InputStream keystore = resource("keystore.p12")) {
      ApkSignerUtils.signApkFile(
          copyOf(apk),
          signed,
          ApkSignerUtils.getSignerConfigs(
              new KeystoreProperties(Paths.get("keystore.p12"), "android", "android", "test"),
              keystore));
    }
    return ContentDigests.getJarManifestDigest(signed.getPath());
  }

  private void assertOnlyEntryDiffers(String apk, String other, String entry) throws Exception {
    Map<String, byte[]> entries = entries(apk);
    Map<String, byte[]> otherEntries = entries(other);
    assertEquals(entries.keySet(), otherEntries.keySet());
    for (String name : entries.keySet()) {
      if (name.equals(entry)) {
        assertFalse(name, Arrays.equals(entries.get(name), otherEntries.get(name)));
      } else {
        assertArrayEquals(name, entries.get(name), otherEntries.get(name));
      }
    }
  }

  private Map<String, byte[]> entries(String apk) throws Exception {
    Map<String, byte[]> entries = new TreeMap<>();
    try (ZipFile zip = new ZipFile(copyOf(apk))) {
      for (ZipEntry entry : Collections.list(zip.entries())) {
        try (InputStream contents = zip.getInputStream(entry)) {
          entries.put(entry.getName(), contents.readAllBytes());
        }
      }
    }
    return entries;
  }

  private File copyOf(String name) throws Exception {
    File copy = temp.newFile();
    try (InputStream contents = resource(name)) {
      Files.copy(contents, copy.toPath(), StandardCopyOption.REPLACE_EXISTING);
    }
    return copy;
  }

  private static InputStream resource(String name) {
    InputStream contents = ContentDigestsTest.class.getResourceAsStream("manifest_digest/" + name);
    if (contents == null) {
      throw new IllegalStateException("Missing test resource manifest_digest/" + name);
    }
    return contents;
  }
}
