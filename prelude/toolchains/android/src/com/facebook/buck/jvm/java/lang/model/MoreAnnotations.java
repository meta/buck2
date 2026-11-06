/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.lang.model;

import com.facebook.infer.annotation.Nullsafe;
import com.sun.tools.javac.code.Symbol;
import java.util.List;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.Element;

/** Helper methods for working with annotations. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class MoreAnnotations {

  /**
   * Get all type annotations attached to an element.
   *
   * <p>NOTE: this method should not be used outside of {@code ElementsExtendedImpl}. It uses
   * internal compiler APIs to fetch type annotations of javac Elements <b>in the right order.</b>
   * This code should be compatible with up to JDK 19, but further compiler upgrades may require
   * revising this code.
   */
  public static List<? extends AnnotationMirror> getAllTypeAnnotations(Element element) {
    if (element instanceof Symbol) {
      Symbol sym = (Symbol) element;
      return sym.getRawTypeAttributes();
    }

    return List.of();
  }
}
