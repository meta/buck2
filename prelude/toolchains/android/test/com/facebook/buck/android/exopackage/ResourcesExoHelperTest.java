/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.core.filesystems.AbsPath;
import com.google.common.collect.ImmutableList;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class ResourcesExoHelperTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  private AbsPath root() throws Exception {
    return AbsPath.of(tmp.getRoot().toPath().toRealPath());
  }

  private IsolatedExopackageInfo.IsolatedResourcesInfo resourcesInfo(
      AbsPath resource, AbsPath hash) {
    return new IsolatedExopackageInfo.IsolatedResourcesInfo(
        ImmutableList.of(new IsolatedExopackageInfo.IsolatedExopackagePathAndHash(resource, hash)));
  }

  @Test
  public void resourceIsNamedByItsHashOnDevice() throws Exception {
    AbsPath root = root();
    AbsPath resource = root.resolve("exo_resources.apk");
    AbsPath hash = root.resolve("exo_resources.apk.hash");
    Files.write(resource.getPath(), new byte[] {1, 2, 3});
    Files.write(hash.getPath(), "abc123\n".getBytes(StandardCharsets.UTF_8));

    ResourcesExoHelper helper = new ResourcesExoHelper(root, resourcesInfo(resource, hash));

    assertEquals(
        ImmutableList.of(Path.of("resources", "abc123.apk")),
        ImmutableList.copyOf(helper.getFilesToInstall().keySet()));
  }

  /**
   * A missing hash file used to be filtered out silently. That map decides both what gets pushed
   * and what is still wanted, so dropping an entry collected the installed copy, never pushed the
   * replacement, and still reported success.
   */
  @Test
  public void missingHashFileFailsInsteadOfDroppingTheResource() throws Exception {
    AbsPath root = root();
    AbsPath resource = root.resolve("exo_resources.apk");
    AbsPath hash = root.resolve("exo_resources.apk.hash");
    Files.write(resource.getPath(), new byte[] {1, 2, 3});
    // Deliberately do not create the hash file.

    ResourcesExoHelper helper = new ResourcesExoHelper(root, resourcesInfo(resource, hash));

    IllegalStateException thrown =
        assertThrows(IllegalStateException.class, helper::getFilesToInstall);
    assertTrue(thrown.getMessage(), thrown.getMessage().contains("exo_resources.apk.hash"));
  }

  @Test
  public void missingHashFileAlsoFailsWhenComputingMetadata() throws Exception {
    AbsPath root = root();
    AbsPath resource = root.resolve("exo_resources.apk");
    AbsPath hash = root.resolve("exo_resources.apk.hash");
    Files.write(resource.getPath(), new byte[] {1, 2, 3});

    ResourcesExoHelper helper = new ResourcesExoHelper(root, resourcesInfo(resource, hash));

    assertThrows(IllegalStateException.class, helper::getMetadataToInstall);
  }
}
