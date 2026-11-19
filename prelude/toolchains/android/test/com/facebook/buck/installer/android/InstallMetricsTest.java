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

import com.google.common.collect.ImmutableMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import org.junit.Test;

public class InstallMetricsTest {

  private static final long T0 = 1_700_000_000_000L;

  @Test
  public void noArrivalsProducesNoMetrics() {
    assertTrue(new InstallMetrics().summarise(T0, ImmutableMap.of()).isEmpty());
  }

  /** Every class reports both halves of its story, and classes read in the order they landed. */
  @Test
  public void eachClassReportsWhenItLandedAndHowLongItTookToTransfer() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("resources_exopackage_res", T0 + 2_000L);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0 + 1_000L);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_metadata", T0 + 4_000L);
    // Any name the installer does not recognise is the apk.
    artifacts.recordFileArrival("fbandroid_arm64_exo-native", T0 + 3_000L);

    timings.recordPush("native_library", 0L, 8_000L);
    timings.recordPush("resources", 8_000L, 9_500L);
    timings.recordPush("secondary_dex", 9_500L, 12_000L);
    timings.recordApkInstall(12_000L, 13_250L);

    Map<String, String> metrics = timings.summarise(T0 + 20_000L, artifacts.arrivals());

    assertEquals(
        List.of(
            "control_arrival_s",
            "control_transfer_s",
            "native_arrival_s",
            "native_transfer_s",
            "resources_arrival_s",
            "resources_transfer_s",
            "apk_arrival_s",
            "apk_transfer_s",
            "dex_arrival_s",
            "dex_transfer_s"),
        new ArrayList<>(metrics.keySet()).subList(0, 10));
    assertEquals("1.000", metrics.get("native_arrival_s"));
    assertEquals("8.000", metrics.get("native_transfer_s"));
    assertEquals("2.000", metrics.get("resources_arrival_s"));
    assertEquals("1.500", metrics.get("resources_transfer_s"));
    assertEquals("4.000", metrics.get("dex_arrival_s"));
    assertEquals("2.500", metrics.get("dex_transfer_s"));
    assertEquals("3.000", metrics.get("apk_arrival_s"));
    assertEquals("1.250", metrics.get("apk_transfer_s"));
    // Control artifacts are read on the host.
    assertEquals("0.000", metrics.get("control_transfer_s"));
  }

  /** A class is only ready once its last artifact lands. */
  @Test
  public void classArrivalUsesTheLatestArtifactInThatClass() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    artifacts.recordFileArrival("resources_exopackage_res_hash", T0 + 500L);
    artifacts.recordFileArrival("resources_exopackage_assets", T0 + 7_000L);
    artifacts.recordFileArrival("resources_exopackage_assets_hash", T0 + 1_000L);

    assertEquals(
        "7.000", timings.summarise(T0 + 9_000L, artifacts.arrivals()).get("resources_arrival_s"));
  }

  /** Retries must not move an artifact's recorded arrival later. */
  @Test
  public void firstArrivalWinsForARepeatedArtifact() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("manifest", T0);
    artifacts.recordFileArrival("manifest", T0 + 6_000L);

    assertEquals(
        "0.000", timings.summarise(T0 + 9_000L, artifacts.arrivals()).get("control_arrival_s"));
  }

  /** Metadata is derived from the payloads, so it belongs to no class of its own. */
  @Test
  public void metadataIsReportedSeparatelyFromTheArtifactClasses() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("my_app", T0);
    timings.recordPush("native_library", 0L, 5_000L);
    timings.recordPush("metadata", 5_000L, 5_400L);

    Map<String, String> metrics = timings.summarise(T0 + 6_000L, artifacts.arrivals());

    assertEquals("0.400", metrics.get("metadata_transfer_s"));
    assertFalse(metrics.containsKey("metadata_arrival_s"));
  }

  @Test
  public void anInstallWithoutMetadataDoesNotReportIt() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("my_app", T0);
    timings.recordApkInstall(0L, 3_000L);

    assertFalse(
        timings.summarise(T0 + 4_000L, artifacts.arrivals()).containsKey("metadata_transfer_s"));
  }

  /**
   * The trace this work started from: native libs and resources are ready immediately, dex and the
   * apk only 37s later, and payloads are pushed one at a time after everything has landed.
   */
  @Test
  public void criticalPathReplaysTheInstallWithoutTheWait() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    for (String control : new String[] {"options", "manifest", "cpu_filters"}) {
      artifacts.recordFileArrival(control, T0);
    }
    for (String resource :
        new String[] {
          "resources_exopackage_assets", "resources_exopackage_assets_hash",
          "resources_exopackage_res", "resources_exopackage_res_hash"
        }) {
      artifacts.recordFileArrival(resource, T0);
    }
    artifacts.recordFileArrival("native_library_exopackage_info_metadata", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0 + 1_000L);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_directory", T0 + 36_000L);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_metadata", T0 + 37_000L);
    artifacts.recordFileArrival("my_app", T0 + 37_000L);

    // Pushed one payload at a time, so the windows do not overlap.
    timings.recordDeviceSetup(0L, 1_000L);
    timings.recordPush("secondary_dex", 1_000L, 28_019L);
    timings.recordPush("native_library", 28_019L, 71_259L);
    timings.recordPush("resources", 71_259L, 89_741L);
    timings.recordPush("metadata", 89_741L, 90_055L);
    timings.recordApkInstall(90_055L, 102_191L);

    Map<String, String> metrics = timings.summarise(T0 + 140_000L, artifacts.arrivals());

    // 1s setup, then resources (available at 0), native (1s), dex (37s), metadata, then the apk.
    assertEquals("1.000", metrics.get("device_setup_s"));
    assertEquals("102.191", metrics.get("critical_path_s"));
    assertEquals("140.000", metrics.get("total_s"));
    assertEquals("37.809", metrics.get("potential_saving_s"));
  }

  /**
   * Payloads arriving apart is the case a per-payload cap gets wrong: resources is pushable from
   * the start even though native does not turn up until much later.
   */
  @Test
  public void staggeredArrivalsAreCreditedIndependently() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("manifest", T0);
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    artifacts.recordFileArrival("resources_exopackage_res_hash", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0 + 50_000L);
    artifacts.recordFileArrival("native_library_exopackage_info_metadata", T0 + 50_000L);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_directory", T0 + 60_000L);
    artifacts.recordFileArrival("my_app", T0 + 60_000L);

    timings.recordPush("resources", 0L, 10_000L);
    timings.recordPush("native_library", 10_000L, 20_000L);
    timings.recordPush("secondary_dex", 20_000L, 40_000L);
    timings.recordApkInstall(40_000L, 50_000L);

    Map<String, String> metrics = timings.summarise(T0 + 110_000L, artifacts.arrivals());

    // resources 0-10s, idle until native at 50s, native 50-60s, dex 60-80s, apk 80-90s.
    assertEquals("90.000", metrics.get("critical_path_s"));
    assertEquals("20.000", metrics.get("potential_saving_s"));
  }

  @Test
  public void anInstallThatNeverWaitedHasNothingToSave() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("manifest", T0);
    artifacts.recordFileArrival("my_app", T0 + 5_000L);
    timings.recordApkInstall(0L, 3_000L);

    Map<String, String> metrics = timings.summarise(T0 + 8_000L, artifacts.arrivals());

    assertEquals("8.000", metrics.get("critical_path_s"));
    assertEquals("0.000", metrics.get("potential_saving_s"));
  }

  /** Shards of one group overlapping is one payload transferring, not two. */
  @Test
  public void overlappingShardsOfOneGroupCountOnce() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0);
    timings.recordPush("native_library", 1_000L, 9_000L);
    timings.recordPush("native_library", 2_000L, 6_000L);

    assertEquals(
        "8.000", timings.summarise(T0 + 10_000L, artifacts.arrivals()).get("native_transfer_s"));
  }

  /**
   * Shards are recorded when they finish, so a long one started first is recorded last. Coalescing
   * has to order them itself; taking them as given would drop everything before the first record.
   */
  @Test
  public void shardsRecordedOutOfOrderStillCoalesce() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0);
    // The short shard finishes first, so it is recorded before the long one that started earlier.
    timings.recordPush("native_library", 1_000L, 5_000L);
    timings.recordPush("native_library", 0L, 30_000L);

    assertEquals(
        "30.000", timings.summarise(T0 + 40_000L, artifacts.arrivals()).get("native_transfer_s"));
  }

  /**
   * Shards queue behind other groups, so a group can be idle between its own shards. That gap is
   * not transfer time -- the span from first start to last end would charge the group for it.
   */
  @Test
  public void aGapBetweenShardsOfOneGroupIsNotTransferTime() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0);
    timings.recordPush("native_library", 0L, 10_000L);
    // Queued behind another group for ten seconds, then transferring again.
    timings.recordPush("native_library", 20_000L, 30_000L);

    assertEquals(
        "20.000", timings.summarise(T0 + 40_000L, artifacts.arrivals()).get("native_transfer_s"));
  }

  /**
   * Once payloads are pushed concurrently, replaying them one at a time would total more than the
   * install actually took and report a negative saving. The replay uses however many pushers the
   * install itself used.
   */
  @Test
  public void concurrentPushesAreReplayedConcurrently() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("options", T0);
    artifacts.recordFileArrival("manifest", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_directory", T0);
    artifacts.recordFileArrival("native_library_exopackage_info_metadata", T0);
    artifacts.recordFileArrival("secondary_dex_exopackage_info_directory", T0);
    artifacts.recordFileArrival("my_app", T0);

    // Both payloads in flight over the same window.
    timings.recordPush("native_library", 0L, 30_000L);
    timings.recordPush("secondary_dex", 0L, 30_000L);

    Map<String, String> metrics = timings.summarise(T0 + 30_000L, artifacts.arrivals());

    assertEquals("30.000", metrics.get("critical_path_s"));
    assertEquals("0.000", metrics.get("potential_saving_s"));
  }

  /**
   * A streamed push finished before the install's device phase began, so it is not part of the
   * device work already accounted for. Counting it would cancel out the correction that charges
   * whatever the replay does not model, and every streaming install would look free.
   */
  @Test
  public void aPushMadeBeforeTheDevicePhaseIsNotCountedAgainstDeviceWork() {
    AndroidArtifacts artifacts = new AndroidArtifacts();
    InstallMetrics timings = new InstallMetrics();
    artifacts.recordFileArrival("resources_exopackage_res", T0);
    artifacts.recordFileArrival("apk", T0);

    // Streamed while the build ran, then the install's own device phase.
    timings.recordPush("resources", T0, T0 + 10_000L);
    timings.recordDeviceWork(T0 + 20_000L, T0 + 40_000L);
    timings.recordPush("secondary_dex", T0 + 22_000L, T0 + 27_000L);
    timings.recordApkInstall(T0 + 30_000L, T0 + 35_000L);

    Map<String, String> metrics = timings.summarise(T0 + 40_000L, artifacts.arrivals());

    // modelled counts the apk install and the in-phase push only: 5s + 5s. The other 10s of device
    // work is unmodelled and must still be charged, so it survives on the critical path.
    assertEquals("30.000", metrics.get("critical_path_s"));
  }
}
