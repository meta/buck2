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
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeParameterElement;
import javax.lang.model.type.NullType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.type.TypeVariable;
import javax.lang.model.util.Types;
import org.jspecify.annotations.Nullable;

/**
 * An implementation of {@link TypeVariable} that is not dependent on any particular compiler
 * implementation. It requires {@link javax.lang.model.element.Element} and {@link TypeMirror}
 * objects, but does not depend on any particular implementation of them (beyond the spec).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class StandaloneTypeVariable extends StandaloneTypeMirror implements TypeVariable {

  private final TypeParameterElement element;
  private final NullType lowerBound;
  @Nullable private TypeMirror upperBound;

  public StandaloneTypeVariable(Types types, TypeParameterElement element) {
    this(types, element, Collections.emptyList());
  }

  public StandaloneTypeVariable(
      Types types, TypeParameterElement element, List<? extends AnnotationMirror> annotations) {
    super(TypeKind.TYPEVAR, annotations);
    this.element = element;
    lowerBound = types.getNullType();
  }

  private StandaloneTypeVariable(
      TypeParameterElement element,
      NullType lowerBound,
      List<? extends AnnotationMirror> annotations) {
    super(TypeKind.TYPEVAR, annotations);
    this.element = element;
    this.lowerBound = lowerBound;
  }

  @Override
  public Element asElement() {
    return element;
  }

  @Override
  public TypeMirror getUpperBound() {
    if (upperBound == null) {
      List<? extends TypeMirror> bounds = element.getBounds();

      if (bounds.size() == 1) {
        upperBound = bounds.get(0);
      } else {
        upperBound = new StandaloneIntersectionType(bounds);
      }
    }
    return upperBound;
  }

  @Override
  public TypeMirror getLowerBound() {
    // TODO(jkeljo): Capture conversion can create a non-null lower bound, but we don't need it yet.
    return lowerBound;
  }

  @Override
  public String toString() {
    return element.toString();
  }

  @Override
  public StandaloneTypeVariable cloneWithAnnotationMirrors(
      List<? extends AnnotationMirror> newAnnotations) {
    return new StandaloneTypeVariable(this.element, this.lowerBound, newAnnotations);
  }
}
