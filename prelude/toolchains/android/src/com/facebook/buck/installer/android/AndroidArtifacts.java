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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.Sets;
import java.util.EnumMap;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import javax.annotation.Nullable;

/**
 * Holds android install related artifacts (apk options, manifest path, etc)
 *
 * <p>Shared across the gRPC handler threads that deliver artifacts, the threads that push them, and
 * the install that reads them, so every member is guarded by this object's monitor.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class AndroidArtifacts {
  @Nullable private AbsPath androidManifestPath;
  @Nullable private AndroidInstallApkOptions apkOptions;
  @Nullable private AbsPath apk;
  @Nullable private ImmutableSet<String> apkAbis;
  private Optional<AbsPath> secondaryDexExopackageInfoDirectory = Optional.empty();
  private Optional<AbsPath> secondaryDexExopackageInfoMetadata = Optional.empty();
  private Optional<AbsPath> nativeLibraryExopackageInfoDirectory = Optional.empty();
  private Optional<AbsPath> nativeLibraryExopackageInfoMetadata = Optional.empty();
  private Optional<AbsPath> resourcesExopackageInfoAssets = Optional.empty();
  private Optional<AbsPath> resourcesExopackageInfoAssetsHash = Optional.empty();
  private Optional<AbsPath> resourcesExopackageInfoRes = Optional.empty();
  private Optional<AbsPath> resourcesExopackageInfoResHash = Optional.empty();
  // Artifact name -> wall-clock arrival.
  private final Map<String, Long> fileArrivalMillis = new HashMap<>();
  // What buck said it would send, split by payload. A class with no entry is one this build does
  // not produce, which is why it can never be complete.
  private final Map<ArtifactClass, Set<String>> expectedByClass =
      new EnumMap<>(ArtifactClass.class);

  /** Records which artifacts buck said it would send, before any of them arrive. */
  public synchronized void setExpectedArtifacts(Set<String> expectedArtifacts) {
    expectedByClass.clear();
    for (String artifactName : expectedArtifacts) {
      expectedByClass
          .computeIfAbsent(ArtifactClass.of(artifactName), unused -> new HashSet<>())
          .add(artifactName);
    }
  }

  /**
   * True once every artifact this build declared for {@code artifactClass} has arrived, and false
   * if it declared none.
   *
   * <p>Checked against what buck said it would send, not against what happens to be on disk: assets
   * are optional for a build, so their absence is otherwise indistinguishable from their not having
   * turned up yet.
   */
  public synchronized boolean hasAllArtifactsFor(ArtifactClass artifactClass) {
    Set<String> declared = expectedByClass.get(artifactClass);
    return declared != null && fileArrivalMillis.keySet().containsAll(declared);
  }

  /**
   * True once every artifact buck declared has arrived, and false if it declared none.
   *
   * <p>Independent of how names bucket into classes: it asks whether anything is still coming, not
   * which payload it belongs to.
   */
  public synchronized boolean allArtifactsArrived() {
    return !expectedByClass.isEmpty() && undeliveredArtifacts().isEmpty();
  }

  /**
   * Artifacts buck declared but never delivered.
   *
   * <p>Only meaningful once buck says it has sent everything; before that an artifact is missing
   * simply because it has not arrived. A name here means the two sides disagree about what an
   * artifact is called, which leaves its class permanently incomplete and its payload unstreamed.
   */
  public synchronized ImmutableSet<String> undeliveredArtifacts() {
    return expectedByClass.values().stream()
        .flatMap(Set::stream)
        .filter(artifactName -> !fileArrivalMillis.containsKey(artifactName))
        .collect(ImmutableSet.toImmutableSet());
  }

  /**
   * Records that {@code artifactName} was delivered by buck at {@code timestampMillis}. Must be
   * called when the file is actually received, not when the installer gets around to consuming it,
   * or every artifact is stamped with the same instant and the spans below all collapse to zero.
   */
  public synchronized void recordFileArrival(String artifactName, long timestampMillis) {
    fileArrivalMillis.putIfAbsent(artifactName, timestampMillis);
  }

  /** When each artifact was delivered, for whoever is measuring the install. */
  public synchronized ImmutableMap<String, Long> arrivals() {
    return ImmutableMap.copyOf(fileArrivalMillis);
  }

  /** Groups install artifacts by the exopackage payload they belong to. */
  enum ArtifactClass {
    SECONDARY_DEX("dex"),
    NATIVE_LIBRARY("native"),
    RESOURCES("resources"),
    CONTROL("control"),
    APK("apk");

    final String metricName;

    ArtifactClass(String metricName) {
      this.metricName = metricName;
    }

    /** The classes that are exopackage payloads, each pushable to the device on its own. */
    static final ImmutableSet<ArtifactClass> EXOPACKAGE_PAYLOADS =
        Sets.immutableEnumSet(SECONDARY_DEX, NATIVE_LIBRARY, RESOURCES);

    /** The class whose arrival gates pushing {@code pushGroup}, or null if it is derived. */
    @Nullable
    static ArtifactClass forPushGroup(String pushGroup) {
      switch (pushGroup) {
        case "secondary_dex":
          return SECONDARY_DEX;
        case "native_library":
          return NATIVE_LIBRARY;
        case "resources":
          return RESOURCES;
        default:
          return null;
      }
    }

    static ArtifactClass of(String artifactName) {
      if (artifactName.startsWith("secondary_dex")) {
        return SECONDARY_DEX;
      }
      if (artifactName.startsWith("native_library")) {
        return NATIVE_LIBRARY;
      }
      if (artifactName.startsWith("resources_exopackage")) {
        return RESOURCES;
      }
      if (artifactName.equals("options")
          || artifactName.equals("manifest")
          || artifactName.equals("cpu_filters")) {
        return CONTROL;
      }
      // AndroidInstallerManager#fileReady treats any unrecognised name as the apk.
      return APK;
    }
  }

  public synchronized void setAndroidManifestPath(AbsPath androidManifestPath) {
    this.androidManifestPath = androidManifestPath;
  }

  @Nullable
  public synchronized AbsPath getAndroidManifestPath() {
    return this.androidManifestPath;
  }

  public synchronized void setApkOptions(AndroidInstallApkOptions apkOptions) {
    this.apkOptions = apkOptions;
  }

  @Nullable
  public synchronized AndroidInstallApkOptions getApkOptions() {
    return this.apkOptions;
  }

  /**
   * The ABIs the apk carries native code for, or null until buck has sent the cpu filters. Empty
   * means the filters named nothing this installer recognises.
   */
  @Nullable
  public synchronized ImmutableSet<String> getApkAbis() {
    return apkAbis;
  }

  public synchronized void setApkAbis(ImmutableSet<String> apkAbis) {
    this.apkAbis = apkAbis;
  }

  @Nullable
  public synchronized AbsPath getApk() {
    return apk;
  }

  public synchronized void setApk(AbsPath apk) {
    this.apk = apk;
  }

  public synchronized Optional<AbsPath> getSecondaryDexExopackageInfoDirectory() {
    return secondaryDexExopackageInfoDirectory;
  }

  public synchronized void setSecondaryDexExopackageInfoDirectory(
      Optional<AbsPath> secondaryDexExopackageInfoDirectory) {
    this.secondaryDexExopackageInfoDirectory = secondaryDexExopackageInfoDirectory;
  }

  public synchronized Optional<AbsPath> getSecondaryDexExopackageInfoMetadata() {
    return secondaryDexExopackageInfoMetadata;
  }

  public synchronized void setSecondaryDexExopackageInfoMetadata(
      Optional<AbsPath> secondaryDexExopackageInfoMetadata) {
    this.secondaryDexExopackageInfoMetadata = secondaryDexExopackageInfoMetadata;
  }

  public synchronized Optional<AbsPath> getNativeLibraryExopackageInfoDirectory() {
    return nativeLibraryExopackageInfoDirectory;
  }

  public synchronized void setNativeLibraryExopackageInfoDirectory(
      Optional<AbsPath> nativeLibraryExopackageInfoDirectory) {
    this.nativeLibraryExopackageInfoDirectory = nativeLibraryExopackageInfoDirectory;
  }

  public synchronized Optional<AbsPath> getNativeLibraryExopackageInfoMetadata() {
    return nativeLibraryExopackageInfoMetadata;
  }

  public synchronized void setNativeLibraryExopackageInfoMetadata(
      Optional<AbsPath> nativeLibraryExopackageInfoMetadata) {
    this.nativeLibraryExopackageInfoMetadata = nativeLibraryExopackageInfoMetadata;
  }

  public synchronized Optional<AbsPath> getResourcesExopackageInfoAssets() {
    return resourcesExopackageInfoAssets;
  }

  public synchronized void setResourcesExopackageInfoAssets(
      Optional<AbsPath> resourcesExopackageInfoAssets) {
    this.resourcesExopackageInfoAssets = resourcesExopackageInfoAssets;
  }

  public synchronized Optional<AbsPath> getResourcesExopackageInfoAssetsHash() {
    return resourcesExopackageInfoAssetsHash;
  }

  public synchronized void setResourcesExopackageInfoAssetsHash(
      Optional<AbsPath> resourcesExopackageInfoAssetsHash) {
    this.resourcesExopackageInfoAssetsHash = resourcesExopackageInfoAssetsHash;
  }

  public synchronized Optional<AbsPath> getResourcesExopackageInfoRes() {
    return resourcesExopackageInfoRes;
  }

  public synchronized void setResourcesExopackageInfoRes(
      Optional<AbsPath> resourcesExopackageInfoRes) {
    this.resourcesExopackageInfoRes = resourcesExopackageInfoRes;
  }

  public synchronized Optional<AbsPath> getResourcesExopackageInfoResHash() {
    return resourcesExopackageInfoResHash;
  }

  public synchronized void setResourcesExopackageInfoResHash(
      Optional<AbsPath> resourcesExopackageInfoResHash) {
    this.resourcesExopackageInfoResHash = resourcesExopackageInfoResHash;
  }
}
