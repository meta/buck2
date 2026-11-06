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
import java.util.List;
import javax.lang.model.element.Element;

/** Interface for {@link Element}s that are implemented by something other than javac itself. */
@Nullsafe(Nullsafe.Mode.LOCAL)
interface ArtificialElement extends Element {
  @Override
  @Nullable
  ArtificialElement getEnclosingElement();

  @Override
  List<? extends Element> getEnclosedElements();

  void addEnclosedElement(Element element);
}
