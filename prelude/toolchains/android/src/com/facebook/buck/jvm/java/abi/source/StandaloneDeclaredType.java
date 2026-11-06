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
import javax.lang.model.element.TypeElement;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Types;

/**
 * An implementation of {@link DeclaredType} that is not dependent on any particular compiler
 * implementation. It requires {@link javax.lang.model.element.Element} and {@link TypeMirror}
 * objects, but does not depend on any particular implementation of them (beyond the spec).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class StandaloneDeclaredType extends StandaloneTypeMirror implements DeclaredType {

  private final TypeElement typeElement;
  private final TypeMirror enclosingType;
  private final List<? extends TypeMirror> typeArguments;

  public StandaloneDeclaredType(Types types, TypeElement typeElement) {
    this(types, typeElement, Collections.emptyList());
  }

  public StandaloneDeclaredType(
      Types types, TypeElement typeElement, List<? extends TypeMirror> typeArguments) {
    this(typeElement, typeArguments, types.getNoType(TypeKind.NONE));
  }

  public StandaloneDeclaredType(
      TypeElement typeElement, List<? extends TypeMirror> typeArguments, TypeMirror enclosingType) {
    this(typeElement, typeArguments, enclosingType, Collections.emptyList());
  }

  public StandaloneDeclaredType(
      TypeElement typeElement,
      List<? extends TypeMirror> typeArguments,
      TypeMirror enclosingType,
      List<? extends AnnotationMirror> annotations) {
    super(TypeKind.DECLARED, annotations);
    this.typeElement = typeElement;
    this.typeArguments = List.copyOf(typeArguments);
    this.enclosingType = enclosingType;
  }

  /* package */ StandaloneDeclaredType(TypeElement typeElement) {
    super(TypeKind.DECLARED, Collections.emptyList());
    this.typeElement = typeElement;
    this.typeArguments = Collections.emptyList();
    this.enclosingType = this;
  }

  @Override
  public Element asElement() {
    return typeElement;
  }

  @Override
  public TypeMirror getEnclosingType() {
    return enclosingType;
  }

  @Override
  public List<? extends TypeMirror> getTypeArguments() {
    return typeArguments;
  }

  @Override
  public String toString() {
    StringBuilder builder = new StringBuilder();

    builder.append(typeElement);
    if (!typeArguments.isEmpty()) {
      builder.append('<');
      for (int i = 0; i < typeArguments.size(); i++) {
        if (i > 0) {
          builder.append(',');
        }
        builder.append(typeArguments.get(i));
      }
      builder.append('>');
    }
    return builder.toString();
  }

  @Override
  public StandaloneDeclaredType cloneWithAnnotationMirrors(
      List<? extends AnnotationMirror> newAnnotations) {
    return new StandaloneDeclaredType(
        this.typeElement, this.typeArguments, this.enclosingType, newAnnotations);
  }
}
