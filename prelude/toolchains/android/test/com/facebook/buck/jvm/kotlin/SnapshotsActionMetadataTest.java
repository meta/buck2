/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.jvm.java.ActionMetadata;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.Map;
import org.junit.Test;

public class SnapshotsActionMetadataTest {

  @Test
  public void when_constructorCalled_then_filtersOnlyJarFiles() {
    Path lib1Bin = Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin");
    Path lib2Class = Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.class");
    Path lib3Txt = Paths.get("gen/pkg/__lib3__/__action__/cccccccccccccccc/lib3_snapshot.txt");
    Path lib4Bin = Paths.get("gen/pkg/__lib4__/__action__/dddddddddddddddd/lib4_snapshot.bin");
    Path lib5Bin = Paths.get("gen/pkg/__lib5__/__action__/eeeeeeeeeeeeeeee/lib5_snapshot.bin");

    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(lib1Bin, "digest1");
    previousDigest.put(lib2Class, "digest2");
    previousDigest.put(lib3Txt, "digest3");
    previousDigest.put(lib4Bin, "digest4");
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(lib1Bin, "digest1");
    currentDigest.put(lib2Class, "digest2");
    currentDigest.put(lib3Txt, "digest3");
    currentDigest.put(lib5Bin, "digest5");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertEquals(2, snapshotsActionMetadata.getPreviousSnapshotsDigest().size());
    assertEquals(2, snapshotsActionMetadata.getCurrentSnapshotsDigest().size());
    assertTrue(snapshotsActionMetadata.getPreviousSnapshotsDigest().containsKey(lib1Bin));
    assertTrue(snapshotsActionMetadata.getPreviousSnapshotsDigest().containsKey(lib4Bin));
    assertFalse(snapshotsActionMetadata.getPreviousSnapshotsDigest().containsKey(lib2Class));
    assertFalse(snapshotsActionMetadata.getPreviousSnapshotsDigest().containsKey(lib3Txt));
    assertTrue(snapshotsActionMetadata.getCurrentSnapshotsDigest().containsKey(lib1Bin));
    assertTrue(snapshotsActionMetadata.getCurrentSnapshotsDigest().containsKey(lib5Bin));
    assertFalse(snapshotsActionMetadata.getCurrentSnapshotsDigest().containsKey(lib2Class));
    assertFalse(snapshotsActionMetadata.getCurrentSnapshotsDigest().containsKey(lib3Txt));
  }

  @Test
  public void when_noChanges_then_hasClasspathChangedReturnsFalse() {
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");
    previousDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    currentDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertFalse(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void when_digestChanged_then_hasClasspathChangedReturnsTrue() {
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");
    previousDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    currentDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"),
        "digest2_changed");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertTrue(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void when_newJarAdded_then_hasClasspathChangedReturnsTrue() {
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");

    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    currentDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertTrue(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void when_jarRemoved_then_hasClasspathChangedReturnsTrue() {
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    previousDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");

    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertTrue(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void when_emptyDigests_then_hasClasspathChangedReturnsFalse() {
    Map<Path, String> previousDigest = new HashMap<>();
    Map<Path, String> currentDigest = new HashMap<>();

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertFalse(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void when_multipleSnapshotsChanged_then_hasClasspathChangedReturnsTrue() {
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");
    previousDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    previousDigest.put(
        Paths.get("gen/pkg/__lib3__/__action__/cccccccccccccccc/lib3_snapshot.bin"), "digest3");
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"),
        "digest1_changed");
    currentDigest.put(
        Paths.get("gen/pkg/__lib4__/__action__/dddddddddddddddd/lib4_snapshot.bin"), "digest4");
    currentDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertTrue(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void
      when_previousDigestEmptyAndCurrentDigestHasSnapshots_then_hasClasspathChangedReturnsTrue() {
    Map<Path, String> previousDigest = new HashMap<>();
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    currentDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertTrue(snapshotsActionMetadata.hasClasspathChanged());
  }

  @Test
  public void
      when_currentDigestEmptyAndPreviousDigestHasSnapshots_then_hasClasspathChangedReturnsTrue() {
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("gen/pkg/__lib1__/__action__/aaaaaaaaaaaaaaaa/lib1_snapshot.bin"), "digest1");
    previousDigest.put(
        Paths.get("gen/pkg/__lib2__/__action__/bbbbbbbbbbbbbbbb/lib2_snapshot.bin"), "digest2");
    Map<Path, String> currentDigest = new HashMap<>();

    ActionMetadata actionMetadata =
        new ActionMetadata(Paths.get("metadata.json"), previousDigest, currentDigest);
    SnapshotsActionMetadata snapshotsActionMetadata = new SnapshotsActionMetadata(actionMetadata);

    assertTrue(snapshotsActionMetadata.hasClasspathChanged());
  }
}
