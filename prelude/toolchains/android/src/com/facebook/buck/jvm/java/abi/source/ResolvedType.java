/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Collections;
import java.util.Set;
import javax.lang.model.element.TypeElement;

/**
 * Represents a resolved type as simulated by {@link
 * com.facebook.buck.jvm.java.abi.source.CompilerTypeResolutionSimulator}.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ResolvedType {
  public final ResolvedTypeKind kind;
  public final TypeElement type;
  public final Set<String> missingDependencies;
  public final Set<String> missingMemberSupertypes;

  public ResolvedType(ResolvedTypeKind kind, TypeElement type) {
    this(kind, type, Collections.emptySet());
  }

  public ResolvedType(ResolvedTypeKind kind, TypeElement type, Set<String> missingDependencies) {
    this(kind, type, missingDependencies, Collections.emptySet());
  }

  public ResolvedType(
      ResolvedTypeKind kind,
      TypeElement type,
      Set<String> missingDependencies,
      Set<String> missingMemberSupertypes) {
    this.kind = kind;
    this.type = type;
    this.missingDependencies = missingDependencies;
    this.missingMemberSupertypes = missingMemberSupertypes;
  }
}
