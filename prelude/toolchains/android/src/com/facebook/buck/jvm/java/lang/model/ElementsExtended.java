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
import java.nio.file.Path;
import java.util.List;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.Element;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.TypeElement;
import javax.lang.model.util.Elements;
import org.jspecify.annotations.Nullable;

/**
 * Wraps and extends {@link javax.lang.model.util.Elements} with methods that cannot be added as
 * pure extension methods on {@link MoreElements} because they require per-instance state.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface ElementsExtended extends Elements {

  List<ExecutableElement> getDeclaredMethods(TypeElement owner, CharSequence name);

  List<ExecutableElement> getAllMethods(TypeElement owner, CharSequence name);

  List<BridgeMethod> getBridgeMethods(TypeElement owner, CharSequence name);

  List<BridgeMethod> getAllBridgeMethods(TypeElement type);

  @Nullable ExecutableElement getImplementation(ExecutableElement method, TypeElement inType);

  @Nullable TypeElement getBinaryImplementationOwner(ExecutableElement method, TypeElement inType);

  boolean isCompiledInCurrentRun(Element element);

  /**
   * Directories of class files this module compiles in another step, such as a mixed module's own
   * Kotlin classes. Their bridges are computed, not read from their class files: source-only ABI
   * generation reads stubs there while the library compilation reads the compiled classes.
   */
  default void setLocalClassDirectories(List<Path> directories) {}

  /**
   * Return type annotations present on an element.
   *
   * <p>Examples:
   *
   * <ul>
   *   <li>For methods this means type annotations present on method's type parameters and types of
   *       formal parameters.
   *   <li>For classes it's type annotations on a class and its superclasses and superinterfaces.
   *   <li>For fields it's type annotations on a field's type.
   * </ul>
   */
  List<? extends AnnotationMirror> getAllTypeAnnotations(Element element);
}
