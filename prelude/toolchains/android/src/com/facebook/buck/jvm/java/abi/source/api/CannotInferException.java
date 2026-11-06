/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source.api;

import com.facebook.infer.annotation.Nullsafe;
import javax.lang.model.element.Element;
import javax.lang.model.type.TypeMirror;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class CannotInferException extends RuntimeException {
  public CannotInferException(String what, Element owner) {
    super(formatMessage(what, owner));
  }

  public CannotInferException(String what, TypeMirror owner) {
    super(formatMessage(what, owner));
  }

  private static String formatMessage(String what, Object owner) {
    return String.format(
        "Buck had to infer the existence of %1$s for source-only ABI generation, and thus cannot"
            + " know the %2$s of the type.\n"
            + "One of three things is happening:\n"
            + "  1. The BUCK file is missing a dependency for %1$s\n"
            + "  2. The module containing %1$s needs to be marked with required_for_source_only_abi"
            + " or referenced as a source_only_abi_dep\n"
            + "  3. An annotation processor is accessing a type that it shouldn't be",
        owner, what);
  }
}
