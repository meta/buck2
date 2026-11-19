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

import static org.hamcrest.MatcherAssert.assertThat;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.google.common.base.Joiner;
import java.io.IOException;
import java.util.stream.Collectors;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.Name;
import javax.lang.model.element.TypeElement;
import javax.lang.model.element.TypeParameterElement;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import org.hamcrest.Matchers;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

@RunWith(Parameterized.class)
public class TreeBackedExecutableElementTest extends CompilerTreeApiParameterizedTest {
  @Test
  public void testGetReturnType() throws IOException {
    compile(Joiner.on('\n').join("abstract class Foo {", "  public abstract String foo();", "}"));

    ExecutableElement element = findMethod("foo", elements.getTypeElement("Foo"));
    TypeMirror stringType = elements.getTypeElement("java.lang.String").asType();

    assertSameType(stringType, element.getReturnType());
  }

  @Test
  public void testGetVoidReturnType() throws IOException {
    compile(Joiner.on('\n').join("abstract class Foo {", "  public void foo();", "}"));

    ExecutableElement element = findMethod("foo", elements.getTypeElement("Foo"));
    TypeMirror voidType = types.getNoType(TypeKind.VOID);

    assertSameType(voidType, element.getReturnType());
  }

  @Test
  public void testGetConstructorReturnType() throws IOException {
    compile(Joiner.on('\n').join("class Foo {", "}"));

    ExecutableElement element = findDefaultConstructor(elements.getTypeElement("Foo"));
    TypeMirror voidType = types.getNoType(TypeKind.VOID);

    assertSameType(voidType, element.getReturnType());
  }

  @Test
  public void testGetReceiverTypeOfStaticIsNone() throws IOException {
    compile(Joiner.on('\n').join("class Foo {", "  static void foo() { }", "}"));

    ExecutableElement element = findMethod("foo", elements.getTypeElement("Foo"));
    TypeMirror receiverType = element.getReceiverType();
    TypeMirror noneType = types.getNoType(TypeKind.NONE);

    assertSameType(noneType, receiverType);
  }

  @Test
  public void testGetReceiverTypeOfTopLevelConstructorIsNone() throws IOException {
    compile(Joiner.on('\n').join("class Foo {", "  Foo () { }", "}"));

    ExecutableElement element = findDefaultConstructor(elements.getTypeElement("Foo"));
    TypeMirror receiverType = element.getReceiverType();
    TypeMirror noneType = types.getNoType(TypeKind.NONE);

    assertSameType(noneType, receiverType);
  }

  @Test
  public void testGetReceiverTypeOfInnerConstructorIsEnclosingType() throws IOException {
    compile(
        Joiner.on('\n')
            .join("class Foo {", "  class Bar {", "  Bar (Foo Foo.this) { }", "  }", "}"));

    ExecutableElement element = findDefaultConstructor(elements.getTypeElement("Foo.Bar"));
    TypeMirror receiverType = element.getReceiverType();

    assertSameType(elements.getTypeElement("Foo").asType(), receiverType);
  }

  @Test
  public void testGetReceiverTypeOfInnerConstructorIsEnclosingTypeGeneric() throws IOException {
    compile(
        Joiner.on('\n')
            .join("class Foo<T> {", "  class Bar {", "  Bar (Foo<T> Foo.this) { }", "  }", "}"));

    ExecutableElement element = findDefaultConstructor(elements.getTypeElement("Foo.Bar"));
    TypeMirror receiverType = element.getReceiverType();

    assertSameType(elements.getTypeElement("Foo").asType(), receiverType);
  }

  @Test
  public void testGetReceiverTypeOfInstanceMethodIsEnclosingType() throws IOException {
    compile(Joiner.on('\n').join("class Foo {", "  void foo(Foo this) { }", "}"));

    TypeElement fooClass = elements.getTypeElement("Foo");
    ExecutableElement fooMethod = findMethod("foo", fooClass);
    TypeMirror receiverType = fooMethod.getReceiverType();

    assertSameType(fooClass.asType(), receiverType);
  }

  @Test
  public void testGetReceiverTypeOfInstanceMethodIsEnclosingTypeGeneric() throws IOException {
    compile(Joiner.on('\n').join("class Foo<T> {", "  void foo(Foo<T> this) { }", "}"));

    TypeElement fooClass = elements.getTypeElement("Foo");
    ExecutableElement fooMethod = findMethod("foo", fooClass);
    TypeMirror receiverType = fooMethod.getReceiverType();

    assertSameType(fooClass.asType(), receiverType);
  }

  @Test
  public void testGetParameters() throws IOException {
    compile(
        Joiner.on('\n')
            .join("abstract class Foo {", "  public abstract void foo(String s, int i);", "}"));

    VariableElement sParameter =
        findParameter("s", findMethod("foo", elements.getTypeElement("Foo")));
    VariableElement iParameter =
        findParameter("i", findMethod("foo", elements.getTypeElement("Foo")));

    assertSameType(elements.getTypeElement("java.lang.String").asType(), sParameter.asType());

    assertSameType(types.getPrimitiveType(TypeKind.INT), iParameter.asType());
  }

  @Test
  public void testNotVarArgs() throws IOException {
    compile(
        Joiner.on('\n')
            .join("abstract class Foo {", "  public abstract void foo(String[] strings);", "}"));

    TypeMirror stringArrayType =
        types.getArrayType(elements.getTypeElement("java.lang.String").asType());

    ExecutableElement method = findMethod("foo", elements.getTypeElement("Foo"));
    assertFalse(method.isVarArgs());
    assertSameType(stringArrayType, findParameter("strings", method).asType());
  }

