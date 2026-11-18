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
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.signature.SignatureVisitor;

@Nullsafe(Nullsafe.Mode.LOCAL)
class SourceAbiCompatibleSignatureVisitor extends SignatureVisitorWrapper {
  public SourceAbiCompatibleSignatureVisitor(SignatureVisitor sv) {
    super(Opcodes.ASM9, sv);
  }

  @Override
  public SignatureVisitor visitClassBound() {
    // Because we can't know whether inferred types are classes or interfaces, we treat everything
    // as interfaces.
    return visitInterfaceBound();
  }
}
