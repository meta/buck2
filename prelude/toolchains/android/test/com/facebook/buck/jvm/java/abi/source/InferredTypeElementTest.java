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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.jvm.java.abi.source.api.CannotInferException;
import com.facebook.buck.jvm.java.testutil.compiler.CompilerTreeApiTest;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.NestingKind;
import javax.lang.model.element.TypeElement;
import javax.lang.model.type.DeclaredType;
import javax.lang.model.type.TypeKind;
import javax.lang.model.type.TypeMirror;
import org.junit.Before;
import org.junit.Test;

public class InferredTypeElementTest extends CompilerTreeApiTest {
  @Before
  public void setUp() {
    testCompiler.useFrontendOnlyJavacTask();
  }

  @Test
  public void testGetSimpleNameInferredFromIdentifier() throws IOException {
    compile(
        Joiner.on('\n').join("package com.example.buck;", "public class Foo extends Bar {", "}"));

    DeclaredType superclass =
        (DeclaredType) elements.getTypeElement("com.example.buck.Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("Bar", element.getSimpleName());
  }

  @Test
  public void testGetQualifiedNameInferredFromIdentifier() throws IOException {
    compile(
        Joiner.on('\n').join("package com.example.buck;", "public class Foo extends Bar {", "}"));

    DeclaredType superclass =
        (DeclaredType) elements.getTypeElement("com.example.buck.Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("com.example.buck.Bar", element.getQualifiedName());
  }

  @Test
  public void testGetSimpleNameInferredFromIdentifierDefaultPackage() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends Bar {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("Bar", element.getSimpleName());
  }

  @Test
  public void testGetQualifiedNameInferredFromIdentifierDefaultPackage() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends Bar {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("Bar", element.getQualifiedName());
  }

  @Test
  public void testGetSimpleNameInferredFromQualifiedName() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("Bar", element.getSimpleName());
  }

  @Test
  public void testGetQualifiedNameInferredFromQualifiedName() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("com.example.buck.Bar", element.getQualifiedName());
  }

  @Test
  public void testMemberClassGetSimpleNameInferredFromQualifiedName() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar.Baz {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("Baz", element.getSimpleName());
  }

  @Test
  public void testMemberClassGetQualifiedNameInferredFromQualifiedName() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar.Baz {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertNameEquals("com.example.buck.Bar.Baz", element.getQualifiedName());
  }

  @Test
  public void testGetNestingKindTopLevel() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertEquals(NestingKind.TOP_LEVEL, element.getNestingKind());
  }

  @Test
  public void testGetNestingKindMember() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar.Baz {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    assertEquals(NestingKind.MEMBER, element.getNestingKind());
  }

  @Test
  public void testAsType() throws IOException {
    compile(Joiner.on('\n').join("public class Foo extends com.example.buck.Bar.Baz {", "}"));

    DeclaredType superclass = (DeclaredType) elements.getTypeElement("Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    TypeMirror type = element.asType();

    assertEquals(TypeKind.DECLARED, type.getKind());
    DeclaredType fooDeclaredType = (DeclaredType) type;
    assertSame(element, fooDeclaredType.asElement());
  }

  @Test
  public void testCannotInferMessageOffersOnlySourceOnlyAbiDeps() throws IOException {
    compile(
        Joiner.on('\n').join("package com.example.buck;", "public class Foo extends Bar {", "}"));

    DeclaredType superclass =
        (DeclaredType) elements.getTypeElement("com.example.buck.Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();
    String message = assertThrows(CannotInferException.class, element::getModifiers).getMessage();

    assertTrue(
        message,
        message.contains(
            "2. The module containing com.example.buck.Bar needs to be added to this target's"
                + " source_only_abi_deps"));
    assertFalse(message, message.contains("required_for_source_only_abi"));
  }

  @Test
  public void testSupertypeOnClasspathIsNotInferred() throws IOException {
    withClasspath(
        ImmutableMap.of(
            "com/example/buck/Bar.java", "package com.example.buck; public class Bar { }"));
    compile(
        Joiner.on('\n').join("package com.example.buck;", "public class Foo extends Bar {", "}"));

    DeclaredType superclass =
        (DeclaredType) elements.getTypeElement("com.example.buck.Foo").getSuperclass();
    TypeElement element = (TypeElement) superclass.asElement();

    assertFalse(element instanceof InferredTypeElement);
    assertTrue(element.getModifiers().contains(Modifier.PUBLIC));
  }
}
