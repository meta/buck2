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

import com.facebook.infer.annotation.Nullsafe;
import java.util.Collections;
import java.util.Set;
import javax.lang.model.element.ElementKind;
import javax.lang.model.element.ElementVisitor;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import javax.lang.model.type.TypeMirror;
import org.jspecify.annotations.Nullable;

/**
 * A {@link javax.lang.model.element.PackageElement} whose existence is inferred from references to
 * it in the code.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class InferredPackageElement extends InferredElement implements ArtificialPackageElement {
  private final Name qualifiedName;
  private final TypeMirror typeMirror;

  public InferredPackageElement(Name simpleName, Name qualifiedName) {
    super(simpleName, null);
    this.qualifiedName = qualifiedName;
    typeMirror = new StandalonePackageType(this);
  }

  @Override
  public Set<Modifier> getModifiers() {
    // Packages have no modifiers, so we can implememnt this method here
    return Collections.emptySet();
  }

  @Override
  public Name getQualifiedName() {
    return qualifiedName;
  }

  @Override
  public boolean isUnnamed() {
    return qualifiedName.length() == 0;
  }

  @Override
  public TypeMirror asType() {
    return typeMirror;
  }

  @Override
  public ElementKind getKind() {
    return ElementKind.PACKAGE;
  }

  @Override
  public <R, P> R accept(ElementVisitor<R, P> v, @Nullable P p) {
    return v.visitPackage(this, p);
  }

  @Override
  public String toString() {
    return qualifiedName.toString();
  }
}
