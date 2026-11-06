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
import com.sun.source.tree.TypeParameterTree;
import com.sun.source.util.TreePath;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import javax.lang.model.element.Element;
import javax.lang.model.element.ElementVisitor;
import javax.lang.model.element.TypeParameterElement;
import javax.lang.model.type.TypeMirror;

/**
 * An implementation of {@link TypeParameterElement} that uses only the information available from a
 * {@link TypeParameterTree}. This results in an incomplete implementation; see documentation for
 * individual methods and {@link com.facebook.buck.jvm.java.abi.source} for more information.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class TreeBackedTypeParameterElement extends TreeBackedElement
    implements ArtificialTypeParameterElement {
  private final TypeParameterElement underlyingElement;
  private final TypeParameterTree tree;
  private final StandaloneTypeVariable typeVar;
  @Nullable private List<TypeMirror> bounds;

  public TreeBackedTypeParameterElement(
      TreeBackedTypes types,
      TypeParameterElement underlyingElement,
      TreePath treePath,
      TreeBackedElement enclosingElement,
      PostEnterCanonicalizer canonicalizer) {
    super(underlyingElement, enclosingElement, treePath, canonicalizer);
    this.underlyingElement = underlyingElement;
    this.tree = (TypeParameterTree) treePath.getLeaf();
    typeVar = new StandaloneTypeVariable(types, this);

    // In javac's implementation, enclosingElement does not have type parameters in the return
    // value of getEnclosedElements
  }

  @Override
  public void complete() {
    getBounds();
  }

  @Override
  public List<? extends ArtificialElement> getEnclosedElements() {
    return Collections.emptyList();
  }

  @Override
  public StandaloneTypeVariable asType() {
    return typeVar;
  }

  @Override
  public Element getGenericElement() {
    // Our constructor does not allow null enclosing elements for this element type
    return Objects.requireNonNull(getEnclosingElement());
  }

  @Override
  public List<? extends TypeMirror> getBounds() {
    if (bounds == null) {
      bounds =
          Collections.unmodifiableList(
              getCanonicalizer()
                  .getCanonicalTypes(
                      underlyingElement.getBounds(),
                      getTreePath(),
                      tree == null ? null : tree.getBounds()));
    }

    return bounds;
  }

  @Override
  public <R, P> R accept(ElementVisitor<R, P> v, @Nullable P p) {
    return v.visitTypeParameter(this, p);
  }
}
