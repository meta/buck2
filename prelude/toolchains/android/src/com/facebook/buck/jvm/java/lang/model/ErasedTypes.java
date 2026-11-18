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

import com.facebook.infer.annotation.Nullsafe;
import java.util.List;
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeElement;
import javax.lang.model.type.ArrayType;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.ExecutableType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Types;

/**
 * Compares erased types the way the JVM compares descriptors. {@link Types#isSameType} treats an
 * error type (a class that is not on the classpath) as the same as any type. An error type cannot
 * name a class that is on the classpath, so here it equals only an error type of the same name.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ErasedTypes {
  private ErasedTypes() {}

  public static boolean isSameType(Types types, TypeMirror a, TypeMirror b) {
    TypeMirror erasedA = types.erasure(a);
    TypeMirror erasedB = types.erasure(b);
    if (hasError(erasedA) || hasError(erasedB)) {
      return hasError(erasedA)
          && hasError(erasedB)
          && errorName(erasedA).equals(errorName(erasedB));
    }
    return types.isSameType(erasedA, erasedB);
  }

  /** Whether two methods have the same erased parameter types and return type. */
  public static boolean isSameDescriptor(Types types, ExecutableType a, ExecutableType b) {
    List<? extends TypeMirror> parametersA = a.getParameterTypes();
    List<? extends TypeMirror> parametersB = b.getParameterTypes();
    if (parametersA.size() != parametersB.size()) {
      return false;
    }
    for (int i = 0; i < parametersA.size(); i++) {
      if (!isSameType(types, parametersA.get(i), parametersB.get(i))) {
        return false;
      }
    }
    return isSameType(types, a.getReturnType(), b.getReturnType());
  }

  /**
   * The name of an erased type that is or contains an error type: the qualified name of the class
   * it stands for, with nested classes in canonical form, as an import names them.
   */
  private static String errorName(TypeMirror type) {
    StringBuilder dimensions = new StringBuilder();
    while (type.getKind() == TypeKind.ARRAY) {
      dimensions.append("[]");
      type = ((ArrayType) type).getComponentType();
    }
    type = ClassFileTypes.originalType(type);
    if (type.getKind() != TypeKind.DECLARED && type.getKind() != TypeKind.ERROR) {
      return type.toString() + dimensions;
    }
    Element element = ((DeclaredType) type).asElement();
    String name =
        element instanceof TypeElement
            ? ((TypeElement) element).getQualifiedName().toString()
            : type.toString();
    return (name.isEmpty() ? type.toString() : name).replace('$', '.') + dimensions;
  }

  private static boolean hasError(TypeMirror type) {
    while (type.getKind() == TypeKind.ARRAY) {
      type = ((ArrayType) type).getComponentType();
    }
    return type.getKind() == TypeKind.ERROR;
  }
}
