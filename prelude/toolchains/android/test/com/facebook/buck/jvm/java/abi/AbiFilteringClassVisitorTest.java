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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyInt;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.verifyNoMoreInteractions;
import static org.mockito.Mockito.when;

import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import java.util.Arrays;
import java.util.Set;
import org.junit.Before;
import org.junit.Test;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.AnnotationNode;
import org.objectweb.asm.tree.ClassNode;
import org.objectweb.asm.tree.MethodNode;

public class AbiFilteringClassVisitorTest {
  private ClassVisitor mockVisitor;
  private AbiFilteringClassVisitor filteringVisitor;

  @Before
  public void setUp() {
    mockVisitor = mock(ClassVisitor.class);
    filteringVisitor =
        new AbiFilteringClassVisitor(mockVisitor, ImmutableList.of(), ImmutableSet.of(), false);
  }

  @Test
  public void testExcludesPrivateFields() {
    testExcludesFieldWithAccess(Opcodes.ACC_PRIVATE);
  }

  @Test
  public void testExcludesPrivateStaticFields() {
    testExcludesFieldWithAccess(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC);
  }

  @Test
  public void testExcludesSyntheticFields() {
    testExcludesFieldWithAccess(Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC);
  }

  @Test
  public void testIncludesPackageFields() {
    testIncludesFieldWithAccess(0);
  }

  @Test
  public void testIncludesPackageStaticFields() {
    testIncludesFieldWithAccess(Opcodes.ACC_STATIC);
  }

  @Test
  public void testIncludesPublicFields() {
    testIncludesFieldWithAccess(Opcodes.ACC_PUBLIC);
  }

  @Test
  public void testIncludesProtectedFields() {
    testIncludesFieldWithAccess(Opcodes.ACC_PROTECTED);
  }

  @Test
  public void testNotConfusedByOtherFieldAccessFlagsIncluding() {
    testIncludesFieldWithAccess(Opcodes.ACC_PUBLIC | Opcodes.ACC_STATIC | Opcodes.ACC_VOLATILE);
  }

  @Test
  public void testNotConfusedByOtherFieldAccessFlagsExcluding() {
    testExcludesFieldWithAccess(Opcodes.ACC_PRIVATE | Opcodes.ACC_STATIC | Opcodes.ACC_VOLATILE);
  }

  @Test
  public void testExcludesPrivateMethods() {
    testExcludesMethodWithAccess(Opcodes.ACC_PRIVATE);
  }

