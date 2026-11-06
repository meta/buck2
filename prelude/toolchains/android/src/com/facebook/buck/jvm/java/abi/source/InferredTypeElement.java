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

import com.facebook.buck.jvm.java.abi.source.api.CannotInferException;
import com.facebook.infer.annotation.Nullsafe;
import java.util.List;
import java.util.Objects;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.ElementVisitor;
import javax.lang.model.element.Name;
import javax.lang.model.element.NestingKind;
import javax.lang.model.type.TypeMirror;
import org.jspecify.annotations.Nullable;

/**
 * Represents a {@link javax.lang.model.element.TypeElement} whose existence has been inferred from
 * references to it in the code.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class InferredTypeElement extends InferredElement implements ArtificialTypeElement {
  private final Name qualifiedName;
  private final TypeMirror typeMirror;

  protected InferredTypeElement(
      Name simpleName, Name qualifiedName, ArtificialElement enclosingElement) {
    super(simpleName, enclosingElement);
    this.qualifiedName = qualifiedName;
    typeMirror = new InferredDeclaredType(this);

    enclosingElement.addEnclosedElement(this);
  }

  @Override
  public List<ArtificialElement> getEnclosedElements() {
    throw new UnsupportedOperationException(
        String.format(
            "We cannot know the enclosed elements of an inferred type: %s.", qualifiedName));
  }

  @Override
  public ArtificialElement getEnclosingElement() {
    return Objects.requireNonNull(super.getEnclosingElement());
  }

  @Override
  public List<? extends ArtificialTypeParameterElement> getTypeParameters() {
    throw new CannotInferException("type parameters", this);
  }

  @Override
  public TypeMirror asType() {
    return typeMirror;
  }

  @Override
  public ElementKind getKind() {
    throw new CannotInferException("kind", this);
  }

  @Override
  public <R, P> R accept(ElementVisitor<R, P> v, @Nullable P p) {
    return v.visitType(this, p);
  }

  @Override
  public NestingKind getNestingKind() {
    // We'll never need to infer local or anonymous classes, so there are only two options left
    // and we can tell the difference:
    if (getEnclosingElement() instanceof InferredTypeElement) {
      return NestingKind.MEMBER;
    }

    return NestingKind.TOP_LEVEL;
  }

  @Override
  public TypeMirror getSuperclass() {
    throw new CannotInferException("superclass", this);
  }

  @Override
  public List<? extends TypeMirror> getInterfaces() {
    throw new CannotInferException("interfaces", this);
  }

  @Override
  public Name getQualifiedName() {
    return qualifiedName;
  }

  @Override
  public String toString() {
    return qualifiedName.toString();
  }
}
