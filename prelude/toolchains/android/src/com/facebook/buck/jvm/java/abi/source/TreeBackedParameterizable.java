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
import com.sun.source.util.TreePath;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import javax.lang.model.element.Element;

@Nullsafe(Nullsafe.Mode.LOCAL)
abstract class TreeBackedParameterizable extends TreeBackedElement
    implements ArtificialParameterizable {
  private final List<TreeBackedTypeParameterElement> typeParameters = new ArrayList<>();

  public TreeBackedParameterizable(
      Element underlyingElement,
      TreeBackedElement enclosingElement,
      @Nullable TreePath treePath,
      PostEnterCanonicalizer canonicalizer) {
    super(underlyingElement, enclosingElement, treePath, canonicalizer);
  }

  /* package */ void addTypeParameter(TreeBackedTypeParameterElement typeParameter) {
    typeParameters.add(typeParameter);
  }

  @Override
  public List<TreeBackedTypeParameterElement> getTypeParameters() {
    return Collections.unmodifiableList(typeParameters);
  }
}
