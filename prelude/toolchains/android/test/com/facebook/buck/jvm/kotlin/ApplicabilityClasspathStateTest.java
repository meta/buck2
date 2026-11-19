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

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;
import static org.junit.Assume.assumeFalse;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.HashMap;
import java.util.Map;
import java.util.jar.JarEntry;
import java.util.jar.JarOutputStream;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.objectweb.asm.AnnotationVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.Opcodes;

public class ApplicabilityClasspathStateTest {

  private static final int CLASS = 1;
  private static final int FILE_FACADE = 2;

  @Rule public TemporaryPaths temporaryPaths = new TemporaryPaths();

  private AbsPath root;
  private AbsPath stateDir;
  private AbsPath reducedJar;
  private AbsPath otherJar;

  @Before
  public void setUp() throws IOException {
    root = temporaryPaths.getRoot();
    stateDir = temporaryPaths.newFolder("incremental_state");
    reducedJar = root.resolve("reduced.jar");
    otherJar = root.resolve("other.jar");
  }

  @Test
  public void firstRoundHasChangedAndOnlyRecordsPendingState() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));

    assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
    assertTrue(
        Files.exists(stateDir.resolve(ApplicabilityClasspathState.PENDING_STATE_FILE).getPath()));
    assertFalse(Files.exists(stateDir.resolve(ApplicabilityClasspathState.STATE_FILE).getPath()));
  }

  @Test
  public void firstRoundCreatesTheStateDirectory() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    AbsPath missingStateDir = root.resolve("missing_incremental_state");

    assertTrue(
        ApplicabilityClasspathState.recordAndCompare(
            root,
            missingStateDir,
            ImmutableList.of(reducedJar),
            new ActionMetadata(
                Paths.get("incremental_config.json"), new HashMap<>(), new HashMap<>())));
    assertTrue(
        Files.exists(
            missingStateDir.resolve(ApplicabilityClasspathState.PENDING_STATE_FILE).getPath()));
  }

  @Test
  public void unchangedClasspathAfterSuccessHasNotChanged() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));

    assertFalse(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void classAddedToReducedJarHasChanged() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));

    writeJar(
        reducedJar,
        ImmutableMap.of(
            "p/A.class", classBytes("p/A", CLASS), "p/B.class", classBytes("p/B", CLASS)));

    assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void jarLeavingReducedClasspathHasChanged() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    writeJar(otherJar, ImmutableMap.of("q/B.class", classBytes("q/B", CLASS)));
    succeedRound(ImmutableList.of(reducedJar, otherJar));

    assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void classMovingBetweenReducedJarsHasNotChanged() throws IOException {
    writeJar(
        reducedJar,
        ImmutableMap.of(
            "p/A.class", classBytes("p/A", CLASS), "p/B.class", classBytes("p/B", CLASS)));
    writeJar(otherJar, ImmutableMap.of("q/C.class", classBytes("q/C", CLASS)));
    succeedRound(ImmutableList.of(reducedJar, otherJar));

    // The index is the union of both jars, which the move leaves as it was.
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    writeJar(
        otherJar,
        ImmutableMap.of(
            "p/B.class", classBytes("p/B", CLASS), "q/C.class", classBytes("q/C", CLASS)));

    assertFalse(recordAndCompare(ImmutableList.of(reducedJar, otherJar)));
  }

  @Test
  public void facadeMetadataChangeHasChanged() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/FKt.class", classBytes("p/FKt", FILE_FACADE, "f")));
    succeedRound(ImmutableList.of(reducedJar));

    writeJar(
        reducedJar, ImmutableMap.of("p/FKt.class", classBytes("p/FKt", FILE_FACADE, "f", "g")));

    assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void classMemberChangeHasNotChanged() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));

    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS, "member")));

    assertFalse(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void nestedClassesAreNotIndexed() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));

    writeJar(
        reducedJar,
        ImmutableMap.of(
            "p/A.class", classBytes("p/A", CLASS), "p/A$B.class", classBytes("p/A$B", CLASS)));

    assertFalse(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void failedRoundKeepsComparingWithLastSuccessfulRound() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));

    writeJar(otherJar, ImmutableMap.of("q/B.class", classBytes("q/B", CLASS)));
    assertTrue(recordAndCompare(ImmutableList.of(reducedJar, otherJar)));
    // The round failed, so its pending state was never promoted.
    assertTrue(recordAndCompare(ImmutableList.of(reducedJar, otherJar)));
  }

  @Test
  public void jarsWithUnchangedDigestsAreNotRead() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    Map<Path, String> digests = new HashMap<>();
    digests.put(root.relativize(reducedJar).getPath(), "digest-1");
    succeedRound(ImmutableList.of(reducedJar), digests);

    Files.write(reducedJar.getPath(), "not a jar".getBytes(StandardCharsets.UTF_8));

    assertFalse(recordAndCompare(ImmutableList.of(reducedJar), digests));
  }

  @Test
  public void unreadableJarHasChanged() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));

    Files.write(reducedJar.getPath(), "not a jar".getBytes(StandardCharsets.UTF_8));

    assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  @Test
  public void metadataValuesOfDifferentTypesFingerprintDifferently() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/FKt.class", classBytes("p/FKt", FILE_FACADE, "f")));
    String integerKind = ApplicabilityClasspathState.fingerprint(ImmutableList.of(reducedJar));

    // The same metadata with the kind as a string, which the scanner does not read as a kind.
    writeJar(
        reducedJar,
        ImmutableMap.of("p/FKt.class", classBytes("p/FKt", Integer.toString(FILE_FACADE), "f")));

    assertNotEquals(
        integerKind, ApplicabilityClasspathState.fingerprint(ImmutableList.of(reducedJar)));
  }

  @Test
  public void arrayElementBoundariesAreKept() throws IOException {
    writeJar(
        reducedJar, ImmutableMap.of("p/FKt.class", classBytes("p/FKt", FILE_FACADE, "ab", "c")));
    String split = ApplicabilityClasspathState.fingerprint(ImmutableList.of(reducedJar));

    writeJar(
        reducedJar, ImmutableMap.of("p/FKt.class", classBytes("p/FKt", FILE_FACADE, "a", "bc")));

    assertNotEquals(split, ApplicabilityClasspathState.fingerprint(ImmutableList.of(reducedJar)));
  }

  @Test
  public void staleOrFailedPendingStateIsNeverLeftForPromotion() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));
    Path pending = stateDir.resolve(ApplicabilityClasspathState.PENDING_STATE_FILE).getPath();
    Path committed = stateDir.resolve(ApplicabilityClasspathState.STATE_FILE).getPath();
    Files.writeString(pending, "fingerprint stale");

    // A round whose committed state is unreadable replaces the stale pending state with its own.
    Files.setPosixFilePermissions(committed, PosixFilePermissions.fromString("---------"));
    try {
      assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
    } finally {
      Files.setPosixFilePermissions(committed, PosixFilePermissions.fromString("rw-r--r--"));
    }
    assertFalse(Files.readString(pending).contains("stale"));
  }

  @Test
  public void aFailingJarReadLeavesNoPendingState() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));
    Path pending = stateDir.resolve(ApplicabilityClasspathState.PENDING_STATE_FILE).getPath();
    Files.writeString(pending, "fingerprint stale");
    // A directory where the jar should be: computing its digest fails.
    Files.delete(reducedJar.getPath());
    Files.createDirectory(reducedJar.getPath());

    assertThrows(IOException.class, () -> recordAndCompare(ImmutableList.of(reducedJar)));

    assertFalse(Files.exists(pending));
    // No later round may compare with the state from before this one.
    assertFalse(Files.exists(stateDir.resolve(ApplicabilityClasspathState.STATE_FILE).getPath()));
  }

  @Test
  public void anUnreadableStateHasChangedAndTheRoundRecordsItsOwn() throws IOException {
    writeJar(reducedJar, ImmutableMap.of("p/A.class", classBytes("p/A", CLASS)));
    succeedRound(ImmutableList.of(reducedJar));
    Path committed = stateDir.resolve(ApplicabilityClasspathState.STATE_FILE).getPath();
    Files.setPosixFilePermissions(committed, java.util.Collections.emptySet());
    assumeFalse("Running as a user that reads any file", Files.isReadable(committed));

    assertTrue(recordAndCompare(ImmutableList.of(reducedJar)));
    ApplicabilityClasspathState.promotePending(stateDir.getPath());

    assertFalse(recordAndCompare(ImmutableList.of(reducedJar)));
  }

  private boolean recordAndCompare(ImmutableList<AbsPath> classpath) throws IOException {
    return recordAndCompare(classpath, new HashMap<>());
  }

  private boolean recordAndCompare(ImmutableList<AbsPath> classpath, Map<Path, String> digests)
      throws IOException {
    return ApplicabilityClasspathState.recordAndCompare(
        root,
        stateDir,
        classpath,
        new ActionMetadata(Paths.get("incremental_config.json"), new HashMap<>(), digests));
  }

  private void succeedRound(ImmutableList<AbsPath> classpath) throws IOException {
    succeedRound(classpath, new HashMap<>());
  }

  private void succeedRound(ImmutableList<AbsPath> classpath, Map<Path, String> digests)
      throws IOException {
    recordAndCompare(classpath, digests);
    ApplicabilityClasspathState.promotePending(stateDir.getPath());
  }

  private static void writeJar(AbsPath jar, ImmutableMap<String, byte[]> entries)
      throws IOException {
    try (OutputStream file = Files.newOutputStream(jar.getPath());
        JarOutputStream out = new JarOutputStream(file)) {
      for (Map.Entry<String, byte[]> entry : entries.entrySet()) {
        out.putNextEntry(new JarEntry(entry.getKey()));
        out.write(entry.getValue());
        out.closeEntry();
      }
    }
  }

  /** A class carrying Kotlin metadata of `kind`, whose `d2` strings name `members`. */
  private static byte[] classBytes(String internalName, Object kind, String... members) {
    ClassWriter writer = new ClassWriter(0);
    writer.visit(Opcodes.V1_8, Opcodes.ACC_PUBLIC, internalName, null, "java/lang/Object", null);
    AnnotationVisitor metadata = writer.visitAnnotation("Lkotlin/Metadata;", true);
    metadata.visit("k", kind);
    metadata.visit("mv", new int[] {2, 4, 0});
    AnnotationVisitor d1 = metadata.visitArray("d1");
    d1.visit(null, "proto:" + String.join(",", members));
    d1.visitEnd();
    AnnotationVisitor d2 = metadata.visitArray("d2");
    for (String member : members) {
      d2.visit(null, member);
    }
    d2.visitEnd();
    metadata.visitEnd();
    for (String member : members) {
      writer.visitField(Opcodes.ACC_PUBLIC, member, "I", null, null).visitEnd();
    }
    writer.visitEnd();
    return writer.toByteArray();
  }
}
