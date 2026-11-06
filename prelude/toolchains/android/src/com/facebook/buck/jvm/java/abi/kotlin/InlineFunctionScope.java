/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.kotlin;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.Collection;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import org.objectweb.asm.tree.ClassNode;

/**
 * Kotlin has the concept of "inline functions", which are guaranteed to be inlined into their call-
 * -sites, even across JARs. To make this work, we need to retain more in the ABIs of Kotlin JARs:
 *
 * <p>- The body (bytecode) of inline functions which is what `kotlinc` bases the code it inlines
 * on. - Nested classes defined within inline functions. Rather than referring to those classes in
 * the defining JAR, they are duplicated in the calling JAR, and so their implementation details
 * cannot be stripped either.
 *
 * <p>This class keeps track of Kotlin class' inline functions, and classes that are nested inside
 * inline functions (or nested inside classes that are nested inside inline functions, and so on,
 * transitively).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class InlineFunctionScope {
  private static final String DEFAULT_METHOD_SUFFIX = "$default";
  private static final int DEFAULT_METHOD_SUFFIX_LENGTH = DEFAULT_METHOD_SUFFIX.length();

  /**
   * Maps from class names (canonical names, formatted as paths, i.e. parts separated by `/`, not
   * `.`) to sets of their inline functions.
   */
  private Map<String, Set<String>> inlineFunctionsMap = new HashMap<>();

  /** Classes within inline scopes, transitively (formatted same as [inlineFunctionsMap] keys) */
  private Set<String> nestedInlineScopes = new HashSet<>();

  /**
   * Add a new set of inline scopes, from [inlineFunctions] that are part of [clazz]. Note that if
   * {@code inlineFunctions.isEmpty()}, nothing is added, and if there are existing scopes added for
   * [clazz], those are updated (extended) to include the new inline functions.
   *
   * @param clazz Canonical class name, formatted as a path.
   * @param inlineFunctions Collection of simple method names.
   */
  public void createScopes(String clazz, Collection<String> inlineFunctions) {
    if (!inlineFunctions.isEmpty()) {
      this.inlineFunctionsMap.computeIfAbsent(clazz, c -> new HashSet<>()).addAll(inlineFunctions);
    }
  }

  /** Indicate that [clazz] was defined nested within the scope of an inline function */
  public void extendScope(String clazz) {
    nestedInlineScopes.add(clazz);
  }

  /**
   * Determine whether a class is captured by an inline scope recorded in this instance. This
   * implies all its contents should be retained in the ABI of the JAR the defines these inline
   * function scopes, and the class in question should be added to the set of scopes (but this needs
   * to be done separately -- it is not a side-effect of this method).
   *
   * @param path The class' canonical name, formatted as a path, without the ".class" prefix.
   * @param classNode ASM representation of the class referred to by [path], (no code needed).
   * @return true if and only if this class is entirely captured by the ABI associated with this set
   *     of inline function scopes.
   */
  public boolean captures(Path path, ClassNode classNode) {
    // Classes whose names contain "$sam$i" are created as wrappers when SAM interfaces are
    // implemented by Kotlin Lambdas.  This can happen within inline functions, and in those cases,
    // like other nested classes, they will be duplicated in the calling context.  But unlike other
    // nested classes, these are not recorded as inner classes in the JAR, so we need to treat them
    // specially.
    //
    // TODO This is a pessimisation: not all "$sam$i" classes need to be retained, we can do better
    //  by tracking the ones that are referenced from inline functions.
    if (path.toString().contains("$sam$i")) {
      return true;
    }

    return isCapturedByOuterScope(classNode);
  }

  /**
   * Whether {@code classNode} is nested in an inline scope owned by its outer class. Unlike {@link
   * #captures}, this ignores the ownerless $sam$i wrappers, so a true result always identifies the
   * capturing outer via {@code classNode.outerClass}.
   */
  public boolean isCapturedByOuterScope(ClassNode classNode) {
    if (classNode.outerClass == null) {
      return false;
    }

    if (nestedInlineScopes.contains(classNode.outerClass)) {
      return true;
    }

    Set<String> inlineFunctions = inlineFunctionsMap.get(classNode.outerClass);
    if (inlineFunctions == null || classNode.outerMethod == null) {
      return false;
    }

    return inlineFunctions.contains(sanitizeMethodName(classNode.outerMethod));
  }

  private static String sanitizeMethodName(String methodName) {
    if (methodName.endsWith(DEFAULT_METHOD_SUFFIX)) {
      return methodName.substring(0, methodName.length() - DEFAULT_METHOD_SUFFIX_LENGTH);
    }

    return methodName;
  }
}
