/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer;

import com.facebook.buck.install.model.DeviceMetadata;
import com.facebook.buck.install.model.FileReadyRequest;
import com.facebook.buck.install.model.FileResponse;
import com.facebook.buck.install.model.InstallInfoRequest;
import com.facebook.buck.install.model.InstallResponse;
import com.facebook.buck.install.model.InstallerGrpc;
import com.facebook.buck.install.model.ShutdownRequest;
import com.facebook.buck.install.model.ShutdownResponse;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Throwables;
import com.google.common.util.concurrent.ListenableFuture;
import com.google.common.util.concurrent.ListeningExecutorService;
import com.google.common.util.concurrent.MoreExecutors;
import com.google.common.util.concurrent.SettableFuture;
import com.google.common.util.concurrent.ThreadFactoryBuilder;
import io.grpc.stub.StreamObserver;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.SynchronousQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.function.Function;
import java.util.logging.Level;
import java.util.logging.Logger; // NOPMD
import java.util.stream.Collectors;

/**
 * Installer Service that implements {@code install.proto}
 *
 * <p>The workflow:
 *
 * <ol>
 *   <li>client (buck2) sends `install` request with install id and file map. It could be multiple
 *       targets that would have different install id associated with them.
 *   <li>client sends multiple `fileReady` requests (file data + install id)
 *   <li>{@link InstallerService} tracks all received `fileReady` requests associated with the same
 *       install id. When all files received then {@link InstallerService} calls {@link
 *       InstallCommand} to actually install all received files for the specific install id into the
 *       device/emulator.
 *   <li>client sends `shutdownServer` when wants to stop interactions and asks {@link
 *       InstallerService} to terminate.
 * </ol>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstallerService extends InstallerGrpc.InstallerImplBase {

  private static final ThreadPoolExecutor THREAD_POOL =
      new ThreadPoolExecutor(
          0,
          Integer.MAX_VALUE,
          1,
          TimeUnit.SECONDS,
          new SynchronousQueue<>(),
          new ThreadFactoryBuilder().setNameFormat("Installer").build());

  private static final ListeningExecutorService LISTENING_EXECUTOR_SERVICE =
      MoreExecutors.listeningDecorator(THREAD_POOL);

  private static final Logger LOG = Logger.getLogger(InstallerService.class.getName());
  private final InstallCommand installer;
  private final SettableFuture<Void> installFinished;
  private final Map<InstallId, Map<String, Optional<Path>>> installIdToFilesMap = new HashMap<>();
  // Installs where at least one artifact failed. Guarded by installIdToFilesMap.
  private final Set<InstallId> failedInstalls = new HashSet<>();
  private final long installTimeoutSeconds;

  public InstallerService(
      InstallCommand installer, SettableFuture<Void> installFinished, long installTimeoutSeconds) {
    this.installer = installer;
    this.installFinished = installFinished;
    this.installTimeoutSeconds = installTimeoutSeconds;
    LOG.info("Install timeout configured to " + this.installTimeoutSeconds + "s");
  }

  @Override
  public void install(
      InstallInfoRequest request, StreamObserver<InstallResponse> responseObserver) {
    try {
      InstallResponse response = handleInstallRequest(request);
      responseObserver.onNext(response);
      responseObserver.onCompleted();
    } catch (Exception e) {
      handleException(responseObserver, e);
    }
  }

  private InstallResponse handleInstallRequest(InstallInfoRequest request) {
    InstallId installId = InstallId.of(request.getInstallId());
    Set<String> fileNames = new HashSet<>(request.getFileNamesList());
    LOG.info(
        String.format(
            "Received install id: %s with %d file names", installId.getValue(), fileNames.size()));
    // Before the install is registered: a file is only accepted for an install that is in the map,
    // so declaring first leaves no window where an artifact can arrive for an install that has not
    // been told what to expect.
    installer.onInstallStarted(installId, fileNames);
    synchronized (installIdToFilesMap) {
      installIdToFilesMap.put(
          installId,
          fileNames.stream()
              .collect(Collectors.toMap(Function.identity(), ignore -> Optional.empty())));
    }
    return InstallResponse.newBuilder()
        .setInstallId(installId.getValue())
        .setInstallerName(installer.name())
        .build();
  }

  @Override
  public void fileReady(FileReadyRequest request, StreamObserver<FileResponse> responseObserver) {
    try {
      FileResponse fileResponse = handleFileReadyRequest(request);
      responseObserver.onNext(fileResponse);
      responseObserver.onCompleted();
    } catch (Exception e) {
      handleException(responseObserver, e);
    }
  }

  private FileResponse handleFileReadyRequest(FileReadyRequest request)
      throws InterruptedException {
    InstallId installId = InstallId.of(request.getInstallId());
    String name = request.getName();
    String path = request.getPath();
    LOG.info(
        String.format(
            "Received artifact %s located at %s for install id: %s",
            name, path, installId.getValue()));

    FileResponse.Builder fileResponseBuilder =
        FileResponse.newBuilder().setName(name).setPath(path).setInstallId(installId.getValue());

    synchronized (installIdToFilesMap) {
      // Absent once the install has been decided, so a repeat or unknown delivery is ignored
      // rather than handed over again -- doing the work would cost a timeout and be discarded.
      if (!installIdToFilesMap.containsKey(installId)) {
        LOG.info(String.format("Ignoring artifact %s for a finished install", name));
        return fileResponseBuilder.build();
      }
    }

    InstallResult result = runBounded(() -> installer.fileReady(name, Paths.get(path), installId));
    // Reported by whichever step produced it. Handing the artifact over runs before the install
    // does, so keeping only the last result would drop anything the first one had to say.
    List<Map<String, String>> deviceMetadata = new ArrayList<>(result.getDeviceMetadata());

    boolean readyToInstall = false;
    synchronized (installIdToFilesMap) {
      Map<String, Optional<Path>> filesMap = installIdToFilesMap.get(installId);
      if (filesMap != null) {
        filesMap.put(name, Optional.of(Paths.get(path)));
        if (result.isError()) {
          failedInstalls.add(installId);
        }
        // Install once every artifact is in and none of them failed.
        if (filesMap.values().stream().allMatch(Optional::isPresent)) {
          readyToInstall = !failedInstalls.contains(installId);
          // Nothing looks these up again, and one server handles many installs.
          installIdToFilesMap.remove(installId);
          failedInstalls.remove(installId);
        }
      }
    }

    if (readyToInstall) {
      LOG.info(String.format("Starting install for install id: %s", installId.getValue()));
      result = runBounded(() -> installer.allFilesReady(installId));
      LOG.info("Install [" + installId.getValue() + "] finished with result: " + result);
      deviceMetadata.addAll(result.getDeviceMetadata());
    }

    if (result.isError()) {
      fileResponseBuilder.setErrorDetail(result.getInstallError().toProtoModel());
    }
    for (Map<String, String> metadata : deviceMetadata) {
      DeviceMetadata.Builder metadataBuilder = DeviceMetadata.newBuilder();
      for (Map.Entry<String, String> entry : metadata.entrySet()) {
        metadataBuilder.addEntry(
            DeviceMetadata.Entry.newBuilder().setKey(entry.getKey()).setValue(entry.getValue()));
      }
      fileResponseBuilder.addDeviceMetadata(metadataBuilder);
    }

    return fileResponseBuilder.build();
  }

  /**
   * Runs one step of an install on a worker thread, bounded by the configured timeout.
   *
   * <p>Handing an artifact over can block -- an implementation may shell out to the device -- so
   * the bound covers per-artifact work as well as the install. Anything thrown is turned into a
   * tagged error, rather than escaping as a bare RPC failure that loses the tag.
   *
   * <p>The bound is on the answer, not on the work. Timing out interrupts the step, but a thread
   * blocked reading an {@code adb} subprocess does not observe an interrupt, so it may run on and
   * finish into a result nobody reads. What that guarantees is that the client always gets a
   * response; reclaiming the thread would need the blocking calls themselves to be bounded. In
   * practice the installer serves one install and exits, so a stranded thread dies with it.
   */
  private InstallResult runBounded(Callable<InstallResult> step) throws InterruptedException {
    ListenableFuture<InstallResult> running = LISTENING_EXECUTOR_SERVICE.submit(step);
    try {
      return running.get(installTimeoutSeconds, TimeUnit.SECONDS);
    } catch (TimeoutException e) {
      running.cancel(true);
      return failed(
          "Timeout of " + installTimeoutSeconds + "s has been exceeded. Install failed.",
          InfraTimeoutErrorTag.INSTANCE);
    } catch (ExecutionException e) {
      return failed(
          Throwables.getStackTraceAsString(Objects.requireNonNull(e.getCause())),
          DefaultInstallErrorTag.INSTANCE);
    }
  }

  private static InstallResult failed(String message, InstallErrorTag tag) {
    return new InstallResult(List.of(), Optional.of(new InstallError(message, tag)));
  }

  @Override
  public void shutdownServer(
      ShutdownRequest request, StreamObserver<ShutdownResponse> responseObserver) {
    try {
      handleShutdownServerRequest(responseObserver);
    } catch (Exception e) {
      handleException(responseObserver, e);
    }
  }

  private void handleShutdownServerRequest(StreamObserver<ShutdownResponse> responseObserver) {
    LOG.info("Received shutting down request");
    responseObserver.onNext(ShutdownResponse.getDefaultInstance());
    responseObserver.onCompleted();
    installFinished.set(null);
  }

  private void handleException(StreamObserver<?> responseObserver, Exception e) {
    LOG.log(Level.SEVERE, "Unexpected exception", e);
    responseObserver.onError(
        io.grpc.Status.INTERNAL
            .withDescription("Unexpected exception: " + Throwables.getStackTraceAsString(e))
            .asException());
  }
}
