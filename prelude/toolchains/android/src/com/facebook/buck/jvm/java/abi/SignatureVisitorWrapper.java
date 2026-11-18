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
import org.objectweb.asm.signature.SignatureVisitor;

/**
 * A {@link SignatureVisitor} that chains to another one. Most of the other visitors in ASM can
 * already do this; {@link SignatureVisitor} is an exception for some reason.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class SignatureVisitorWrapper extends SignatureVisitor {
  private final SignatureVisitor sv;

  public SignatureVisitorWrapper(int api, SignatureVisitor sv) {
    super(api);
    this.sv = sv;
  }

  @Override
  public void visitFormalTypeParameter(String name) {
    sv.visitFormalTypeParameter(name);
  }

  @Override
  public SignatureVisitor visitClassBound() {
    return sv.visitClassBound();
  }

  @Override
  public SignatureVisitor visitInterfaceBound() {
    return sv.visitInterfaceBound();
  }

  @Override
  public SignatureVisitor visitSuperclass() {
    return sv.visitSuperclass();
  }

  @Override
  public SignatureVisitor visitInterface() {
    return sv.visitInterface();
  }

  @Override
  public SignatureVisitor visitParameterType() {
    return sv.visitParameterType();
  }

  @Override
  public SignatureVisitor visitReturnType() {
    return sv.visitReturnType();
  }

  @Override
  public SignatureVisitor visitExceptionType() {
    return sv.visitExceptionType();
  }

  @Override
  public void visitBaseType(char descriptor) {
    sv.visitBaseType(descriptor);
  }

  @Override
  public void visitTypeVariable(String name) {
    sv.visitTypeVariable(name);
  }

  @Override
  public SignatureVisitor visitArrayType() {
    return sv.visitArrayType();
  }

  @Override
  public void visitClassType(String name) {
    sv.visitClassType(name);
  }

  @Override
  public void visitInnerClassType(String name) {
    sv.visitInnerClassType(name);
  }

  @Override
  public void visitTypeArgument() {
    sv.visitTypeArgument();
  }

  @Override
  public SignatureVisitor visitTypeArgument(char wildcard) {
    return sv.visitTypeArgument(wildcard);
  }

  @Override
  public void visitEnd() {
    sv.visitEnd();
  }
}
