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

import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.collect.ImmutableList;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
import org.junit.Rule;
import org.junit.Test;

public class IncrementalStateCopyTest {
  @Rule public TemporaryPaths temporaryPaths = new TemporaryPaths();

  @Test
  public void restoringPutsBackTheCopiedStateExactly() throws IOException {
    Path state = temporaryPaths.newFolder("incremental_state").getPath();
    Path workingDir = state.resolve("kotlinc_working_dir");
    Path classes = state.resolve("__kotlin_classes__");
    Path cache = workingDir.resolve("caches-jvm/inputs/source-snapshot.tab");
    Path classFile = classes.resolve("p/A.class");
    Files.createDirectories(cache.getParent());
    Files.createDirectories(classFile.getParent());
    Files.writeString(cache, "cache");
    Files.writeString(classFile, "class");
    Files.setLastModifiedTime(classFile, FileTime.fromMillis(1_000));

    IncrementalStateCopy copy =
        IncrementalStateCopy.copy(state.resolve("copy"), ImmutableList.of(workingDir, classes));
    // A full compile cleans both directories and writes something else.
    Files.delete(cache);
    Files.delete(classFile);
    Files.writeString(classes.resolve("Partial.class"), "partial");

    copy.restore();

    assertEquals("cache", Files.readString(cache));
    assertEquals("class", Files.readString(classFile));
    assertEquals(1_000, Files.getLastModifiedTime(classFile).toMillis());
    assertFalse(Files.exists(classes.resolve("Partial.class")));
    assertFalse(Files.exists(state.resolve("copy")));
  }

  @Test
  public void discardingKeepsTheCurrentState() throws IOException {
    Path state = temporaryPaths.newFolder("incremental_state").getPath();
    Path classes = state.resolve("__kotlin_classes__");
    Files.createDirectories(classes);
    Files.writeString(classes.resolve("A.class"), "old");

    IncrementalStateCopy copy =
        IncrementalStateCopy.copy(state.resolve("copy"), ImmutableList.of(classes));
    Files.writeString(classes.resolve("A.class"), "new");
    copy.discard();

    assertEquals("new", Files.readString(classes.resolve("A.class")));
    assertFalse(Files.exists(state.resolve("copy")));
  }

  @Test
  public void aLeftoverCopyIsReplaced() throws IOException {
    Path state = temporaryPaths.newFolder("incremental_state").getPath();
    Path classes = state.resolve("__kotlin_classes__");
    Files.createDirectories(classes);
    Files.writeString(classes.resolve("A.class"), "current");
    Files.createDirectories(state.resolve("copy/0"));
    Files.writeString(state.resolve("copy/0/Stale.class"), "stale");

    IncrementalStateCopy copy =
        IncrementalStateCopy.copy(state.resolve("copy"), ImmutableList.of(classes));
    copy.restore();

    assertTrue(Files.exists(classes.resolve("A.class")));
    assertFalse(Files.exists(classes.resolve("Stale.class")));
  }
}
