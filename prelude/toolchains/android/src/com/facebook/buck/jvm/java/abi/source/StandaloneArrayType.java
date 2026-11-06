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
import java.util.List;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.type.ArrayType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;

/**
 * An implementation of {@link ArrayType} that is not dependent on any particular compiler
 * implementation. It requires a {@link TypeMirror}, but does not depend on any particular
 * implementation of it (beyond the spec).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class StandaloneArrayType extends StandaloneTypeMirror implements ArrayType {

  private final TypeMirror componentType;

  public StandaloneArrayType(TypeMirror componentType) {
    this(componentType, Collections.emptyList());
  }

  public StandaloneArrayType(
      TypeMirror componentType, List<? extends AnnotationMirror> annotations) {
    super(TypeKind.ARRAY, annotations);
    this.componentType = componentType;
  }

  @Override
  public TypeMirror getComponentType() {
    return componentType;
  }

  @Override
  public String toString() {
    return String.format("%s[]", componentType.toString());
  }

  @Override
  public StandaloneArrayType cloneWithAnnotationMirrors(
      List<? extends AnnotationMirror> newAnnotations) {
    return new StandaloneArrayType(this.componentType, newAnnotations);
  }
}
