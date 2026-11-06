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

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import java.util.ArrayList;
import java.util.EnumSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.Future;
import java.util.function.Function;
import java.util.logging.Level;
import java.util.logging.Logger; // NOPMD

/**
 * The payloads sent ahead of an install, and the pushes carrying them.
 *
 * <p>Guards itself. Claiming a payload and recording the push that carries it have to happen
 * together, so that pair is one method rather than a contract callers are trusted to keep, and the
 * pushes are waited on and stopped from here rather than handed out.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
final class StreamedPushes {
  private static final Logger LOG = Logger.getLogger(StreamedPushes.class.getName());

  private final Set<AndroidArtifacts.ArtifactClass> dispatched =
      EnumSet.noneOf(AndroidArtifacts.ArtifactClass.class);
  private final List<Future<?>> pushes = new ArrayList<>();
  private boolean sealed;

  /**
   * Sends whatever of {@code ready} has not gone yet, and records the push. Does nothing when
   * everything ready has already gone, or once the install has sealed them.
   */
  synchronized void dispatch(
      Set<AndroidArtifacts.ArtifactClass> ready,
      Function<Set<AndroidArtifacts.ArtifactClass>, Future<?>> push) {
    if (sealed) {
      return;
    }
    Set<AndroidArtifacts.ArtifactClass> pending =
        EnumSet.noneOf(AndroidArtifacts.ArtifactClass.class);
    pending.addAll(ready);
    pending.removeAll(dispatched);
    if (pending.isEmpty()) {
      return;
    }
    // Recorded only once the push exists: a payload marked sent with nothing carrying it would
    // never be offered again.
    Future<?> started = push.apply(pending);
    dispatched.addAll(pending);
    pushes.add(started);
  }

  /**
   * Hands the pushes to the install and waits for them, so nothing is still writing into the
   * directories it is about to. Sealing and waiting are one method because waiting without sealing
   * first lets a dispatch slip in behind the wait.
   */
  void sealAndAwait() throws InterruptedException {
    seal();
    await();
  }

  private synchronized void seal() {
    sealed = true;
  }

  /** Waits for every push to finish. A push that failed is left for the install to redo. */
  private void await() throws InterruptedException {
    List<Future<?>> streaming = started();
    for (Future<?> streamed : streaming) {
      try {
        streamed.get();
      } catch (InterruptedException e) {
        // Interrupting does not stop a push: the pool hands each task straight to a thread, which
        // then blocks on adb's monitor and inside a subprocess read, neither of which is
        // interruptible. So the install gives up rather than pretending the device is quiet.
        Thread.currentThread().interrupt();
        throw e;
      } catch (ExecutionException e) {
        LOG.log(Level.WARNING, "A streamed push failed; the install will push instead", e);
      }
    }
  }

  /**
   * The pushes so far. Copied under the monitor and used outside it: waiting on a push while
   * holding it would block the arrivals that are still trying to dispatch.
   */
  private synchronized List<Future<?>> started() {
    return List.copyOf(pushes);
  }

  @VisibleForTesting
  synchronized int pushCount() {
    return pushes.size();
  }
}
