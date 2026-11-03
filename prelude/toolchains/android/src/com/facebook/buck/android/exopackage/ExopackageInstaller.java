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

import com.facebook.buck.android.AdbHelper;
import com.facebook.buck.android.AndroidInstallPrinter;
import com.facebook.buck.android.ContentDigests;
import com.facebook.buck.android.IsolatedApkInfo;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.installer.android.AndroidInstallException;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.util.NamedTemporaryFile;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Preconditions;
import com.google.common.base.Splitter;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableListMultimap;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableMultimap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSortedMap;
import com.google.common.collect.ImmutableSortedSet;
import com.google.common.collect.Ordering;
import com.google.common.io.Closer;
import com.google.common.util.concurrent.ThreadFactoryBuilder;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.function.Function;

/**
 * ExopackageInstaller manages the installation of apps with the "exopackage" flag set to true.
 *
 * <p>Two ways in, one way down. {@link #doInstall} is the whole install; {@link #streamPayloads}
 * sends a subset of the payloads while the build is still running. Both reach the device through
 * {@code pushMissingFiles}, the only thing here that moves payload bytes.
 *
 * <p>Three names for sets of files:
 *
 * <ul>
 *   <li>{@code filesOnDevice} -- what listing the data root found already there.
 *   <li>{@code filesToDelete} -- on the device from previous installs and not wanted anymore.
 *   <li>{@code filesToPush} -- wanted by one payload and not on the device.
 * </ul>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ExopackageInstaller {

  private static final Logger LOG = Logger.get(ExopackageInstaller.class);

  // When there are a small number of files to delete, it's faster (we issue
  // fewer rm commands over adb) if we group them by the data root, so
  //   cd /data/local/tmp/exopackage/foo && rm resources/bar.txt native-libs/baz.txt
  // But when there are more files, because of the way RealAndroidDevice splits up
  // commandlines to avoid hitting limits, it's more efficient to group by the
  // subdirectories, so
  //   cd /data/local/tmp/exopackage/foo/resources && rm bar.txt bap.txt boz.txt
  //   cd /data/local/tmp/exopackage/foo/native-libs && rm baz.txt zog.txt
  // We pick a heuristic number of files (10) at which to change behavior for this based
  // on these assumptions:
  //    approx available commandline length = 800
  //    max length of a path from the dataRoot for a well known app = 77
  private static final int RM_GROUPING_THRESHOLD = 10;

  private static final int MAX_CONCURRENT_PUSHES = 8;

  private static final long TARGET_SHARD_BYTES = 32L * 1024L * 1024L;

  private static final ExecutorService PUSH_EXECUTOR =
      Executors.newFixedThreadPool(
          MAX_CONCURRENT_PUSHES,
          new ThreadFactoryBuilder().setNameFormat("exopackage-push-%d").setDaemon(true).build());

  public static final Path EXOPACKAGE_INSTALL_ROOT = Paths.get("/data/local/tmp/exopackage/");

  private final IsolatedExopackageInfo exoInfo;
  private final AndroidInstallPrinter androidPrinter;
  private final AbsPath rootPath;
  private final AndroidDevice device;
  private final String packageName;
  private final Optional<String> buck2BuildUuid;
  private final Path dataRoot;
  private final InstallTimings timings;
  private boolean appStopped;

  public ExopackageInstaller(
      IsolatedExopackageInfo exoInfo,
      AndroidInstallPrinter androidPrinter,
      AbsPath rootPath,
      String packageName,
      AndroidDevice device,
      Optional<String> buck2BuildUuid) {
    this(
        exoInfo,
        androidPrinter,
        rootPath,
        packageName,
        device,
        buck2BuildUuid,
        InstallTimings.NONE);
  }

  public ExopackageInstaller(
      IsolatedExopackageInfo exoInfo,
      AndroidInstallPrinter androidPrinter,
      AbsPath rootPath,
      String packageName,
      AndroidDevice device,
      Optional<String> buck2BuildUuid,
      InstallTimings timings) {
    this.timings = timings;
    this.exoInfo = exoInfo;
    this.androidPrinter = androidPrinter;
    this.rootPath = rootPath;
    this.device = device;
    this.packageName = packageName;
    this.dataRoot = EXOPACKAGE_INSTALL_ROOT.resolve(packageName);
    this.buck2BuildUuid = buck2BuildUuid;

    Preconditions.checkArgument(AdbHelper.PACKAGE_NAME_PATTERN.matcher(packageName).matches());
  }

  /**
   * Installs an apk with its exopackage payloads.
   *
   * <p>Stops the running app only when something requires it: ahead of a change it could observe,
   * or, with {@code restartApp}, so that the activity started after the install gets a fresh
   * process. Replacing the apk needs no stop of its own, since the package manager kills the app to
   * replace it.
   */
  public void doInstall(
      IsolatedApkInfo isolatedApkInfo,
      SetDebugAppMode setDebugAppMode,
      boolean restartApp,
      boolean forceQueryable)
      throws Exception {
    if (setDebugAppMode == SetDebugAppMode.SET) {
      device.setDebugAppPackageName(packageName);
    }
    if (exopackageEnabled()) {
      long setupStart = System.currentTimeMillis();
      prepareDataRoot();
      ImmutableSortedSet<Path> filesOnDevice = device.listDirRecursive(dataRoot);
      timings.recordDeviceSetup(setupStart, System.currentTimeMillis());
      ImmutableList<ResolvedExoPayload> payloads = resolveExoPayloads();
      // The running app may still load a file about to be deleted, and reads the metadata about to
      // be rewritten, so it is stopped before either. New payload files need no stop: they are
      // named by content hash, and nothing reads one until metadata names it.
      ImmutableSortedSet<Path> filesToDelete = filesToDelete(filesOnDevice, payloads);
      if (!filesToDelete.isEmpty()) {
        stopApp();
      }

      // Reclaim space before pushing so a device that is already full can free room for the
      // payload. Best effort on purpose: neither step is needed for the install to be correct.
      // Scratch is read by nothing, and filesToDelete is disjoint from filesToPush, so the new app
      // gets every file it needs either way -- a failure here leaves unreferenced files on the
      // device and costs disk, not correctness. That disjointness is also why the push below still
      // reads the pre-delete listing: nothing deleted is a file any payload asks about. A push that
      // still does not fit fails with ENOSPC, which is tagged NO_SPACE_LEFT_ON_DEVICE.
      try {
        device.rmStaleFiles(packageName);
        deleteFiles(filesToDelete);
      } catch (Exception e) {
        LOG.warn(e, "Could not reclaim exopackage space for %s; continuing", packageName);
      }
      pushMissingFiles(filesOnDevice, payloads);

      // Metadata is what the app reads to find these files, so it must not land before them.
      ImmutableMap<Path, String> metadata =
          payloads.stream()
              .flatMap(payload -> payload.metadataToInstall.entrySet().stream())
              .collect(ImmutableMap.toImmutableMap(Map.Entry::getKey, Map.Entry::getValue));
      if (metadataChanged(filesOnDevice, metadata)) {
        stopApp();
        installMetadata(metadata);
      }
    }
    if (buck2BuildUuid.isPresent()) {
      device.installBuildUuidFile(
          AdbHelper.BUILD_METADATA_INSTALL_ROOT, packageName, buck2BuildUuid.get());
    }
    boolean apkInstalled = installApkIfNecessary(isolatedApkInfo, forceQueryable);
    if (restartApp && !apkInstalled) {
      stopApp();
    }
  }

  /** Installs the apk if its contents or requested queryability differ. */
  private boolean installApkIfNecessary(IsolatedApkInfo isolatedApkInfo, boolean forceQueryable)
      throws Exception {
    File apk = isolatedApkInfo.getApkPath().toFile();

    if (AdbHelper.shouldAppBeInstalled(device, packageName, apk, androidPrinter, forceQueryable)) {
      long installStart = System.currentTimeMillis();
      boolean success =
          device.installApkOnDevice(
              apk,
              /* installViaSd= */ false,
              /* quiet= */ false,
              /* verifyTempWritable= */ true,
              /* stagedInstallMode= */ false,
              /* userId= */ null,
              /* allowFastDeploy= */ false,
              packageName,
              forceQueryable);
      timings.recordApkInstall(installStart, System.currentTimeMillis());
      if (!success) {
        throw AndroidInstallException.Companion.adbCommandFailedException(
            "Installing Apk failed.", null);
      }
      return true;
    }
    return false;
  }

  /** Stops the running app, at most once per install. */
  private void stopApp() throws Exception {
    if (!appStopped) {
      device.stopPackage(packageName);
      appStopped = true;
    }
  }

  /**
   * Whether any metadata file on the device differs from what this install would write. Compared by
   * content hash, so it is byte-exact and independent of how adb transfers text.
   */
  private boolean metadataChanged(
      ImmutableSortedSet<Path> filesOnDevice, ImmutableMap<Path, String> metadata) {
    for (Map.Entry<Path, String> entry : metadata.entrySet()) {
      if (!filesOnDevice.contains(entry.getKey())) {
        return true;
      }
      String onDevice;
      try {
        onDevice = device.getContentHash(dataRoot.resolve(entry.getKey()).toString());
      } catch (Exception e) {
        LOG.info("Could not hash %s; rewriting it: %s", entry.getKey(), e.getMessage());
        return true;
      }
      String wanted =
          ContentDigests.getContentHash(entry.getValue().getBytes(StandardCharsets.UTF_8));
      if (!onDevice.equalsIgnoreCase(wanted)) {
        return true;
      }
    }
    return false;
  }

  /**
   * The exopackage payloads for this install, with their contents resolved exactly once.
   *
   * <p>Resolving a payload parses its metadata file and, for native libs, queries the device for
   * its ABIs. The delete and push phases both need the same answers, so they share one resolution
   * rather than recomputing it.
   */
  private ImmutableList<ResolvedExoPayload> resolveExoPayloads() throws Exception {
    ImmutableList.Builder<ResolvedExoPayload> payloads = ImmutableList.builder();

    Optional<IsolatedExopackageInfo.IsolatedDexInfo> dexInfo = exoInfo.getDexInfo();
    if (dexInfo.isPresent()) {
      payloads.add(new ResolvedExoPayload(new DexExoHelper(rootPath, dexInfo.get())));
    }

    Optional<IsolatedExopackageInfo.IsolatedNativeLibsInfo> nativeLibsInfo =
        exoInfo.getNativeLibsInfo();
    if (nativeLibsInfo.isPresent()) {
      payloads.add(
          new ResolvedExoPayload(
              new NativeExoHelper(this::getDeviceAbis, rootPath, nativeLibsInfo.get())));
    }

    Optional<IsolatedExopackageInfo.IsolatedResourcesInfo> resourcesInfo =
        exoInfo.getResourcesInfo();
    if (resourcesInfo.isPresent()) {
      payloads.add(new ResolvedExoPayload(new ResourcesExoHelper(rootPath, resourcesInfo.get())));
    }

    return payloads.build();
  }

  private List<String> getDeviceAbis() {
    try {
      return device.getDeviceAbis();
    } catch (Exception e) {
      throw AndroidInstallException.Companion.adbCommandFailedException(
          "Unable to communicate with device.", e.getMessage());
    }
  }

  /** Every path this install wants on the device; anything else under the data root is stale. */
  /**
   * Pushes whatever of {@code payloads} the device does not already have.
   *
   * <p>Every payload byte reaches a device through here, whether it goes while the build is still
   * running or as part of the install.
   */
  private void pushMissingFiles(
      ImmutableSortedSet<Path> filesOnDevice, ImmutableList<ResolvedExoPayload> payloads)
      throws Exception {
    ImmutableMap.Builder<ResolvedExoPayload, ImmutableSortedMap<Path, Path>> transfers =
        ImmutableMap.builder();
    for (ResolvedExoPayload payload : payloads) {
      transfers.put(payload, filesToPush(filesOnDevice, payload.filesToInstall));
    }
    ImmutableMap<ResolvedExoPayload, ImmutableSortedMap<Path, Path>> filesToTransfer =
        transfers.build();

    ImmutableList.Builder<PushShard> shards = ImmutableList.builder();
    filesToTransfer.forEach(
        (payload, files) ->
            shards.addAll(
                splitIntoShards(payload.type, files, rootPath, dataRoot, TARGET_SHARD_BYTES)));
    pushShards(shards.build());
  }

  /**
   * Pushes payload content ahead of the install proper.
   *
   * <p>Writes no metadata, collects no stale files and does not touch the apk: all three need the
   * complete artifact set, and until metadata names them the pushed files are inert. The install
   * lists the directory afterwards, so anything landed here is seen as already present and skipped
   * -- which is what makes this safe to run more than once, and safe to fail.
   */
  public void streamPayloads() throws Exception {
    if (!exopackageEnabled()) {
      return;
    }
    // Not recorded as device setup: this happens while the build is still running, so charging it
    // to the install would report time the install never spent.
    prepareDataRoot();
    pushMissingFiles(device.listDirRecursive(dataRoot), resolveExoPayloads());
  }

  /** Makes the data root usable. */
  private void prepareDataRoot() throws Exception {
    device.mkDirP(dataRoot.toString());
    device.fixRootDir(dataRoot.toString());
  }

  @VisibleForTesting
  static ImmutableSortedMap<Path, Path> filesToPush(
      ImmutableSortedSet<Path> filesOnDevice, ImmutableMap<Path, Path> filesToInstall) {
    return filesToInstall.entrySet().stream()
        .filter(entry -> !filesOnDevice.contains(entry.getKey()))
        .collect(
            ImmutableSortedMap.toImmutableSortedMap(
                Ordering.natural(), Map.Entry::getKey, Map.Entry::getValue));
  }

  /**
   * Splits a payload into roughly equal chunks by size.
   *
   * <p>A single `adb push` is limited by per-stream round trips rather than by bandwidth or by the
   * host, so concurrent pushes scale close to linearly. Sharding by bytes rather than by payload
   * matters because native libs alone are over half the total, and pushing one payload per stream
   * leaves that stream setting the wall time on its own.
   */
  @VisibleForTesting
  static ImmutableList<PushShard> splitIntoShards(
      String filesType,
      ImmutableSortedMap<Path, Path> filesToInstall,
      AbsPath rootPath,
      Path dataRoot,
      long targetShardBytes) {
    ImmutableList.Builder<PushShard> shards = ImmutableList.builder();
    ImmutableMap.Builder<Path, Path> current = ImmutableMap.builder();
    long currentBytes = 0L;
    boolean currentIsEmpty = true;

    for (Map.Entry<Path, Path> file : filesToInstall.entrySet()) {
      Path localPath = rootPath.resolve(file.getValue()).getPath();
      current.put(dataRoot.resolve(file.getKey()), localPath);
      currentBytes += localPath.toFile().length();
      currentIsEmpty = false;
      // A shard can never be smaller than a single file, so a payload of one huge file stays whole.
      if (currentBytes >= targetShardBytes) {
        shards.add(new PushShard(filesType, current.build()));
        current = ImmutableMap.builder();
        currentBytes = 0L;
        currentIsEmpty = true;
      }
    }
    if (!currentIsEmpty) {
      shards.add(new PushShard(filesType, current.build()));
    }
    return shards.build();
  }

  /** Pushes every shard, up to {@link #MAX_CONCURRENT_PUSHES} at a time. */
  private void pushShards(ImmutableList<PushShard> shards) throws Exception {
    if (shards.isEmpty()) {
      return;
    }
    // Create every destination directory up front: shards from one payload share a directory, and
    // concurrent mkdir -p of the same path is pointless work at best.
    ImmutableSet<Path> destinationDirs =
        shards.stream()
            .flatMap(shard -> shard.installPaths.keySet().stream())
            .map(Path::getParent)
            .filter(Objects::nonNull)
            .collect(ImmutableSet.toImmutableSet());
    for (Path destinationDir : destinationDirs) {
      device.mkDirP(destinationDir.toString());
    }

    List<Future<Void>> pushes = new ArrayList<>(shards.size());
    for (PushShard shard : shards) {
      pushes.add(
          PUSH_EXECUTOR.submit(
              () -> {
                pushShard(shard);
                return null;
              }));
    }
    awaitAll(pushes);
  }

  private void pushShard(PushShard shard) throws Exception {
    long start = System.currentTimeMillis();
    device.installFiles(shard.filesType, shard.installPaths, packageName);
    // Each shard records its own window; the group's transfer time is their union.
    timings.recordPush(shard.filesType, start, System.currentTimeMillis());
  }

  /**
   * Waits for every push, reporting the first failure with any others attached.
   *
   * <p>Deliberately does not cancel the rest once one fails. A thread blocked reading an {@code
   * adb} subprocess does not observe an interrupt, so cancelling would not stop the transfer -- it
   * would only stop us waiting for it, leaving shards writing to the device after the install has
   * been called failed, and skipping the scratch each one removes on its way out. The cost is that
   * a failing install takes as long as its slowest shard.
   *
   * <p>Interruption is the exception: it means the process is going away, so it stops waiting and
   * leaves whatever is in flight to die with the JVM.
   */
  private static void awaitAll(List<Future<Void>> pushes) throws Exception {
    Exception failure = null;
    for (Future<Void> push : pushes) {
      try {
        push.get();
      } catch (ExecutionException e) {
        Exception cause = e.getCause() instanceof Exception ? (Exception) e.getCause() : e;
        if (failure == null) {
          failure = cause;
        } else {
          // Every shard that failed, not just the first: they fail independently, and which one
          // arrives first says nothing about which one explains the install.
          failure.addSuppressed(cause);
        }
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        throw e;
      }
    }
    if (failure != null) {
      throw failure;
    }
  }

  /** One concurrently pushable chunk of a payload. */
  @VisibleForTesting
  static final class PushShard {
    final String filesType;
    final ImmutableMap<Path, Path> installPaths;

    PushShard(String filesType, ImmutableMap<Path, Path> installPaths) {
      this.filesType = filesType;
      this.installPaths = installPaths;
    }
  }

  /** One exopackage payload class, with its contents resolved exactly once. */
  static final class ResolvedExoPayload {
    private final String type;
    private final ImmutableMap<Path, Path> filesToInstall;
    private final ImmutableMap<Path, String> metadataToInstall;

    ResolvedExoPayload(ExoHelper helper) throws IOException {
      this.type = helper.getType();
      this.filesToInstall = helper.getFilesToInstall();
      this.metadataToInstall = helper.getMetadataToInstall();
    }
  }

  /**
   * @return true if the given apk info contains any items which need to be installed via exopackage
   */
  private boolean exopackageEnabled() {
    return isExopackage(exoInfo);
  }

  /** True when {@code exoInfo} carries any payload to be pushed alongside the apk. */
  public static boolean isExopackage(IsolatedExopackageInfo exoInfo) {
    return exoInfo.getDexInfo().isPresent()
        || exoInfo.getNativeLibsInfo().isPresent()
        || exoInfo.getResourcesInfo().isPresent();
  }

  /** What the device holds that no payload wants. The lock file belongs to no payload and stays. */
  @VisibleForTesting
  static ImmutableSortedSet<Path> filesToDelete(
      ImmutableSortedSet<Path> filesOnDevice, ImmutableList<ResolvedExoPayload> payloads) {
    ImmutableSet.Builder<Path> wanted = ImmutableSet.builder();
    for (ResolvedExoPayload payload : payloads) {
      wanted.addAll(payload.filesToInstall.keySet());
      wanted.addAll(payload.metadataToInstall.keySet());
    }
    ImmutableSet<Path> wantedFiles = wanted.build();
    return filesOnDevice.stream()
        .filter(p -> !p.getFileName().toString().equals("lock") && !wantedFiles.contains(p))
        .collect(ImmutableSortedSet.toImmutableSortedSet(Ordering.natural()));
  }

  private void deleteFiles(ImmutableSortedSet<Path> toDelete) {
    Function<Path, Path> toRootDirFn =
        toDelete.size() <= RM_GROUPING_THRESHOLD
            ? path -> dataRoot
            : path -> dataRoot.resolve(path).getParent();
    Function<Path, String> toFileFn =
        toDelete.size() <= RM_GROUPING_THRESHOLD
            ? Path::toString
            : path -> path.getFileName().toString();

    try {
      toDelete.stream()
          .collect(ImmutableListMultimap.toImmutableListMultimap(toRootDirFn, toFileFn))
          .asMap()
          .forEach((dir, files) -> device.rmFiles(dir.toString(), files));
    } catch (Exception e) {
      // Every failure, not just a tagged one: `rmFiles` also surfaces the IO of writing and
      // pushing its manifest, and those would otherwise escape without the tag below.
      throw AndroidInstallException.Companion.exopackageGarbageCollectionFailed(e.getMessage());
    }
  }

  private void installMetadata(ImmutableMap<Path, String> metadataToInstall) throws Exception {
    try (Closer closer = Closer.create()) {
      ImmutableMap.Builder<Path, Path> filesToInstall = ImmutableMap.builder();
      for (Map.Entry<Path, String> entry : metadataToInstall.entrySet()) {
        NamedTemporaryFile temp =
            Objects.requireNonNull(closer.register(new NamedTemporaryFile("metadata", "tmp")));
        com.google.common.io.Files.write(
            entry.getValue().getBytes(StandardCharsets.UTF_8), temp.get().toFile());
        filesToInstall.put(
            dataRoot.resolve(entry.getKey()), rootPath.resolve(temp.get()).getPath());
      }
      // Pushed as one shard, through the same path as every payload. It stays inside the closer:
      // the temporary files it names are deleted when that closes.
      pushShards(ImmutableList.of(new PushShard("metadata", filesToInstall.build())));
    }
  }

  /**
   * Parses a text file which is supposed to be in the following format: "file_path_without_spaces
   * file_hash ...." i.e. it parses the first two columns of each line and ignores the rest of it.
   *
   * @return A multi map from the file hash to its path, which equals the raw path resolved against
   *     {@code resolvePathAgainst}.
   */
  @VisibleForTesting
  public static ImmutableMultimap<String, Path> parseExopackageInfoMetadata(
      Path metadataTxt, Path resolvePathAgainst, AbsPath rootPath) throws IOException {
    ImmutableMultimap.Builder<String, Path> builder = ImmutableMultimap.builder();
    for (String line : ProjectFilesystemUtils.readLines(rootPath, metadataTxt)) {
      // ignore lines that start with '.'
      if (line.startsWith(".")) {
        continue;
      }
      List<String> parts = Splitter.on(' ').splitToList(line);
      if (parts.size() < 2) {
        throw new RuntimeException("Illegal line in metadata file: " + line);
      }
      builder.put(parts.get(1), resolvePathAgainst.resolve(parts.get(0)));
    }
    return builder.build();
  }

  public static ImmutableMultimap<String, Path> parseExopackageInfoMetadata(
      AbsPath metadataTxt, AbsPath resolvePathAgainst, AbsPath rootPath) throws IOException {
    return parseExopackageInfoMetadata(
        metadataTxt.getPath(), resolvePathAgainst.getPath(), rootPath);
  }
}