  @Test
  public void testVarArgs() throws IOException {
    compile(
        Joiner.on('\n')
            .join("abstract class Foo {", "  public abstract void foo(String... strings);", "}"));

    TypeMirror stringArrayType =
        types.getArrayType(elements.getTypeElement("java.lang.String").asType());

    ExecutableElement method = findMethod("foo", elements.getTypeElement("Foo"));
    assertTrue(method.isVarArgs());
    assertSameType(stringArrayType, findParameter("strings", method).asType());
  }

  @Test
  public void testTypeUseAnnotatedArrayParameter() throws IOException {
    compile(
        Joiner.on('\n')
            .join(
                "import java.lang.annotation.*;",
                "abstract class Foo {",
                "  public abstract void foo(String @Anno [] strings);",
                "}",
                "@Target(ElementType.TYPE_USE)",
                "@interface Anno { }"));

    TypeMirror stringArrayType =
        types.getArrayType(elements.getTypeElement("java.lang.String").asType());

    ExecutableElement method = findMethod("foo", elements.getTypeElement("Foo"));
    assertSameType(stringArrayType, findParameter("strings", method).asType());
  }

  @Test
  public void testTypeUseAnnotatedTypeArgumentsParameter() throws IOException {
    compile(
        Joiner.on('\n')
            .join(
                "import java.lang.annotation.*;",
                "import java.util.List;",
                "abstract class Foo {",
                "  public abstract void foo(List<@Anno List<String>> lists, List<@Anno ? extends"
                    + " Number> numbers);",
                "}",
                "@Target(ElementType.TYPE_USE)",
                "@interface Anno { }"));

    TypeElement list = elements.getTypeElement("java.util.List");
    TypeMirror listOfListOfString =
        types.getDeclaredType(
            list,
            types.getDeclaredType(list, elements.getTypeElement("java.lang.String").asType()));
    TypeMirror listOfExtendsNumber =
        types.getDeclaredType(
            list,
            types.getWildcardType(elements.getTypeElement("java.lang.Number").asType(), null));

    ExecutableElement method = findMethod("foo", elements.getTypeElement("Foo"));
    assertSameType(listOfListOfString, findParameter("lists", method).asType());
    assertSameType(listOfExtendsNumber, findParameter("numbers", method).asType());
  }

  @Test
  public void testNotDefault() throws IOException {
    compile(Joiner.on('\n').join("interface Foo {", "  void foo();", "}"));

    assertFalse(findMethod("foo", elements.getTypeElement("Foo")).isDefault());
  }

  @Test
  public void testDefault() throws IOException {
    compile(Joiner.on('\n').join("interface Foo {", "  default void foo() {};", "}"));

    assertTrue(findMethod("foo", elements.getTypeElement("Foo")).isDefault());
  }

  @Test
  public void testGetThrownTypes() throws IOException {
    compile(
        Joiner.on('\n')
            .join(
                "abstract class Foo {",
                "  public abstract void foo() throws Exception, RuntimeException;",
                "}"));

    ExecutableElement method = findMethod("foo", elements.getTypeElement("Foo"));

    assertThat(
        method.getThrownTypes().stream().map(TypeMirror::toString).collect(Collectors.toList()),
        Matchers.contains(
            elements.getTypeElement("java.lang.Exception").asType().toString(),
            elements.getTypeElement("java.lang.RuntimeException").asType().toString()));
  }

  /**
   * See {@link com.facebook.buck.jvm.java.abi.source.TreeBackedAnnotationValueTest} for lots of
   * tests of methods with default values.
   */
  @Test
  public void testGetDefaultValueNoDefault() throws IOException {
    compile(Joiner.on('\n').join("@interface Foo {", "  int value();", "}"));

    assertThat(
        findMethod("value", elements.getTypeElement("Foo")).getDefaultValue(),
        Matchers.nullValue());
  }

  /**
   * See {@link TreeBackedTypeParameterElementTest} for lots of tests of the type parameter elements
   * themselves.
   */
  @Test
  public void testGetTypeParameters() throws IOException {
    compile(Joiner.on('\n').join("class Foo {", "  public <T, U> void foo(T t, U u) { }", "}"));

    assertThat(
        findMethod("foo", elements.getTypeElement("Foo")).getTypeParameters().stream()
            .map(TypeParameterElement::getSimpleName)
            .map(Name::toString)
            .collect(Collectors.toList()),
        Matchers.contains("T", "U"));
  }

  // TODO(jkeljo): Uncomment when we stop supporting javac 7 and implement receiver types again
  /*  @Test
  public void testAsType() throws IOException {
    compile(
        Joiner.on('\n')
            .join(
                "abstract class Foo {",
                "public abstract <E extends Exception> int foo(Foo this, String s, long l, E e) throws E;",
                "}"));

    TypeElement fooTypeElement = elements.getTypeElement("Foo");
    ExecutableElement fooExecutableElement = findMethod("foo", fooTypeElement);
    TypeMirror typeMirror = fooExecutableElement.asType();
    TypeMirror typeParam = fooExecutableElement.getTypeParameters().get(0).asType();

    assertEquals(TypeKind.EXECUTABLE, typeMirror.getKind());
    ExecutableType type = (ExecutableType) typeMirror;
    assertSameType(types.getPrimitiveType(TypeKind.INT), type.getReturnType());
    assertSameType(fooTypeElement.asType(), type.getReceiverType());
    assertThat(
        type.getParameterTypes(),
        Matchers.contains(
            ImmutableList.of(
                sameType(elements.getTypeElement("java.lang.String").asType()),
                sameType(types.getPrimitiveType(TypeKind.LONG)),
                sameType(typeParam))));
    assertThat(type.getThrownTypes(), Matchers.contains(sameType(typeParam)));
  }*/
}
