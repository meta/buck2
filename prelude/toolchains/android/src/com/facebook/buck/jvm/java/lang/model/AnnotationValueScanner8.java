/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.lang.model;

import com.facebook.buck.util.liteinfersupport.Nullable;
import com.facebook.infer.annotation.Nullsafe;
import java.util.List;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.AnnotationValue;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.AbstractAnnotationValueVisitor8;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class AnnotationValueScanner8<R, P> extends AbstractAnnotationValueVisitor8<R, P> {
  @Nullable private final R defaultValue;

  protected AnnotationValueScanner8() {
    this(null);
  }

  protected AnnotationValueScanner8(@Nullable R defaultValue) {
    this.defaultValue = defaultValue;
  }

  @Nullable
  public final R scan(Iterable<? extends AnnotationValue> iterable, P p) {
    R result = defaultValue;
    for (AnnotationValue value : iterable) {
      result = scan(value, p);
    }
    return result;
  }

  public R scan(AnnotationValue value, P p) {
    return value.accept(this, p);
  }

  public final R scan(AnnotationValue value) {
    return value.accept(this, null);
  }

  @Override
  @Nullable
  public R visitBoolean(boolean b, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitByte(byte b, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitChar(char c, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitDouble(double d, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitFloat(float f, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitInt(int i, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitLong(long i, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitShort(short s, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitString(String s, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitType(TypeMirror t, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitEnumConstant(VariableElement c, P p) {
    return defaultValue;
  }

  @Override
  @Nullable
  public R visitAnnotation(AnnotationMirror a, P p) {
    return scan(a.getElementValues().values(), p);
  }

  @Override
  @Nullable
  public R visitArray(List<? extends AnnotationValue> vals, P p) {
    return scan(vals, p);
  }

  @Override
  @Nullable
  public R visitUnknown(AnnotationValue av, P p) {
    return defaultValue;
  }
}
