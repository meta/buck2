/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi;

import com.facebook.buck.jvm.java.abi.kotlin.KotlinMetadataReader;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Path;
import java.util.Collections;
import java.util.List;
import java.util.Set;
import org.jspecify.annotations.Nullable;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.AnnotationNode;
import org.objectweb.asm.tree.ClassNode;

/** Write an entry to stub jar by directly copy an existing abi. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class StubJarExistingEntry extends StubJarEntry {
  private final LibraryReader existingAbiInput;
  private final Path path;
  @Nullable private Set<String> referencedClassNames;
  @Nullable private List<String> sealedSubclasses;

  public static StubJarExistingEntry of(LibraryReader existingAbiInput, Path path) {
    return new StubJarExistingEntry(existingAbiInput, path);
  }

  private StubJarExistingEntry(LibraryReader existingAbiInput, Path path) {
    this.existingAbiInput = existingAbiInput;
    this.path = path;
  }

  @Override
  public void write(StubJarWriter writer) {
    writer.writeEntry(path, () -> existingAbiInput.openInputStream(path));
  }

  @Override
  public List<String> getInlineFunctions() {
    return Collections.emptyList();
  }

  @Override
  public boolean extendsInlineFunctionScope() {
    return false;
  }

  @Override
  public Set<String> getReferencedClassNames() {
    if (referencedClassNames == null) {
      ClassReferenceTracker tracker = new ClassReferenceTracker();
      try {
        existingAbiInput.visitClass(path, tracker, /* skipCode */ true);
      } catch (IOException e) {
        throw new UncheckedIOException(e);
      }
      referencedClassNames = tracker.getReferencedClassNames();
    }
    return referencedClassNames;
  }

  @Override
  public List<String> getSealedSubclasses() {
    if (sealedSubclasses == null) {
      ClassNode node = new ClassNode(Opcodes.ASM9);
      try {
        existingAbiInput.visitClass(path, node, /* skipCode */ true);
      } catch (IOException e) {
        throw new UncheckedIOException(e);
      }
      AnnotationNode metadata = StubJarClassEntry.findKotlinMetadataAnnotation(node);
      sealedSubclasses =
          metadata == null
              ? Collections.emptyList()
              : KotlinMetadataReader.getSealedSubclasses(
                  KotlinMetadataReader.readMetadata(metadata));
    }
    return sealedSubclasses;
  }
}
