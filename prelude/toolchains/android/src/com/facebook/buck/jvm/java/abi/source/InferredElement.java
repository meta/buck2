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
import java.lang.annotation.Annotation;
import java.util.List;
import java.util.Set;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.Element;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import org.jspecify.annotations.Nullable;

/**
 * Represents an {@link javax.lang.model.element.Element} whose existence has been inferred from
 * references to it in the code.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
abstract class InferredElement implements ArtificialElement {
  private final Name simpleName;
  @Nullable private final ArtificialElement enclosingElement;

  protected InferredElement(Name simpleName, @Nullable ArtificialElement enclosingElement) {
    this.simpleName = simpleName;
    this.enclosingElement = enclosingElement;
  }

  @Override
  public void addEnclosedElement(Element element) {
    // getEnclosedElements throws, so don't bother keeping track of them.
  }

  @Override
  public Set<Modifier> getModifiers() {
    throw new CannotInferException("modifiers", this);
  }

  @Override
  public final Name getSimpleName() {
    return simpleName;
  }

  @Override
  @Nullable
  public ArtificialElement getEnclosingElement() {
    return enclosingElement;
  }

  @Override
  public List<? extends Element> getEnclosedElements() {
    throw new CannotInferException("enclosed elements", this);
  }

  @Override
  public final List<? extends AnnotationMirror> getAnnotationMirrors() {
    throw new CannotInferException("annotations", this);
  }

  @Override
  public final <A extends Annotation> A getAnnotation(Class<A> annotationType) {
    throw new CannotInferException("annotations", this);
  }

  @Override
  public final <A extends Annotation> A[] getAnnotationsByType(Class<A> annotationType) {
    throw new CannotInferException("annotations", this);
  }
}
