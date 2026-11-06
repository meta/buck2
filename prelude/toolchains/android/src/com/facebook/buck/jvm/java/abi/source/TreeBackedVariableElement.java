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

import com.facebook.buck.util.liteinfersupport.Nullable;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.VariableTree;
import com.sun.source.util.TreePath;
import java.util.Collections;
import java.util.List;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.ElementVisitor;
import javax.lang.model.element.VariableElement;
import javax.lang.model.type.TypeMirror;

@Nullsafe(Nullsafe.Mode.LOCAL)
class TreeBackedVariableElement extends TreeBackedElement implements ArtificialVariableElement {
  private final VariableElement underlyingElement;

  @Nullable private final VariableTree tree;

  @Nullable private TypeMirror type;

  TreeBackedVariableElement(
      VariableElement underlyingElement,
      TreeBackedElement enclosingElement,
      @Nullable TreePath treePath,
      PostEnterCanonicalizer canonicalizer) {
    super(underlyingElement, enclosingElement, treePath, canonicalizer);
    this.underlyingElement = underlyingElement;
    this.tree = treePath == null ? null : (VariableTree) treePath.getLeaf();
    if (underlyingElement.getKind() == ElementKind.PARAMETER) {
      ((TreeBackedExecutableElement) enclosingElement).addParameter(this);
    } else {
      enclosingElement.addEnclosedElement(this);
    }
  }

  @Override
  public void complete() {
    asType();
  }

  @Override
  public List<? extends ArtificialElement> getEnclosedElements() {
    return Collections.emptyList();
  }

  @Override
  @Nullable
  VariableTree getTree() {
    return tree;
  }

  @Override
  public TypeMirror asType() {
    if (type == null) {
      type =
          getCanonicalizer()
              .getCanonicalType(
                  underlyingElement.asType(), getTreePath(), tree == null ? null : tree.getType());
    }
    return type;
  }

  @Override
  @Nullable
  public Object getConstantValue() {
    return underlyingElement.getConstantValue();
  }

  @Override
  public <R, P> R accept(ElementVisitor<R, P> v, @Nullable P p) {
    return v.visitVariable(this, p);
  }
}
