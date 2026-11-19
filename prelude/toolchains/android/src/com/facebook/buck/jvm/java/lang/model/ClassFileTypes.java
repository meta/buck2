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
import com.sun.tools.javac.code.Symbol;
import com.sun.tools.javac.code.Type;
import javax.lang.model.element.Element;
import javax.lang.model.element.Name;
import javax.lang.model.type.ArrayType;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.ExecutableType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Types;
import javax.tools.JavaFileObject;
import org.jspecify.annotations.Nullable;

/**
 * Descriptors of members read from class files. A class file names every class in a member's
 * descriptor by its binary name, so javac knows that name even for a class that is not on the
 * classpath (an error type). Uses internal compiler APIs for the binary name of such a class.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ClassFileTypes {
  private ClassFileTypes() {}

  /** Whether {@code element} is a class javac read from a class file. */
  public static boolean isReadFromClassFile(Element element) {
    if (!(element instanceof Symbol.ClassSymbol)) {
      return false;
    }
    Symbol.ClassSymbol symbol = (Symbol.ClassSymbol) element;
    try {
      symbol.complete();
    } catch (Symbol.CompletionFailure e) {
      return false;
    }
    JavaFileObject classfile = symbol.classfile;
    return classfile != null && classfile.getKind() == JavaFileObject.Kind.CLASS;
  }

  /**
   * The descriptor of an erased method type of a member read from a class file, or null when a type
   * in it has no binary name.
   */
  public static @Nullable String getDescriptor(Types types, ExecutableType method) {
    StringBuilder descriptor = new StringBuilder("(");
    for (TypeMirror parameter : method.getParameterTypes()) {
      if (!appendDescriptor(types.erasure(parameter), descriptor)) {
        return null;
      }
    }
    descriptor.append(')');
    if (!appendDescriptor(types.erasure(method.getReturnType()), descriptor)) {
      return null;
    }
    return descriptor.toString();
  }

  /** The internal name of a class type, including one not on the classpath, or null. */
  public static @Nullable String getInternalName(TypeMirror type) {
    if (type.getKind() != TypeKind.DECLARED && type.getKind() != TypeKind.ERROR) {
      return null;
    }
    Element element = ((DeclaredType) originalType(type)).asElement();
    if (!(element instanceof Symbol.ClassSymbol)) {
      return null;
    }
    Name flatName = ((Symbol.ClassSymbol) element).flatName();
    if (flatName == null) {
      return null;
    }
    String name = flatName.toString();
    return name.isEmpty() ? null : name.replace('.', '/');
  }

  /**
   * The class an error type stands for, when javac knows it: an error type read from a class file
   * keeps the class its descriptor names as its original type. A simple name in the source that
   * resolves to a missing class has none ({@code <any>}).
   */
  public static TypeMirror originalType(TypeMirror type) {
    TypeMirror current = type;
    for (int depth = 0; depth < 8 && current instanceof Type.ErrorType; depth++) {
      Type original = ((Type.ErrorType) current).getOriginalType();
      if (original == null || original.getKind() == TypeKind.NONE || original == current) {
        break;
      }
      current = original;
    }
    return current;
  }

  private static boolean appendDescriptor(TypeMirror type, StringBuilder descriptor) {
    switch (type.getKind()) {
      case BOOLEAN:
        descriptor.append('Z');
        return true;
      case BYTE:
        descriptor.append('B');
        return true;
      case CHAR:
        descriptor.append('C');
        return true;
      case SHORT:
        descriptor.append('S');
        return true;
      case INT:
        descriptor.append('I');
        return true;
      case LONG:
        descriptor.append('J');
        return true;
      case FLOAT:
        descriptor.append('F');
        return true;
      case DOUBLE:
        descriptor.append('D');
        return true;
      case VOID:
        descriptor.append('V');
        return true;
      case ARRAY:
        descriptor.append('[');
        return appendDescriptor(((ArrayType) type).getComponentType(), descriptor);
      case DECLARED:
      case ERROR:
        String internalName = getInternalName(type);
        if (internalName == null) {
          return false;
        }
        descriptor.append('L').append(internalName).append(';');
        return true;
      // $CASES-OMITTED$
      default:
        return false;
    }
  }
}
