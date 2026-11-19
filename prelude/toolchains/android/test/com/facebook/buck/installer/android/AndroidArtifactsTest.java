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
import static org.junit.Assert.assertTrue;

import java.util.Set;
import org.junit.Test;

public class AndroidArtifactsTest {
  private static final long T0 = 1_700_000_000_000L;

  /** A payload is complete only once every artifact buck declared for it has turned up. */
  @Test
  public void aPayloadIsCompleteOnlyWhenAllOfItsArtifactsHaveArrived() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(
        Set.of(
            "native_library_exopackage_info_directory", "native_library_exopackage_info_metadata"));

    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0);
    assertFalse(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY));

    artifacts.recordFileArrival("native_library_exopackage_info_metadata", T0 + 1L);
    assertTrue(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY));
  }

  /**
   * A payload the build does not produce is never complete, so the early push does not treat it as
   * something to send.
   */
  @Test
  public void anUndeclaredPayloadIsNeverComplete() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(Set.of("options", "manifest", "my_app"));
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("manifest", T0);
    artifacts.recordFileArrival("my_app", T0);

    assertFalse(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY));
    assertFalse(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.RESOURCES));
    assertTrue(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.APK));
  }

  /** Before buck says what it will send, nothing is complete, however much has arrived. */
  @Test
  public void nothingIsCompleteBeforeBuckDeclaresWhatItWillSend() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_metadata", T0);

    assertFalse(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY));
  }

  /** Payloads complete independently, which is what lets each be pushed on its own. */
  @Test
  public void oneIncompletePayloadDoesNotHoldBackAnother() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(
        Set.of(
            "resources_exopackage_res",
            "resources_exopackage_res_hash",
            "secondary_dex_exopackage_info_directory",
            "secondary_dex_exopackage_info_metadata"));
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    artifacts.recordFileArrival("resources_exopackage_res_hash", T0);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_directory", T0);

    assertTrue(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.RESOURCES));
    assertFalse(artifacts.hasAllArtifactsFor(AndroidArtifacts.ArtifactClass.SECONDARY_DEX));
  }

  /** Names buck declared but never sent, which is how a naming drift becomes visible. */
  @Test
  public void undeliveredArtifactsAreThoseDeclaredButNeverArrived() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(
        Set.of(
            "resources_exopackage_res",
            "resources_exopackage_res_hash",
            "secondary_dex_exopackage_info_directory"));
    artifacts.recordFileArrival("resources_exopackage_res", T0);

    assertEquals(
        Set.of("resources_exopackage_res_hash", "secondary_dex_exopackage_info_directory"),
        artifacts.undeliveredArtifacts());
  }

  /** Nothing outstanding once every declared artifact has turned up. */
  @Test
  public void nothingIsUndeliveredOnceEverythingHasArrived() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(
        Set.of("resources_exopackage_res", "resources_exopackage_res_hash"));
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    artifacts.recordFileArrival("resources_exopackage_res_hash", T0);

    assertTrue(artifacts.undeliveredArtifacts().isEmpty());
  }

  /** An arrival buck never declared is not undelivered; only the declared set is tracked. */
  @Test
  public void anUndeclaredArrivalIsNotReportedAsUndelivered() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(Set.of("resources_exopackage_res"));
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_directory", T0);

    assertTrue(artifacts.undeliveredArtifacts().isEmpty());
  }

  /** Nothing left to wait for once every declared artifact has turned up. */
  @Test
  public void allArtifactsArrivedOnceEveryDeclaredOneHas() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.setExpectedArtifacts(Set.of("resources_exopackage_res", "apk"));
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    assertFalse(artifacts.allArtifactsArrived());

    artifacts.recordFileArrival("apk", T0);
    assertTrue(artifacts.allArtifactsArrived());
  }

  /** Before buck declares anything, nothing has arrived as far as this is concerned. */
  @Test
  public void nothingHasArrivedBeforeBuckDeclaresWhatItWillSend() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    artifacts.recordFileArrival("apk", T0);

    assertFalse(artifacts.allArtifactsArrived());
  }
}
