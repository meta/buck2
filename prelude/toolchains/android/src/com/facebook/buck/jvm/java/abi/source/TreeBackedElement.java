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
import com.sun.source.tree.Tree;
import com.sun.source.util.TreePath;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import javax.lang.model.element.Element;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import javax.lang.model.type.TypeMirror;

/**
 * An implementation of {@link Element} that uses only the information available from a {@link
 * com.sun.source.tree.Tree}. This results in an incomplete implementation; see documentation for
 * individual methods and {@link com.facebook.buck.jvm.java.abi.source} for more information.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class TreeBackedElement extends TreeBackedAnnotatedConstruct
    implements ArtificialElement {
  private final Element underlyingElement;
  @Nullable private final TreeBackedElement enclosingElement;
  private final List<Element> enclosedElements = new ArrayList<>();
  @Nullable private final PostEnterCanonicalizer canonicalizer;

  @Nullable private final TreePath treePath;

  public TreeBackedElement(
      Element underlyingElement,
      @Nullable TreeBackedElement enclosingElement,
      @Nullable TreePath treePath,
      @Nullable PostEnterCanonicalizer canonicalizer) {
    super(underlyingElement);
    this.underlyingElement = underlyingElement;
    this.enclosingElement = enclosingElement;
    // Some element types don't appear as members of enclosingElement.getEnclosedElements, so
    // it's up to each subtype's constructor to decide whether to add itself or not.
    this.treePath = treePath;
    this.canonicalizer = canonicalizer;
  }

  public Element getUnderlyingElement() {
    return underlyingElement;
  }

  protected final PostEnterCanonicalizer getCanonicalizer() {
    return Objects.requireNonNull(canonicalizer);
  }

  public abstract void complete();

  @Nullable
  /* package */ TreePath getTreePath() {
    return treePath;
  }

  @Nullable
  /* package */ Tree getTree() {
    return treePath == null ? null : treePath.getLeaf();
  }

  @Override
  public ElementKind getKind() {
    return underlyingElement.getKind();
  }

  @Override
  public Set<Modifier> getModifiers() {
    return underlyingElement.getModifiers();
  }

  @Override
  public Name getSimpleName() {
    return underlyingElement.getSimpleName();
  }

  @Override
  @Nullable
  public TreeBackedElement getEnclosingElement() {
    return enclosingElement;
  }

  @Override
  public List<? extends Element> getEnclosedElements() {
    return Collections.unmodifiableList(enclosedElements);
  }

  @Override
  public void addEnclosedElement(Element element) {
    enclosedElements.add(element);
  }

  @Override
  public abstract TypeMirror asType();

  @Override
  public String toString() {
    return underlyingElement.toString();
  }
}
