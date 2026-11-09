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

import com.facebook.buck.android.AdbHelper;
import com.facebook.buck.android.IsolatedApkInfo;
import com.facebook.buck.android.exopackage.IsolatedExopackageInfo;
import com.facebook.buck.android.exopackage.SetDebugAppMode;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.installer.InstallCommand;
import com.facebook.buck.installer.InstallError;
import com.facebook.buck.installer.InstallId;
import com.facebook.buck.installer.InstallResult;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Preconditions;
import com.google.common.base.Throwables;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.Sets;
import com.google.common.util.concurrent.ThreadFactoryBuilder;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.logging.Level;
import java.util.logging.Logger; // NOPMD

/**
 * Coordinates an Android Install of an APK. We need three artifacts: the apk,
 * install_android_options.json (configuration options), and an AndroidManifest.xml
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class AndroidInstallerManager implements InstallCommand {

  private static final Logger LOG = Logger.getLogger(AndroidInstallerManager.class.getName());
  private final AndroidCommandLineOptions options;
  private final AndroidInstallErrorClassifier errorClassifier =
      AndroidInstallErrorClassifier.INSTANCE;
  private final Map<InstallId, InstallState> installs = new ConcurrentHashMap<>();

  private static final AbsPath ROOT_PATH = AbsPath.of(Paths.get(".").normalize().toAbsolutePath());

  private static final ExecutorService STREAM_EXECUTOR =
      Executors.newCachedThreadPool(
          new ThreadFactoryBuilder().setNameFormat("exopackage-stream-%d").setDaemon(true).build());

  private static final ImmutableMap<String, String> SHORT_TO_FULL_ABI_MAP =
      ImmutableMap.of(
          "armv7", "armeabi-v7a", "arm64", "arm64-v8a", "x86", "x86", "x86_64", "x86_64");

  AndroidInstallerManager(AndroidCommandLineOptions options) {
    this.options = options;
  }

  @Override
  public String name() {
    return "android";
  }

  /**
   * Coordinates the install artifacts needed for an install. The install_android_options.json is
   * parsed into a AndroidInstallApkOptions and the manifest is later set in the apkInstallOptions
   * as a separate field.
   */
  @Override
  public InstallResult fileReady(String artifactName, Path artifactPath, InstallId installId) {
    try {
      AndroidArtifacts androidArtifacts = installState(installId).artifacts();
      long arrivedAt = System.currentTimeMillis();

      // Path before arrival: an artifact counted as arrived while its path is still unset reads as
      // usable to anyone judging readiness from arrivals. Delivery is all this records -- what the
      // install then makes of the artifact is not the artifact's business.
      recordArtifactPath(installState(installId), artifactName, artifactPath);
      androidArtifacts.recordFileArrival(artifactName, arrivedAt);

      if (artifactName.equals("options")) {
        Optional<InstallError> adbMissing = checkAdbExists(androidArtifacts);
        if (adbMissing.isPresent()) {
          return InstallResult.error(adbMissing.get());
        }
        resolveDevices(installId, androidArtifacts);
      }
      Optional<InstallError> incompatible = checkAbiCompatibility(installId);
      if (incompatible.isPresent()) {
        return InstallResult.error(incompatible.get());
      }
      maybeStreamReadyPayloads(installId, androidArtifacts);
      return InstallResult.success();
    } catch (Exception err) {
      String errMsg = Throwables.getStackTraceAsString(err);
      LOG.log(
          Level.SEVERE,
          String.format(
              "Error installing %s from %s due to %s", artifactName, artifactPath, errMsg),
          err);
      return InstallResult.error(errorClassifier.fromErrorMessage(errMsg));
    }
  }

  @Override
  public void onInstallStarted(InstallId installId, Set<String> expectedArtifacts) {
    installState(installId).artifacts().setExpectedArtifacts(expectedArtifacts);
  }

  /**
   * Records an artifact as it arrives. The install_android_options.json is parsed into an
   * AndroidInstallApkOptions and the manifest is later set in the apkInstallOptions as a separate
   * field.
   */
  private void recordArtifactPath(
      InstallState installState, String artifactName, Path artifactPath) {
    AndroidArtifacts androidArtifacts = installState.artifacts();
    switch (artifactName) {
      case "cpu_filters":
        androidArtifacts.setApkAbis(readApkAbis(artifactPath));
        break;
      case "options":
        try {
          androidArtifacts.setApkOptions(
              new AndroidInstallApkOptions(artifactPath, options.adbExecutablePath));
        } catch (IOException e) {
          // Surfaces as an install failure: InstallerService turns this into an error response.
          throw new UncheckedIOException("Could not read install options " + artifactPath, e);
        }
        LOG.log(Level.INFO, androidArtifacts.getApkOptions().toString());
        break;
      case "manifest":
        androidArtifacts.setAndroidManifestPath(AbsPath.of(artifactPath));
        // Read now rather than when a later stage wants it: a manifest that cannot be parsed names
        // no package, and no part of the install can proceed without one.
        installState.setPackageName(AdbHelper.tryToExtractPackageNameFromManifest(artifactPath));
        break;
      case "secondary_dex_exopackage_info_directory":
        androidArtifacts.setSecondaryDexExopackageInfoDirectory(
            Optional.of(AbsPath.of(artifactPath)));
        break;
      case "secondary_dex_exopackage_info_metadata":
        androidArtifacts.setSecondaryDexExopackageInfoMetadata(
            Optional.of(AbsPath.of(artifactPath)));
        break;
      case "native_library_exopackage_info_directory":
        androidArtifacts.setNativeLibraryExopackageInfoDirectory(
            Optional.of(AbsPath.of(artifactPath)));
        break;
      case "native_library_exopackage_info_metadata":
        androidArtifacts.setNativeLibraryExopackageInfoMetadata(
            Optional.of(AbsPath.of(artifactPath)));
        break;
      case "resources_exopackage_assets":
        androidArtifacts.setResourcesExopackageInfoAssets(Optional.of(AbsPath.of(artifactPath)));
        break;
      case "resources_exopackage_assets_hash":
        androidArtifacts.setResourcesExopackageInfoAssetsHash(
            Optional.of(AbsPath.of(artifactPath)));
        break;
      case "resources_exopackage_res":
        androidArtifacts.setResourcesExopackageInfoRes(Optional.of(AbsPath.of(artifactPath)));
        break;
      case "resources_exopackage_res_hash":
        androidArtifacts.setResourcesExopackageInfoResHash(Optional.of(AbsPath.of(artifactPath)));
        break;
      default:
        androidArtifacts.setApk(AbsPath.of(artifactPath));
        break;
    }
  }

  @Override
  public InstallResult allFilesReady(InstallId installId) {
    try {
      AndroidArtifacts androidArtifacts = installState(installId).artifacts();

      // Before anything else, and before any path can return: a push cannot be stopped once it has
      // started, so the install waits it out rather than leaving one writing into the directories
      // it is about to write itself.
      installState(installId).streamedPushes().sealAndAwait();

      ImmutableSet<String> undelivered = androidArtifacts.undeliveredArtifacts();
      if (!undelivered.isEmpty()) {
        LOG.log(
            Level.WARNING,
            String.format(
                "Install %s: buck declared these artifacts and never sent them, so their payload"
                    + " never counted as complete and was pushed by the install rather than"
                    + " ahead of it: %s",
                installId.getValue(), undelivered));
      }

      Optional<InstallError> adbMissing = checkAdbExists(androidArtifacts);
      if (adbMissing.isPresent()) {
        return InstallResult.error(adbMissing.get());
      }

      AdbHelper adbHelper = resolveDevices(installId, androidArtifacts);
      ImmutableSet<String> departed = adbHelper.departedSerials();
      if (!departed.isEmpty()) {
        return InstallResult.error(
            AndroidInstallException.Companion.devicesDeparted(departed).getInstallError());
      }

      Optional<IsolatedExopackageInfo> isolatedExopackageInfo =
          buildExopackageInfo(androidArtifacts, AndroidArtifacts.ArtifactClass.EXOPACKAGE_PAYLOADS);

      AndroidInstall androidInstaller =
          new AndroidInstall(
              LOG,
              ROOT_PATH,
              options,
              IsolatedApkInfo.of(
                  androidArtifacts.getAndroidManifestPath(), androidArtifacts.getApk()),
              isolatedExopackageInfo,
              installId,
              installState(installId),
              adbHelper);
      return androidInstaller.installApk();

    } catch (Exception err) {
      String errMsg = Throwables.getStackTraceAsString(err);
      LOG.log(Level.SEVERE, String.format("Install error due to %s", errMsg), err);
      return InstallResult.error(errorClassifier.fromErrorMessage(errMsg));
    }
  }

  /**
   * Starts pushing exopackage payloads as soon as they are complete, rather than waiting for the
   * apk.
   *
   * <p>Payloads finish at different points in the build, so the device would otherwise sit idle
   * until the last of them. Readiness is judged per payload, so one that finishes early does not
   * wait behind one that finishes late; payloads that are ready together go in a single push, which
   * pays for the device setup once rather than once each.
   */
  private void maybeStreamReadyPayloads(InstallId installId, AndroidArtifacts androidArtifacts) {
    if (options.cleanUp) {
      // This install only uninstalls, so nothing it sends would ever be read -- and the payload
      // would outlive the app it belongs to.
      return;
    }
    // Check if it is too early to start streaming.
    AdbHelper adbHelper = installState(installId).adbHelper();
    if (androidArtifacts.getAndroidManifestPath() == null || adbHelper == null) {
      // No package name, or no devices to reach yet. Before anything is claimed, because a claim
      // is permanent: claiming a payload that cannot be sent leaves it to the install to push.
      return;
    }
    // Check if it is too late to start streaming.
    if (androidArtifacts.allArtifactsArrived()) {
      // Nothing left to wait for, so there is no idle device time to fill.
      return;
    }
    Set<AndroidArtifacts.ArtifactClass> ready =
        AndroidArtifacts.ArtifactClass.EXOPACKAGE_PAYLOADS.stream()
            .filter(androidArtifacts::hasAllArtifactsFor)
            .collect(ImmutableSet.toImmutableSet());
    installState(installId)
        .streamedPushes()
        .dispatch(
            ready,
            pending ->
                STREAM_EXECUTOR.submit(
                    () -> streamPayloads(installState(installId), adbHelper, pending)));
  }

  private void streamPayloads(
      InstallState state, AdbHelper adbHelper, Set<AndroidArtifacts.ArtifactClass> payloads) {
    try {
      Optional<IsolatedExopackageInfo> exopackageInfo =
          buildExopackageInfo(state.artifacts(), payloads);
      if (exopackageInfo.isEmpty()) {
        return;
      }
      // Read here rather than where the push is dispatched: an unreadable manifest is this
      // method's problem to swallow, not a reason to fail the artifact that triggered it.
      String packageName = state.packageName();
      if (packageName == null) {
        LOG.log(Level.WARNING, "No package name in the manifest; leaving the payloads to install");
        return;
      }
      // The devices the install itself will use, so a payload never streams somewhere the install
      // is not going. Counted like any other push: whatever lands here the install then finds
      // already present, so no payload is recorded twice.
      adbHelper.streamExopackagePayloads(ROOT_PATH, exopackageInfo.get(), packageName);
    } catch (InterruptedException e) {
      // Only reachable through adbCall's signature; nothing interrupts these threads. Rethrown so
      // the future carries the failure rather than reporting a push that did not happen.
      LOG.log(Level.WARNING, String.format("Streaming %s was interrupted", payloads), e);
      throw new RuntimeException(e);
    } catch (Exception e) {
      // Best effort. Whatever did not make it is pushed by the install, which lists the directory
      // and so sees exactly what is missing.
      LOG.log(
          Level.WARNING,
          String.format("Could not stream %s; the install will push instead", payloads),
          e);
    }
  }

  /**
   * Assembles the payloads named in {@code payloads} from the artifacts that have arrived.
   *
   * <p>A payload left out is reported absent rather than empty, so the caller pushes only what it
   * asked for. Only payloads known to be complete may be named: a half-delivered one fails the
   * pairing checks below.
   */
  @VisibleForTesting
  static Optional<IsolatedExopackageInfo> buildExopackageInfo(
      AndroidArtifacts androidArtifacts, Set<AndroidArtifacts.ArtifactClass> payloads) {
    boolean includeSecondaryDex = payloads.contains(AndroidArtifacts.ArtifactClass.SECONDARY_DEX);
    boolean includeNativeLibrary = payloads.contains(AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY);
    boolean includeResources = payloads.contains(AndroidArtifacts.ArtifactClass.RESOURCES);
    Optional<AbsPath> secondaryDexExopackageInfoDirectory =
        includeSecondaryDex
            ? androidArtifacts.getSecondaryDexExopackageInfoDirectory()
            : Optional.empty();
    Optional<AbsPath> secondaryDexExopackageInfoMetadata =
        includeSecondaryDex
            ? androidArtifacts.getSecondaryDexExopackageInfoMetadata()
            : Optional.empty();
    Optional<AbsPath> nativeLibraryExopackageInfoDirectory =
        includeNativeLibrary
            ? androidArtifacts.getNativeLibraryExopackageInfoDirectory()
            : Optional.empty();
    Optional<AbsPath> nativeLibraryExopackageInfoMetadata =
        includeNativeLibrary
            ? androidArtifacts.getNativeLibraryExopackageInfoMetadata()
            : Optional.empty();
    ImmutableList.Builder<IsolatedExopackageInfo.IsolatedExopackagePathAndHash> pathAndHashBuilder =
        ImmutableList.builder();
    if (includeResources) {
      // Assets are optional for a build, but a resource and its hash always ship together. Without
      // this, a half-delivered pair is indistinguishable from a build that has no assets at all.
      Preconditions.checkState(
          androidArtifacts.getResourcesExopackageInfoAssets().isPresent()
              == androidArtifacts.getResourcesExopackageInfoAssetsHash().isPresent(),
          "Exopackage resource assets and their hash must be present together");
      Preconditions.checkState(
          androidArtifacts.getResourcesExopackageInfoRes().isPresent()
              == androidArtifacts.getResourcesExopackageInfoResHash().isPresent(),
          "Exopackage resources and their hash must be present together");
      androidArtifacts
          .getResourcesExopackageInfoAssets()
          .ifPresent(
              assets ->
                  pathAndHashBuilder.add(
                      new IsolatedExopackageInfo.IsolatedExopackagePathAndHash(
                          assets, androidArtifacts.getResourcesExopackageInfoAssetsHash().get())));
      androidArtifacts
          .getResourcesExopackageInfoRes()
          .ifPresent(
              res ->
                  pathAndHashBuilder.add(
                      new IsolatedExopackageInfo.IsolatedExopackagePathAndHash(
                          res, androidArtifacts.getResourcesExopackageInfoResHash().get())));
    }
    ImmutableList<IsolatedExopackageInfo.IsolatedExopackagePathAndHash> exopackageResources =
        pathAndHashBuilder.build();

    Optional<IsolatedExopackageInfo> isolatedExopackageInfo = Optional.empty();
    if (secondaryDexExopackageInfoDirectory.isPresent()
        || secondaryDexExopackageInfoMetadata.isPresent()
        || nativeLibraryExopackageInfoDirectory.isPresent()
        || nativeLibraryExopackageInfoMetadata.isPresent()
        || !exopackageResources.isEmpty()) {
      Preconditions.checkState(
          secondaryDexExopackageInfoDirectory.isPresent()
              == secondaryDexExopackageInfoMetadata.isPresent());
      Optional<IsolatedExopackageInfo.IsolatedDexInfo> dexInfo =
          secondaryDexExopackageInfoDirectory.map(
              directory ->
                  new IsolatedExopackageInfo.IsolatedDexInfo(
                      secondaryDexExopackageInfoMetadata.get(), directory));

      Preconditions.checkState(
          nativeLibraryExopackageInfoDirectory.isPresent()
              == nativeLibraryExopackageInfoMetadata.isPresent());
      Optional<IsolatedExopackageInfo.IsolatedNativeLibsInfo> nativeLibsInfo =
          nativeLibraryExopackageInfoDirectory.map(
              absPath ->
                  new IsolatedExopackageInfo.IsolatedNativeLibsInfo(
                      nativeLibraryExopackageInfoMetadata.get(), absPath));

      Optional<IsolatedExopackageInfo.IsolatedResourcesInfo> resourcesInfo;
      if (!exopackageResources.isEmpty()) {
        resourcesInfo =
            Optional.of(new IsolatedExopackageInfo.IsolatedResourcesInfo(exopackageResources));
      } else {
        resourcesInfo = Optional.empty();
      }

      isolatedExopackageInfo =
          Optional.of(new IsolatedExopackageInfo(dexInfo, nativeLibsInfo, resourcesInfo));
    }
    return isolatedExopackageInfo;
  }

  /**
   * Checked before anything runs adb: running a missing adb fails with a stack trace that names
   * neither adb nor the fix.
   */
  private Optional<InstallError> checkAdbExists(AndroidArtifacts androidArtifacts) {
    String adbPath = androidArtifacts.getApkOptions().adbExecutable;
    if (Files.exists(Paths.get(adbPath))) {
      return Optional.empty();
    }
    return Optional.of(
        errorClassifier.fromErrorMessage(
            String.format(
                "Could not find `adb` in PATH, and could not find `adb` "
                    + "at %s. Please add `adb` to your PATH",
                adbPath)));
  }

  /**
   * Fixes the devices this install targets, as soon as there are options to reach them with.
   *
   * <p>Pinned here rather than at install time so that everything the install does reaches one set
   * of devices. A device connected after this point is not installed to; one that disconnects
   * before the install fails it, rather than being dropped silently.
   */
  private AdbHelper resolveDevices(InstallId installId, AndroidArtifacts androidArtifacts) {
    InstallState state = installState(installId);
    // Per install, not process wide: resolution asks adb, and two installs arriving together have
    // no reason to wait for each other.
    return state.resolveDevices(
        () -> {
          AdbHelper adbHelper =
              AdbHelperFactory.create(
                  LOG,
                  options,
                  androidArtifacts.getApkOptions(),
                  options.skipSetDebugApp ? SetDebugAppMode.SKIP : SetDebugAppMode.SET,
                  state.metrics());
          // Asking now is what fixes the set: the helper resolves its devices once, on
          // first use, and answers from that for the rest of the install. Leaving it to
          // whoever asks first would move the instant this install is pinned to.
          ImmutableSet<String> serials =
              adbHelper.getDevices(true).stream()
                  .map(device -> device.getSerialNumber())
                  .collect(ImmutableSet.toImmutableSet());
          LOG.log(Level.INFO, "Install targets " + serials);
          return adbHelper;
        });
  }

  private InstallState installState(InstallId installId) {
    return installs.computeIfAbsent(installId, ignore -> new InstallState());
  }

  /**
   * The ABIs the apk carries native code for, from the cpu filters buck sent.
   *
   * <p>Empty when the filters name nothing this installer recognises, or cannot be read at all.
   * Either way the check has nothing to compare and the install goes ahead -- an ABI we cannot
   * determine is not treated as incompatible.
   */
  private static ImmutableSet<String> readApkAbis(Path cpuFiltersPath) {
    try {
      // A filter this installer does not know is dropped rather than carried through as null: it
      // would never match a device ABI, and would reach the user as the word "null".
      return Files.readAllLines(cpuFiltersPath).stream()
          .map(abi -> SHORT_TO_FULL_ABI_MAP.get(abi.trim()))
          .filter(Objects::nonNull)
          .collect(ImmutableSet.toImmutableSet());
    } catch (IOException e) {
      LOG.log(Level.WARNING, "Could not read the apk cpu filters; skipping the ABI check", e);
      return ImmutableSet.of();
    }
  }

  /**
   * Fails early when the apk has no native code the targeted devices can run.
   *
   * <p>Offered every arrival and answers for itself: it needs the cpu filters and the devices,
   * which turn up in either order, and does nothing until both are there. Empty means compatible,
   * or that no device would report an ABI -- unknown is not treated as incompatible.
   */
  private Optional<InstallError> checkAbiCompatibility(InstallId installId) {
    InstallState state = installState(installId);
    ImmutableSet<String> apkAbis = state.artifacts().getApkAbis();
    AdbHelper adbHelper = state.adbHelper();
    if (apkAbis == null || adbHelper == null) {
      return Optional.empty();
    }
    if (apkAbis.isEmpty()) {
      // No native code this installer recognises, so there is nothing a device could be wrong for.
      return Optional.empty();
    }
    ImmutableMap<String, ImmutableSet<String>> abisByDevice = adbHelper.deviceAbisBySerial();
    if (abisByDevice.isEmpty()) {
      return Optional.empty();
    }
    // Every device, not their union: an install reaches all of them, so one that cannot run the
    // apk fails it, and failing here is the point -- the alternative is finding out after the
    // build.
    ImmutableList<String> incompatible =
        abisByDevice.entrySet().stream()
            .filter(device -> Sets.intersection(apkAbis, device.getValue()).isEmpty())
            .map(device -> device.getKey() + " (" + String.join(",", device.getValue()) + ")")
            .sorted()
            .collect(ImmutableList.toImmutableList());
    if (incompatible.isEmpty()) {
      return Optional.empty();
    }
    return Optional.of(
        new InstallError(
            String.format(
                "You are trying to install an APK with native libraries built for %s, onto "
                    + "device(s) that cannot run any of them: %s.",
                String.join(",", apkAbis), String.join("; ", incompatible)),
            AndroidInstallErrorTag.INCOMPATIBLE_NATIVE_LIB));
  }
}
