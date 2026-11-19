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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.install.model.FileReadyRequest;
import com.facebook.buck.install.model.FileResponse;
import com.facebook.buck.install.model.InstallInfoRequest;
import com.facebook.buck.install.model.InstallResponse;
import com.google.common.util.concurrent.SettableFuture;
import io.grpc.stub.StreamObserver;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.stream.Collectors;
import org.junit.Test;

public class InstallerServiceTest {

  private static final String INSTALL_ID = "//some:target";

  /** Ample: these tests finish well inside it, so the bound never decides the outcome. */
  private static final long AMPLE_TIMEOUT_SECONDS = 30L;

  /** The service bounds steps in whole seconds, so this is as briefly as a timeout can be shown. */
  private static final long SHORT_TIMEOUT_SECONDS = 1L;

  /** Outlasts {@link #SHORT_TIMEOUT_SECONDS}, so the bound ends the step rather than the sleep. */
  private static final long LONGER_THAN_SHORT_TIMEOUT_MILLIS = 5_000L;

  /** An installer that succeeds at everything, for subclasses to override a piece of. */
  private static class FakeInstaller implements InstallCommand {
    final AtomicInteger installs = new AtomicInteger();
    final AtomicInteger handedOver = new AtomicInteger();

    @Override
    public String name() {
      return "fake";
    }

    @Override
    public InstallResult fileReady(String artifact, Path artifactPath, InstallId installId) {
      handedOver.incrementAndGet();
      return InstallResult.success();
    }

    @Override
    public InstallResult allFilesReady(InstallId installId) {
      installs.incrementAndGet();
      return InstallResult.success();
    }
  }

  private static final class CapturingObserver<T> implements StreamObserver<T> {
    T value;
    Throwable error;

    @Override
    public void onNext(T next) {
      value = next;
    }

    @Override
    public void onError(Throwable thrown) {
      error = thrown;
    }

    @Override
    public void onCompleted() {}
  }

  private static InstallerService serviceFor(
      InstallCommand installer, long timeoutSeconds, String... artifacts) {
    InstallerService service =
        new InstallerService(installer, SettableFuture.create(), timeoutSeconds);
    CapturingObserver<InstallResponse> observer = new CapturingObserver<>();
    service.install(
        InstallInfoRequest.newBuilder()
            .setInstallId(INSTALL_ID)
            .addAllFileNames(List.of(artifacts))
            .build(),
        observer);
    return service;
  }

  private static FileResponse deliver(InstallerService service, String artifact) {
    CapturingObserver<FileResponse> observer = new CapturingObserver<>();
    service.fileReady(
        FileReadyRequest.newBuilder()
            .setInstallId(INSTALL_ID)
            .setName(artifact)
            .setPath("/tmp/" + artifact)
            .build(),
        observer);
    // Failures belong in the response, not as an RPC-level error.
    assertNull(observer.error);
    return observer.value;
  }

  /**
   * Handing an artifact over can block on the device, so it needs the same bound as the install.
   */
  @Test
  public void anArtifactThatHangsIsBoundedByTheTimeout() {
    FakeInstaller hangs =
        new FakeInstaller() {
          @Override
          public InstallResult fileReady(String artifact, Path artifactPath, InstallId installId) {
            try {
              Thread.sleep(LONGER_THAN_SHORT_TIMEOUT_MILLIS);
            } catch (InterruptedException e) {
              Thread.currentThread().interrupt();
            }
            return InstallResult.success();
          }
        };

    FileResponse response = deliver(serviceFor(hangs, SHORT_TIMEOUT_SECONDS, "apk"), "apk");

    assertTrue(response.hasErrorDetail());
    assertTrue(
        response.getErrorDetail().getMessage(),
        response
            .getErrorDetail()
            .getMessage()
            .contains("Timeout of " + SHORT_TIMEOUT_SECONDS + "s"));
    assertEquals(0, hangs.installs.get());
  }

