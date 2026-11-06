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

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.jvm.java.abi.kotlin.InlineFunctionScope;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.file.Path;
import java.util.Collections;
import java.util.List;
import java.util.Set;
import javax.annotation.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class StubJarEntry {
  @Nullable
  static StubJarEntry of(
      LibraryReader input,
      Path path,
      @Nullable LibraryReader existingAbiInput,
      Set<String> existingAbiPathStrings,
      AbiGenerationMode compatibilityMode,
      @Nullable InlineFunctionScope inlineFunctionScope,
      boolean keepSynthetic)
      throws IOException {
    if (isStubbableResource(input, path)) {
      return StubJarResourceEntry.of(input, path);
    } else if (input.isClass(path)) {
      if (existingAbiInput != null && existingAbiPathStrings.contains(path.toString())) {
        return StubJarExistingEntry.of(existingAbiInput, path);
      } else {
        return StubJarClassEntry.of(
            input, path, compatibilityMode, inlineFunctionScope, keepSynthetic);
      }
    }

    return null;
  }

  public abstract void write(StubJarWriter writer);

  public abstract List<String> getInlineFunctions();

  public abstract boolean extendsInlineFunctionScope();

  public boolean isFilePrivateKotlinClass() {
    return false;
  }

  @Nullable
  public String getClassName() {
    return null;
  }

  public Set<String> getReferencedClassNames() {
    return Collections.emptySet();
  }

  @Nullable
  public String getScopeOuterClass() {
    return null;
  }

  private static boolean isStubbableResource(LibraryReader input, Path path) {
    return input.isResource(path);
  }
}