  @Test
  public void testIncludesPrivateMethodsWhenRetained() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of("foo"), ImmutableSet.of(), false);
    testIncludesMethodWithAccess(Opcodes.ACC_PRIVATE);
  }

  @Test
  public void testIncludesForInlineVariantOfRetainedMethod() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of("foo"), ImmutableSet.of(), false);
    int access = Opcodes.ACC_PRIVATE | Opcodes.ACC_SYNTHETIC;
    MethodVisitor delegate = mock(MethodVisitor.class);
    when(mockVisitor.visitMethod(access, "foo$$forInline", "()V", null, null)).thenReturn(delegate);
    MethodVisitor result =
        filteringVisitor.visitMethod(access, "foo$$forInline", "()V", null, null);
    verify(mockVisitor).visitMethod(access, "foo$$forInline", "()V", null, null);
    // Full visitor (body kept): the delegate is returned directly, not wrapped
    // in the code-skipping visitor used for ordinary ABI stubs.
    assertSame(delegate, result);
  }

  @Test
  public void testExcludesForInlineVariantOfNonRetainedMethod() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of("bar"), ImmutableSet.of(), false);
    filteringVisitor.visitMethod(
        Opcodes.ACC_PRIVATE | Opcodes.ACC_SYNTHETIC, "foo$$forInline", "()V", null, null);
    verify(mockVisitor, never()).visitMethod(anyInt(), anyString(), anyString(), any(), any());
  }

  @Test
  public void testIncludesPackageMethods() {
    testIncludesMethodWithAccess(Opcodes.ACC_PUBLIC);
  }

  @Test
  public void testIncludesProtectedMethods() {
    testIncludesMethodWithAccess(Opcodes.ACC_PUBLIC);
  }

  @Test
  public void testIncludesPublicMethods() {
    testIncludesMethodWithAccess(Opcodes.ACC_PUBLIC);
  }

  @Test
  public void testExcludesSyntheticMethods() {
    testExcludesMethodWithAccess(Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC);
  }

  @Test
  public void testIncludesSyntheticHiddenDeprecatedMethodInInterface() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of(), ImmutableSet.of(), false, ImmutableSet.of("foo()V"));
    visitInterface(filteringVisitor, "Foo");
    // Real Kotlin HIDDEN members carry ACC_DEPRECATED alongside ACC_SYNTHETIC; on an interface
    // `shouldInclude` drops SYNTHETIC, so this only passes via the HIDDEN-retention path.
    testIncludesMethodWithAccess(
        Opcodes.ACC_PUBLIC | Opcodes.ACC_ABSTRACT | Opcodes.ACC_SYNTHETIC | Opcodes.ACC_DEPRECATED);
  }

  @Test
  public void testIncludesSyntheticHiddenDeprecatedMethodInAbstractClass() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of(), ImmutableSet.of(), false, ImmutableSet.of("foo()V"));
    visitAbstractClass(filteringVisitor, "Foo");
    testIncludesMethodWithAccess(
        Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC | Opcodes.ACC_DEPRECATED);
  }

  @Test
  public void testExcludesSyntheticMethodWithDifferentDescriptor() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of(), ImmutableSet.of(), false, ImmutableSet.of("foo(I)V"));
    visitInterface(filteringVisitor, "Foo");
    filteringVisitor.visitMethod(
        Opcodes.ACC_PUBLIC | Opcodes.ACC_ABSTRACT | Opcodes.ACC_SYNTHETIC | Opcodes.ACC_DEPRECATED,
        "foo",
        "()V",
        null,
        null);
    verify(mockVisitor, never()).visitMethod(anyInt(), anyString(), anyString(), any(), any());
  }

  @Test
  public void testFindHiddenDeprecatedMethods() {
    ClassNode classNode = new ClassNode(Opcodes.ASM9);
    classNode.methods.add(
        methodWithKotlinDeprecation(
            Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC, "hidden", "()V", "HIDDEN"));
    classNode.methods.add(
        methodWithKotlinDeprecation(
            Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC, "warning", "()V", "WARNING"));
    classNode.methods.add(
        new MethodNode(Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC, "plain", "()V", null, null));
    classNode.methods.add(
        methodWithKotlinDeprecation(Opcodes.ACC_PUBLIC, "notSynthetic", "()V", "HIDDEN"));

    Set<String> hidden = AbiFilteringClassVisitor.findHiddenDeprecatedMethods(classNode);

    assertEquals(ImmutableSet.of("hidden()V"), hidden);
  }

  @Test
  public void testFindHiddenDeprecatedMethodsIncludesInvisibleAnnotation() {
    ClassNode classNode = new ClassNode(Opcodes.ASM9);
    MethodNode method =
        new MethodNode(Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC, "hidden", "()V", null, null);
    AnnotationNode annotation = new AnnotationNode("Lkotlin/Deprecated;");
    annotation.values =
        Arrays.asList(
            "message", "old", "level", new String[] {"Lkotlin/DeprecationLevel;", "HIDDEN"});
    method.invisibleAnnotations = Arrays.asList(annotation);
    classNode.methods.add(method);

    assertTrue(
        AbiFilteringClassVisitor.findHiddenDeprecatedMethods(classNode).contains("hidden()V"));
  }

  private static MethodNode methodWithKotlinDeprecation(
      int access, String name, String desc, String level) {
    MethodNode method = new MethodNode(access, name, desc, null, null);
    AnnotationNode annotation = new AnnotationNode("Lkotlin/Deprecated;");
    annotation.values =
        Arrays.asList("message", "old", "level", new String[] {"Lkotlin/DeprecationLevel;", level});
    method.visibleAnnotations = Arrays.asList(annotation);
    return method;
  }

  @Test
  public void testNotConfusedByOtherMethodAccessFlagsIncluding() {
    testIncludesMethodWithAccess(
        Opcodes.ACC_PUBLIC | Opcodes.ACC_ABSTRACT | Opcodes.ACC_SYNCHRONIZED);
  }

  @Test
  public void testNotConfusedByOtherMethodAccessFlagsExcluding() {
    testExcludesMethodWithAccess(
        Opcodes.ACC_PRIVATE | Opcodes.ACC_ABSTRACT | Opcodes.ACC_SYNCHRONIZED);
  }

  @Test
  public void testExcludesStaticInitializers() {
    testExcludesMethodWithAccess(Opcodes.ACC_STATIC, "<clinit>");
  }

  @Test
  public void testAlwaysVisitsClassNode() {
    visitClass(filteringVisitor, "Foo");
    verifyVisitClass(mockVisitor, "Foo");
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testIncludesInnerClassEntryForClassItself() {
    visitClass(filteringVisitor, "Foo$Inner");
    filteringVisitor.visitInnerClass("Foo$Inner", "Foo", "Inner", Opcodes.ACC_PUBLIC);

    verifyVisitClass(mockVisitor, "Foo$Inner");
    verify(mockVisitor).visitInnerClass("Foo$Inner", "Foo", "Inner", Opcodes.ACC_PUBLIC);
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testIncludesInnerClassEntryForInnerClass() {
    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass("Foo$Inner", "Foo", "Inner", Opcodes.ACC_PUBLIC);

    verifyVisitClass(mockVisitor, "Foo");
    verify(mockVisitor).visitInnerClass("Foo$Inner", "Foo", "Inner", Opcodes.ACC_PUBLIC);
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testIncludesInnerClassEntryForReferencedOtherClassInnerClass() {
    filteringVisitor =
        new AbiFilteringClassVisitor(
            mockVisitor, ImmutableList.of(), ImmutableSet.of("Bar$Inner"), false);

    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass("Bar$Inner", "Bar", "Inner", Opcodes.ACC_PUBLIC);

    verifyVisitClass(mockVisitor, "Foo");
    verify(mockVisitor).visitInnerClass("Bar$Inner", "Bar", "Inner", Opcodes.ACC_PUBLIC);
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testExcludesInnerClassEntryForUnreferencedOtherClassInnerClass() {
    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass("Bar$Inner", "Bar", "Inner", Opcodes.ACC_PUBLIC);

    verifyVisitClass(mockVisitor, "Foo");
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testIncludesPrivateInnerClassesForNow() {
    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass("Foo$Inner", "Foo", "Inner", Opcodes.ACC_PRIVATE);

    verifyVisitClass(mockVisitor, "Foo");
    verify(mockVisitor).visitInnerClass("Foo$Inner", "Foo", "Inner", Opcodes.ACC_PRIVATE);
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testExcludesSyntheticInnerClasses() {
    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass(
        "Foo$Inner", "Foo", "Inner", Opcodes.ACC_PUBLIC | Opcodes.ACC_SYNTHETIC);

    verifyVisitClass(mockVisitor, "Foo");
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testExcludesAnonymousInnerClasses() {
    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass("Foo$1", null, null, 0);

    verifyVisitClass(mockVisitor, "Foo");
    verifyNoMoreInteractions(mockVisitor);
  }

  @Test
  public void testExcludesLocalClasses() {
    visitClass(filteringVisitor, "Foo");
    filteringVisitor.visitInnerClass("Foo$1Bar", null, "Bar", 0);

    verifyVisitClass(mockVisitor, "Foo");
    verifyNoMoreInteractions(mockVisitor);
  }

  private static void visitClass(ClassVisitor cv, String name) {
    cv.visit(Opcodes.V1_8, Opcodes.ACC_PUBLIC, name, null, "java/lang/Object", null);
  }

  private static void visitInterface(ClassVisitor cv, String name) {
    cv.visit(
        Opcodes.V1_8,
        Opcodes.ACC_PUBLIC | Opcodes.ACC_INTERFACE | Opcodes.ACC_ABSTRACT,
        name,
        null,
        "java/lang/Object",
        null);
  }

  private static void visitAbstractClass(ClassVisitor cv, String name) {
    cv.visit(
        Opcodes.V1_8,
        Opcodes.ACC_PUBLIC | Opcodes.ACC_ABSTRACT,
        name,
        null,
        "java/lang/Object",
        null);
  }

  private static void verifyVisitClass(ClassVisitor cv, String name) {
    verify(cv).visit(Opcodes.V1_8, Opcodes.ACC_PUBLIC, name, null, "java/lang/Object", null);
  }

  private void testExcludesFieldWithAccess(int access) {
    testFieldWithAccess(access, false);
  }

  private void testIncludesFieldWithAccess(int access) {
    testFieldWithAccess(access, true);
  }

  private void testFieldWithAccess(int access, boolean shouldInclude) {
    filteringVisitor.visitField(access, "Foo", "I", null, null);
    if (shouldInclude) {
      verify(mockVisitor).visitField(access, "Foo", "I", null, null);
    }
  }

  private void testExcludesMethodWithAccess(int access) {
    testExcludesMethodWithAccess(access, "foo");
  }

  private void testIncludesMethodWithAccess(int access) {
    testIncludesMethodWithAccess(access, "foo");
  }

  private void testExcludesMethodWithAccess(int access, String name) {
    testMethodWithAccess(access, name, false);
  }

  private void testIncludesMethodWithAccess(int access, String name) {
    testMethodWithAccess(access, name, true);
  }

  private void testMethodWithAccess(int access, String name, boolean shouldInclude) {
    filteringVisitor.visitMethod(access, name, "()V", null, null);
    if (shouldInclude) {
      verify(mockVisitor).visitMethod(access, name, "()V", null, null);
    }
  }
}
