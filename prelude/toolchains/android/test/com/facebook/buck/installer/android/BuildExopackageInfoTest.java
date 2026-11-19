/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer.android;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.android.exopackage.IsolatedExopackageInfo;
import com.facebook.buck.core.filesystems.AbsPath;
import java.nio.file.Paths;
import java.util.Optional;
import java.util.Set;
import org.junit.Test;

/**
 * A streamed push carries only the payloads that are complete, so assembling a subset has to name
 * exactly those. Getting it wrong is invisible at runtime: the pairing checks throw, streaming
 * treats that as best effort, and the install pushes everything anyway.
 */
public class BuildExopackageInfoTest {

  private static AbsPath path(String name) {
    return AbsPath.of(Paths.get("/tmp").resolve(name).toAbsolutePath());
  }

  /** Every payload populated, so any subset asked for can be assembled. */
  private static AndroidArtifacts allPayloads() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setSecondaryDexExopackageInfoDirectory(Optional.of(path("dex")));
    artifacts.setSecondaryDexExopackageInfoMetadata(Optional.of(path("dex.metadata")));
    artifacts.setNativeLibraryExopackageInfoDirectory(Optional.of(path("libs")));
    artifacts.setNativeLibraryExopackageInfoMetadata(Optional.of(path("libs.metadata")));
    artifacts.setResourcesExopackageInfoAssets(Optional.of(path("assets")));
    artifacts.setResourcesExopackageInfoAssetsHash(Optional.of(path("assets.hash")));
    artifacts.setResourcesExopackageInfoRes(Optional.of(path("res")));
    artifacts.setResourcesExopackageInfoResHash(Optional.of(path("res.hash")));
    return artifacts;
  }

  @Test
  public void secondaryDexAloneBringsOnlySecondaryDex() {
    IsolatedExopackageInfo info =
        AndroidInstallerManager.buildExopackageInfo(
                allPayloads(), Set.of(AndroidArtifacts.ArtifactClass.SECONDARY_DEX))
            .get();

    assertTrue(info.getDexInfo().isPresent());
    assertFalse(info.getNativeLibsInfo().isPresent());
    assertFalse(info.getResourcesInfo().isPresent());
  }

  @Test
  public void nativeLibrariesAloneBringOnlyNativeLibraries() {
    IsolatedExopackageInfo info =
        AndroidInstallerManager.buildExopackageInfo(
                allPayloads(), Set.of(AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY))
            .get();

    assertFalse(info.getDexInfo().isPresent());
    assertTrue(info.getNativeLibsInfo().isPresent());
    assertFalse(info.getResourcesInfo().isPresent());
  }

  @Test
  public void resourcesAloneBringOnlyResources() {
    IsolatedExopackageInfo info =
        AndroidInstallerManager.buildExopackageInfo(
                allPayloads(), Set.of(AndroidArtifacts.ArtifactClass.RESOURCES))
            .get();

    assertFalse(info.getDexInfo().isPresent());
    assertFalse(info.getNativeLibsInfo().isPresent());
    assertTrue(info.getResourcesInfo().isPresent());
    assertEquals(2, info.getResourcesInfo().get().getResourcesPaths().size());
  }

  /** The install asks for all of them, and gets all of them. */
  @Test
  public void everyPayloadTogetherBringsAllThree() {
    IsolatedExopackageInfo info =
        AndroidInstallerManager.buildExopackageInfo(
                allPayloads(), AndroidArtifacts.ArtifactClass.EXOPACKAGE_PAYLOADS)
            .get();

    assertTrue(info.getDexInfo().isPresent());
    assertTrue(info.getNativeLibsInfo().isPresent());
    assertTrue(info.getResourcesInfo().isPresent());
  }

  /** Nothing complete yet means nothing to send, rather than an empty push. */
  @Test
  public void noPayloadsAtAllIsAbsentRatherThanEmpty() {
    assertFalse(AndroidInstallerManager.buildExopackageInfo(allPayloads(), Set.of()).isPresent());
  }

  /** A build with no exopackage payloads has nothing to assemble however much is asked for. */
  @Test
  public void aBuildWithoutPayloadsIsAbsent() {
    assertFalse(
        AndroidInstallerManager.buildExopackageInfo(
                new AndroidArtifacts(), AndroidArtifacts.ArtifactClass.EXOPACKAGE_PAYLOADS)
            .isPresent());
  }

  /**
   * A payload named while only half delivered is a bug in the readiness check, not something to
   * paper over: assembling it would push resources the metadata does not describe.
   */
  @Test
  public void aHalfDeliveredPayloadIsRejected() {
    AndroidArtifacts artifacts = allPayloads();
    artifacts.setResourcesExopackageInfoResHash(Optional.empty());

    assertThrows(
        IllegalStateException.class,
        () ->
            AndroidInstallerManager.buildExopackageInfo(
                artifacts, Set.of(AndroidArtifacts.ArtifactClass.RESOURCES)));
  }
}
