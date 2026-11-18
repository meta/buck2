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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.collect.ImmutableList;
import com.google.common.hash.Hasher;
import com.google.common.hash.Hashing;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import org.jspecify.annotations.Nullable;
import org.objectweb.asm.AnnotationVisitor;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Tracks, across incremental rounds, the inputs of the index that the Kosabi applicability plugin
 * builds from the reduced (source-only ABI) classpath.
 *
 * <p>The incremental compiler cannot see that classpath: a jar entering or leaving it, or a class
 * moving across its boundary, changes no source and no compile-classpath snapshot, yet can change a
 * verdict. Each round fingerprints what {@code SourceOnlyAbiClasspathScanner} reads, which is the
 * top-level class entry names and the Kotlin metadata of the classes that can declare top-level
 * members, and is forced to be a full round when the fingerprint differs from the last successful
 * round's. The state is recorded as pending and promoted only once the action succeeds, so it
 * always describes a round whose verdict was READY.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ApplicabilityClasspathState {
  @VisibleForTesting static final String STATE_FILE = "applicability_classpath.txt";
  @VisibleForTesting static final String PENDING_STATE_FILE = "applicability_classpath.pending.txt";

  private static final String KOTLIN_METADATA_DESCRIPTOR = "Lkotlin/Metadata;";
  // Class, synthetic class and multi-file class facade: the scanner indexes no members of these.
  private static final Set<Object> NO_TOP_LEVEL_MEMBER_KINDS = Set.of(1, 3, 4);
  private static final String FINGERPRINT_PREFIX = "fingerprint ";
  private static final String JAR_PREFIX = "jar ";

  private ApplicabilityClasspathState() {}

  /**
   * Records the current reduced classpath as pending state and returns whether the inputs of its
   * index differ from the last successful round's. When every jar has the digest it had then, no
   * jar is read. Without a committed state, or with one that cannot be read, the inputs count as
   * changed and the round still records its own. When the round's state cannot be made, the
   * committed state is deleted too, so that no later round compares with a state older than its
   * outputs.
   */
  public static boolean recordAndCompare(
      AbsPath rootProjectDir,
      AbsPath incrementalStateDir,
      ImmutableList<AbsPath> classpath,
      ActionMetadata actionMetadata)
      throws IOException {
    Path pending = incrementalStateDir.resolve(PENDING_STATE_FILE).getPath();
    Path committed = incrementalStateDir.resolve(STATE_FILE).getPath();
    try {
      // First, so that no read below can fail while an earlier round's pending state is still
      // there to be promoted for this one.
      Files.deleteIfExists(pending);
      List<String> jars = new ArrayList<>(classpath.size());
      for (AbsPath jar : classpath) {
        Path relativePath = rootProjectDir.relativize(jar).getPath();
        jars.add(
            digest(jar, actionMetadata.getCurrentDigest().get(relativePath)) + " " + relativePath);
      }
      @Nullable State previous;
      try {
        previous = read(committed);
      } catch (IOException e) {
        previous = null;
      }
      String fingerprint =
          previous != null && previous.jars.equals(jars)
              ? previous.fingerprint
              : fingerprint(classpath);
      write(pending, new State(fingerprint, jars));
      return previous == null || !previous.fingerprint.equals(fingerprint);
    } catch (IOException | RuntimeException e) {
      Files.deleteIfExists(pending);
      Files.deleteIfExists(committed);
      throw e;
    }
  }

  /**
   * Makes the pending state the last successful round's: call only once the action succeeded, and
   * only for an action that recorded applicability state in this round.
   */
  public static void promotePending(Path incrementalStateDir) throws IOException {
    Path pending = incrementalStateDir.resolve(PENDING_STATE_FILE);
    if (Files.exists(pending)) {
      Files.move(
          pending,
          incrementalStateDir.resolve(STATE_FILE),
          StandardCopyOption.REPLACE_EXISTING,
          StandardCopyOption.ATOMIC_MOVE);
    }
  }

  /**
   * A digest of everything the scanner reads from {@code classpath}. It covers more than the index
   * itself (the whole metadata of a facade, not only the names it declares), so an unchanged
   * fingerprint implies an unchanged index.
   */
  @VisibleForTesting
  static String fingerprint(List<AbsPath> classpath) {
    TreeSet<String> inputs = new TreeSet<>();
    for (int index = 0; index < classpath.size(); index++) {
      try (JarFile jar = new JarFile(classpath.get(index).toFile(), false)) {
        for (JarEntry entry : Collections.list(jar.entries())) {
          String name = entry.getName();
          if (!name.endsWith(".class") || name.contains("$")) {
            continue;
          }
          inputs.add("class " + name);
          try (InputStream input = jar.getInputStream(entry)) {
            @Nullable String metadata = topLevelMemberMetadata(input.readAllBytes());
            if (metadata != null) {
              inputs.add("metadata " + name + " " + metadata);
            }
          } catch (IOException | RuntimeException e) {
            inputs.add("unreadable-class " + index);
          }
        }
      } catch (IOException e) {
        inputs.add("unreadable-entry " + index);
      }
    }
    Hasher hasher = Hashing.sha256().newHasher();
    for (String input : inputs) {
      hasher.putString(input, StandardCharsets.UTF_8).putByte((byte) '\n');
    }
    return hasher.hash().toString();
  }

  private static String digest(AbsPath jar, @Nullable String actionMetadataDigest)
      throws IOException {
    if (actionMetadataDigest != null) {
      return actionMetadataDigest;
    }
    // Symlinked inputs carry no digest in the action metadata.
    return Hashing.sha256().hashBytes(Files.readAllBytes(jar.getPath())).toString();
  }

  /**
   * The Kotlin metadata of a class whose metadata kind lets it declare top-level members, or null
   * for a class without Kotlin metadata or of a kind the scanner ignores (class, synthetic class,
   * multi-file class facade).
   */
  private static @Nullable String topLevelMemberMetadata(byte[] classBytes) {
    MetadataCollector collector = new MetadataCollector();
    new ClassReader(classBytes)
        .accept(
            collector, ClassReader.SKIP_CODE | ClassReader.SKIP_DEBUG | ClassReader.SKIP_FRAMES);
    if (!collector.present) {
      return null;
    }
    @Nullable Object kind = collector.kind;
    if (kind != null && NO_TOP_LEVEL_MEMBER_KINDS.contains(kind)) {
      return null;
    }
    StringBuilder encoded = new StringBuilder();
    collector.fields.forEach(
        (name, value) -> appendDelimited(appendDelimited(encoded, name), value));
    return encoded.toString();
  }

  /**
   * Appends {@code value} as its length, a colon and itself, which keeps boundaries unambiguous.
   */
  private static StringBuilder appendDelimited(StringBuilder builder, String value) {
    return builder.append(value.length()).append(':').append(value);
  }

  /**
   * A metadata value with its type, so that values of different types never encode alike: the
   * scanner reads a field only when it has the expected type.
   */
  private static String typed(Object value) {
    if (value instanceof int[]) {
      StringBuilder encoded = new StringBuilder("int[]").append(((int[]) value).length);
      for (int element : (int[]) value) {
        appendDelimited(encoded.append(','), Integer.toString(element));
      }
      return encoded.toString();
    }
    return appendDelimited(
            new StringBuilder(value.getClass().getSimpleName()), String.valueOf(value))
        .toString();
  }

  private static final class MetadataCollector extends ClassVisitor {
    private boolean present;
    private @Nullable Object kind;
    private final Map<String, String> fields = new TreeMap<>();

    MetadataCollector() {
      super(Opcodes.ASM9);
    }

    @Override
    public @Nullable AnnotationVisitor visitAnnotation(String descriptor, boolean visible) {
      if (!KOTLIN_METADATA_DESCRIPTOR.equals(descriptor)) {
        return null;
      }
      present = true;
      return new AnnotationVisitor(Opcodes.ASM9) {
        @Override
        public void visit(@Nullable String name, Object value) {
          if ("k".equals(name)) {
            kind = value;
          }
          fields.put(String.valueOf(name), typed(value));
        }

        @Override
        public AnnotationVisitor visitArray(@Nullable String name) {
          List<String> values = new ArrayList<>();
          return new AnnotationVisitor(Opcodes.ASM9) {
            @Override
            public void visit(@Nullable String ignored, Object value) {
              values.add(typed(value));
            }

            @Override
            public void visitEnd() {
              StringBuilder encoded = new StringBuilder("array").append(values.size());
              for (String value : values) {
                appendDelimited(encoded.append(','), value);
              }
              fields.put(String.valueOf(name), encoded.toString());
            }
          };
        }
      };
    }
  }

  private static final class State {
    private final String fingerprint;
    private final List<String> jars;

    State(String fingerprint, List<String> jars) {
      this.fingerprint = fingerprint;
      this.jars = jars;
    }
  }

  private static @Nullable State read(Path path) throws IOException {
    if (!Files.exists(path)) {
      return null;
    }
    @Nullable String fingerprint = null;
    List<String> jars = new ArrayList<>();
    for (String line : Files.readAllLines(path, StandardCharsets.UTF_8)) {
      if (line.startsWith(FINGERPRINT_PREFIX)) {
        fingerprint = line.substring(FINGERPRINT_PREFIX.length());
      } else if (line.startsWith(JAR_PREFIX)) {
        jars.add(line.substring(JAR_PREFIX.length()));
      }
    }
    return fingerprint == null ? null : new State(fingerprint, jars);
  }

  private static void write(Path path, State state) throws IOException {
    List<String> lines = new ArrayList<>(state.jars.size() + 1);
    lines.add(FINGERPRINT_PREFIX + state.fingerprint);
    for (String jar : state.jars) {
      lines.add(JAR_PREFIX + jar);
    }
    // The first round runs before the incremental state directory exists.
    Path directory = Objects.requireNonNull(path.getParent());
    Files.createDirectories(directory);
    // Complete or absent: a pending state is never promoted half-written.
    Path temporary = Files.createTempFile(directory, PENDING_STATE_FILE, ".tmp");
    try {
      Files.write(temporary, lines, StandardCharsets.UTF_8);
      Files.move(temporary, path, StandardCopyOption.ATOMIC_MOVE);
    } finally {
      Files.deleteIfExists(temporary);
    }
  }
}