  @Test
  public void anArtifactThatThrowsBecomesAnErrorResponse() {
    FakeInstaller throwsUp =
        new FakeInstaller() {
          @Override
          public InstallResult fileReady(String artifact, Path artifactPath, InstallId installId) {
            throw new IllegalStateException("boom");
          }
        };

    FileResponse response = deliver(serviceFor(throwsUp, AMPLE_TIMEOUT_SECONDS, "apk"), "apk");

    assertTrue(response.hasErrorDetail());
    assertTrue(
        response.getErrorDetail().getMessage(),
        response.getErrorDetail().getMessage().contains("boom"));
    assertEquals(0, throwsUp.installs.get());
  }

  /**
   * Whether the install runs must not depend on which artifact happened to arrive last, so both
   * orders are exercised. The failing-last case is the one the old dispatch got right by accident;
   * failing-first is the one it got wrong.
   */
  @Test
  public void anArtifactFailingStopsTheInstallWhicheverOrderItArrivesIn() {
    for (String[] order : new String[][] {{"bad", "good"}, {"good", "bad"}}) {
      FakeInstaller failsOnBadArtifact =
          new FakeInstaller() {
            @Override
            public InstallResult fileReady(
                String artifact, Path artifactPath, InstallId installId) {
              return artifact.equals("bad") ? InstallResult.error("nope") : InstallResult.success();
            }
          };

      InstallerService service =
          serviceFor(failsOnBadArtifact, AMPLE_TIMEOUT_SECONDS, "bad", "good");
      assertEquals(order[0].equals("bad"), deliver(service, order[0]).hasErrorDetail());
      assertEquals(order[1].equals("bad"), deliver(service, order[1]).hasErrorDetail());

      assertEquals(
          "delivered " + order[0] + " then " + order[1], 0, failsOnBadArtifact.installs.get());
    }
  }

  /** Per-install bookkeeping is dropped once the install is decided, so a repeat is ignored. */
  @Test
  public void anArtifactDeliveredAgainAfterTheInstallIsIgnored() {
    FakeInstaller installer = new FakeInstaller();

    InstallerService service = serviceFor(installer, AMPLE_TIMEOUT_SECONDS, "apk");
    deliver(service, "apk");
    assertEquals(1, installer.installs.get());

    assertFalse(deliver(service, "apk").hasErrorDetail());
    assertEquals("must not install twice", 1, installer.installs.get());
    assertEquals("must not hand the artifact over twice", 1, installer.handedOver.get());
  }

  /** Handing an artifact over happens before the install, so its metadata must not be dropped. */
  @Test
  public void metadataFromBothStepsReachesTheResponse() {
    FakeInstaller reportsFromBothSteps =
        new FakeInstaller() {
          @Override
          public InstallResult fileReady(String artifact, Path artifactPath, InstallId installId) {
            return new InstallResult(List.of(Map.of("from", "fileReady")), Optional.empty());
          }

          @Override
          public InstallResult allFilesReady(InstallId installId) {
            installs.incrementAndGet();
            return new InstallResult(List.of(Map.of("from", "allFilesReady")), Optional.empty());
          }
        };

    FileResponse response =
        deliver(serviceFor(reportsFromBothSteps, AMPLE_TIMEOUT_SECONDS, "apk"), "apk");

    assertEquals(
        List.of("fileReady", "allFilesReady"),
        response.getDeviceMetadataList().stream()
            .flatMap(metadata -> metadata.getEntryList().stream())
            .map(entry -> entry.getValue())
            .collect(Collectors.toList()));
  }

  @Test
  public void theInstallRunsOnceEveryArtifactHasArrived() {
    FakeInstaller installer = new FakeInstaller();

    InstallerService service = serviceFor(installer, AMPLE_TIMEOUT_SECONDS, "apk", "manifest");
    deliver(service, "apk");
    assertEquals("must wait for the rest", 0, installer.installs.get());
    deliver(service, "manifest");

    assertEquals(1, installer.installs.get());
  }
}
