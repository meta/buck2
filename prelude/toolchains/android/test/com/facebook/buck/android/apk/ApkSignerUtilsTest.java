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

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ApkSignerUtilsTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  @Test
  public void needsV1SignatureBelowApi24() {
    assertTrue(ApkSignerUtils.needsV1Signature(9));
    assertTrue(ApkSignerUtils.needsV1Signature(23));
    assertFalse(ApkSignerUtils.needsV1Signature(24));
    assertFalse(ApkSignerUtils.needsV1Signature(26));
  }

  @Test
  public void assumesV1SignatureWhenTheManifestCannotBeRead() throws Exception {
    File notAnApk = tmp.newFile("not-an-apk.apk");
    Files.write(notAnApk.toPath(), "not a zip".getBytes(StandardCharsets.UTF_8));
    assertTrue(ApkSignerUtils.needsV1Signature(notAnApk));
  }
}
