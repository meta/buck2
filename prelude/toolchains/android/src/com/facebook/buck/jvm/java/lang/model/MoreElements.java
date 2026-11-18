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
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.stream.Stream;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.AnnotationValue;
import javax.lang.model.element.Element;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.NestingKind;
import javax.lang.model.element.PackageElement;
import javax.lang.model.element.TypeElement;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Elements;
import org.jspecify.annotations.Nullable;

/**
 * More utility functions for working with {@link Element}s, along the lines of those found on
 * {@link javax.lang.model.util.Elements}.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class MoreElements {

  private MoreElements() {}

  /** Get a package by name, even if it has no members. */
  @Nullable
  public static PackageElement getPackageElementEvenIfEmpty(
      Elements elements, CharSequence packageName) {
    // In Java 9 and later, only the new {@link
    // javax.lang.model.util.Elements#getPackageElement(ModuleElement, CharSequence)},
    // which {@link javax.lang.mode.util.Elements#getAllPackageElements} calls, returns empty
    // packages.
    Set<? extends PackageElement> packages = elements.getAllPackageElements(packageName);
    if (packages.isEmpty()) {
      return null;
    }
    if (packages.size() > 1) {
      // TODO(jtorkkola): Add proper Java 9+ module support.
      throw new IllegalStateException("Found more than one package matching " + packageName);
    }
    return packages.iterator().next();
  }

  @Nullable
  public static TypeElement getSuperclass(TypeElement type) {
    TypeMirror superclassType = type.getSuperclass();
    switch (superclassType.getKind()) {
      case DECLARED:
      case ERROR:
        return (TypeElement) ((DeclaredType) superclassType).asElement();
      case NONE:
        return null;
      // $CASES-OMITTED$
      default:
        throw new IllegalArgumentException(superclassType.toString());
    }
  }

  public static Stream<TypeElement> getInterfaces(TypeElement type) {
    return type.getInterfaces().stream()
        .filter(it -> it.getKind() == TypeKind.DECLARED || it.getKind() == TypeKind.ERROR)
        .map(it -> (TypeElement) ((DeclaredType) it).asElement());
  }

  public static Stream<TypeElement> getTransitiveSuperclasses(TypeElement type) {
    Stream.Builder<TypeElement> builder = Stream.builder();

    TypeElement walker = getSuperclass(type);
    while (walker != null) {
      builder.add(walker);
      walker = getSuperclass(walker);
    }

    return builder.build();
  }

  public static boolean isTransitiveMemberClass(TypeElement inner, TypeElement outer) {
    Element walker = inner.getEnclosingElement();
    while (walker != null) {
      if (walker == outer) {
        return true;
      }
      walker = walker.getEnclosingElement();
    }

    return false;
  }

  public static TypeElement getTypeElement(Element element) {
    if (element.getKind() == ElementKind.PACKAGE) {
      throw new IllegalArgumentException();
    }

    Element walker = element;
    while (!walker.getKind().isClass() && !walker.getKind().isInterface()) {
      walker = Objects.requireNonNull(walker.getEnclosingElement());
    }

    return (TypeElement) walker;
  }

  public static TypeElement getTopLevelTypeElement(Element element) {
    if (element.getKind() == ElementKind.PACKAGE) {
      throw new IllegalArgumentException();
    }

    Element walker = element;
    while (walker.getEnclosingElement() != null
        && walker.getEnclosingElement().getKind() != ElementKind.PACKAGE) {
      walker = Objects.requireNonNull(walker.getEnclosingElement());
    }

    return (TypeElement) walker;
  }

  public static PackageElement getPackageElement(Element element) {
    Element walker = element;
    while (walker.getKind() != ElementKind.PACKAGE) {
      walker = Objects.requireNonNull(walker.getEnclosingElement());
    }

    return (PackageElement) walker;
  }

  public static boolean isInnerClassConstructor(ExecutableElement e) {
    return isConstructor(e) && isInnerClass((TypeElement) e.getEnclosingElement());
  }

  public static boolean isConstructor(ExecutableElement e) {
    return e.getSimpleName().contentEquals("<init>");
  }

  public static boolean isInnerClass(TypeElement e) {
    return e.getNestingKind() == NestingKind.MEMBER && !e.getModifiers().contains(Modifier.STATIC);
  }

  public static Element getOuterClass(ExecutableElement e) {
    Element innerClass = e.getEnclosingElement();
    Element outerClass = innerClass.getEnclosingElement();
    if (outerClass == null) {
      throw new IllegalArgumentException(
          String.format("Cannot get outer class of element which isn't an inner class: %s", e));
    }
    return outerClass;
  }

  public static boolean isRuntimeRetention(AnnotationMirror annotation) {
    DeclaredType annotationType = annotation.getAnnotationType();
    TypeElement annotationTypeElement = (TypeElement) annotationType.asElement();

    AnnotationMirror retentionAnnotation =
        findAnnotation("java.lang.annotation.Retention", annotationTypeElement);
    if (retentionAnnotation == null) {
      return false;
    }

    VariableElement retentionPolicy =
        (VariableElement) Objects.requireNonNull(findAnnotationValue(retentionAnnotation, "value"));

    return retentionPolicy.getSimpleName().contentEquals("RUNTIME");
  }

  public static boolean isSourceRetention(AnnotationMirror annotation) {
    DeclaredType annotationType = annotation.getAnnotationType();
    TypeElement annotationTypeElement = (TypeElement) annotationType.asElement();

    AnnotationMirror retentionAnnotation =
        findAnnotation("java.lang.annotation.Retention", annotationTypeElement);
    if (retentionAnnotation == null) {
      return false;
    }

    VariableElement retentionPolicy =
        (VariableElement) Objects.requireNonNull(findAnnotationValue(retentionAnnotation, "value"));

    return retentionPolicy.getSimpleName().contentEquals("SOURCE");
  }

  @Nullable
  private static AnnotationMirror findAnnotation(CharSequence name, Element element) {
    for (AnnotationMirror annotationMirror : element.getAnnotationMirrors()) {
      DeclaredType annotationType = annotationMirror.getAnnotationType();
      TypeElement annotationTypeElement = (TypeElement) annotationType.asElement();

      if (annotationTypeElement.getQualifiedName().contentEquals(name)) {
        return annotationMirror;
      }
    }

    return null;
  }

  @Nullable
  private static Object findAnnotationValue(AnnotationMirror annotation, String name) {
    for (Map.Entry<? extends ExecutableElement, ? extends AnnotationValue> entry :
        annotation.getElementValues().entrySet()) {
      if (entry.getKey().getSimpleName().contentEquals(name)) {
        return entry.getValue().getValue();
      }
    }

    return null;
  }
}
