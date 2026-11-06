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
import java.util.List;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.type.IntersectionType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;

/**
 * An implementation of {@link IntersectionType} that is not dependent on any particular compiler
 * implementation. It requires a {@link TypeMirror}, but does not depend on any particular
 * implementation of it (beyond the spec).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class StandaloneIntersectionType extends StandaloneTypeMirror implements IntersectionType {

  private final List<TypeMirror> bounds;

  public StandaloneIntersectionType(List<? extends TypeMirror> bounds) {
    super(TypeKind.INTERSECTION);

    this.bounds = List.copyOf(bounds);
  }

  @Override
  public List<? extends TypeMirror> getBounds() {
    return bounds;
  }

  @Override
  public String toString() {
    StringBuilder builder = new StringBuilder();
    builder.append("java.lang.Object");

    for (TypeMirror bound : bounds) {
      builder.append("&");
      builder.append(bound);
    }

    return builder.toString();
  }

  @Override
  public StandaloneIntersectionType cloneWithAnnotationMirrors(
      List<? extends AnnotationMirror> annotations) {
    throw new UnsupportedOperationException("Cannot add annotations to an intersection type");
  }
}
