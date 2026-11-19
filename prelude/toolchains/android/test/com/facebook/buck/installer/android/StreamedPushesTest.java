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

import static com.facebook.buck.installer.android.AndroidArtifacts.ArtifactClass.NATIVE_LIBRARY;
import static com.facebook.buck.installer.android.AndroidArtifacts.ArtifactClass.RESOURCES;
import static com.facebook.buck.installer.android.AndroidArtifacts.ArtifactClass.SECONDARY_DEX;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;

import java.util.EnumSet;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.Test;

/** What stops a payload being streamed twice, or streamed after the install has started. */
public class StreamedPushesTest {

  @Test
  public void everyPushThatStartedIsRecorded() {
    StreamedPushes pushes = new StreamedPushes();
    pushes.dispatch(EnumSet.of(RESOURCES), pending -> CompletableFuture.completedFuture(null));
    pushes.dispatch(EnumSet.of(NATIVE_LIBRARY), pending -> CompletableFuture.completedFuture(null));

    assertEquals(2, pushes.pushCount());
  }

  /** Nothing is sent when everything ready has already gone. */
  @Test
  public void aPayloadAlreadySentIsNotDispatchedAgain() {
    StreamedPushes pushes = new StreamedPushes();
    pushes.dispatch(EnumSet.of(RESOURCES), pending -> CompletableFuture.completedFuture(null));
    pushes.dispatch(EnumSet.of(RESOURCES), pending -> CompletableFuture.completedFuture(null));

    assertEquals(1, pushes.pushCount());
  }

  /**
   * Once the install has taken the pushes over it writes the same directories, so a payload that
   * only now becomes ready is left to the install rather than started alongside it.
   */
  @Test
  public void nothingIsDispatchedOnceSealed() throws Exception {
    StreamedPushes pushes = new StreamedPushes();
    pushes.sealAndAwait();

    AtomicBoolean pushed = new AtomicBoolean();
    pushes.dispatch(
        EnumSet.of(NATIVE_LIBRARY),
        pending -> {
          pushed.set(true);
          return CompletableFuture.completedFuture(null);
        });

    assertFalse(pushed.get());
    assertEquals(0, pushes.pushCount());
  }

  /** Only what has not gone yet: an overlapping offer sends the difference, not the whole set. */
  @Test
  public void anOverlappingOfferSendsOnlyWhatIsNew() {
    StreamedPushes pushes = new StreamedPushes();
    pushes.dispatch(
        EnumSet.of(RESOURCES, SECONDARY_DEX), pending -> CompletableFuture.completedFuture(null));

    AtomicReference<Set<AndroidArtifacts.ArtifactClass>> second = new AtomicReference<>();
    pushes.dispatch(
        EnumSet.of(SECONDARY_DEX, NATIVE_LIBRARY),
        pending -> {
          second.set(pending);
          return CompletableFuture.completedFuture(null);
        });

    assertEquals(EnumSet.of(NATIVE_LIBRARY), second.get());
  }

  /**
   * A failed push is the install's problem to redo, not a reason to fail: whatever did not land is
   * pushed again once the install lists the device.
   */
  @Test
  public void aFailedPushDoesNotFailTheInstall() throws Exception {
    StreamedPushes pushes = new StreamedPushes();
    pushes.dispatch(
        EnumSet.of(RESOURCES),
        pending -> CompletableFuture.failedFuture(new IllegalStateException("adb died")));

    pushes.sealAndAwait();
  }

  /** Waiting on pushes that all worked is uneventful. */
  @Test
  public void awaitingSuccessfulPushesReturns() throws Exception {
    StreamedPushes pushes = new StreamedPushes();
    pushes.dispatch(EnumSet.of(RESOURCES), pending -> CompletableFuture.completedFuture(null));

    pushes.sealAndAwait();

    assertEquals(1, pushes.pushCount());
  }
}
