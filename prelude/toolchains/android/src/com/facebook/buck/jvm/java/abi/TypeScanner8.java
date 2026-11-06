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

import com.facebook.infer.annotation.Nullsafe;
import java.util.HashSet;
import java.util.Set;
import javax.annotation.Nullable;
import javax.lang.model.element.TypeParameterElement;
import javax.lang.model.type.ArrayType;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.ErrorType;
import javax.lang.model.type.ExecutableType;
import javax.lang.model.type.IntersectionType;
import javax.lang.model.type.NoType;
import javax.lang.model.type.NullType;
import javax.lang.model.type.PrimitiveType;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.type.TypeVariable;
import javax.lang.model.type.UnionType;
import javax.lang.model.type.WildcardType;
import javax.lang.model.util.AbstractTypeVisitor8;

/**
 * A scanning visitor of {@link TypeMirror}s for {@link javax.lang.model.SourceVersion#RELEASE_8}.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class TypeScanner8<R, P> extends AbstractTypeVisitor8<R, P> {
  private final Set<TypeParameterElement> visitedTypeVars = new HashSet<>();
  @Nullable private final R defaultValue;

  protected TypeScanner8() {
    this(null);
  }

  protected TypeScanner8(@Nullable R defaultValue) {
    this.defaultValue = defaultValue;
  }

  @Nullable
  public final R scan(Iterable<? extends TypeMirror> iterable, P p) {
    R result = defaultValue;
    for (TypeMirror t : iterable) {
      result = scan(t, p);
    }
    return result;
  }

  @Nullable
  public R scan(TypeMirror t, P p) {
    return t.accept(this, p);
  }

  @Nullable
  public final R scan(TypeMirror t) {
    return t.accept(this, null);
  }

  @Override
  @Nullable
  public R visitPrimitive(PrimitiveType t, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitNull(NullType t, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitArray(ArrayType t, P p) {
    return scan(t.getComponentType(), p);
  }

  @Override
  @Nullable
  public R visitDeclared(DeclaredType t, P p) {
    scan(t.getEnclosingType(), p);
    return scan(t.getTypeArguments(), p);
  }

  @Override
  @Nullable
  public R visitError(ErrorType t, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitTypeVariable(TypeVariable t, P p) {
    // Avoid infinite recursion in cases like T extends List<T>
    TypeParameterElement typeParameterElement = (TypeParameterElement) t.asElement();
    if (visitedTypeVars.contains(typeParameterElement)) {
      return defaultValue;
    }
    visitedTypeVars.add(typeParameterElement);

    scan(t.getLowerBound(), p);
    return scan(t.getUpperBound(), p);
  }

  @Override
  @Nullable
  public R visitWildcard(WildcardType t, P p) {
    R result = defaultValue;
    TypeMirror extendsBound = t.getExtendsBound();
    if (extendsBound != null) {
      result = scan(extendsBound, p);
    }

    TypeMirror superBound = t.getSuperBound();
    if (superBound != null) {
      result = scan(superBound, p);
    }

    return result;
  }

  @Override
  @Nullable
  public R visitExecutable(ExecutableType t, P p) {
    // TODO(jkeljo): Either make this adaptive or just wait until we drop javac 7 support to
    // uncomment:
    // scan(t.getReceiverType(), p);
    scan(t.getTypeVariables(), p);
    scan(t.getReturnType(), p);
    scan(t.getParameterTypes(), p);
    return scan(t.getThrownTypes(), p);
  }

  @Override
  @Nullable
  public R visitNoType(NoType t, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitIntersection(IntersectionType t, P p) {
    return scan(t.getBounds(), p);
  }

  @Override
  @Nullable
  public R visitUnion(UnionType t, P p) {
    throw new UnsupportedOperationException("NYI");
  }
}
