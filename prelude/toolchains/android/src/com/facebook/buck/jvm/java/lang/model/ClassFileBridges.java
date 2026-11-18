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
import com.sun.tools.javac.code.Flags;
import com.sun.tools.javac.code.Scope;
import com.sun.tools.javac.code.Symbol;
import java.io.IOException;
import java.net.URI;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.TypeElement;
import javax.tools.JavaFileObject;
import org.jspecify.annotations.Nullable;

/**
 * The bridge methods a class file declares. javac decides whether a subclass needs a bridge from
 * the members of its superclasses' class files ({@code TransTypes.binaryImplementation}), bridges
 * included, so a class compiled against a superclass whose class file lacks a bridge gets its own
 * copy. Uses internal compiler APIs: {@link javax.lang.model.element.Element#getEnclosedElements}
 * leaves synthetic members out.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ClassFileBridges {
  private ClassFileBridges() {}

  /**
   * The bridge methods the class file of {@code type} declares, by name, or null when {@code type}
   * is not read from a class file or is read from one of {@code localClassDirectories}.
   */
  public static @Nullable Map<String, List<ExecutableElement>> getDeclaredBridges(
      TypeElement type, List<Path> localClassDirectories) {
    if (!(type instanceof Symbol.ClassSymbol)) {
      return null;
    }
    Symbol.ClassSymbol symbol = (Symbol.ClassSymbol) type;
    Iterable<Symbol> members;
    try {
      // Completes the symbol, which sets its class file.
      Scope scope = symbol.members();
      members = scope == null ? null : scope.getSymbols();
    } catch (Symbol.CompletionFailure e) {
      return null;
    }
    if (members == null) {
      return null;
    }
    JavaFileObject classfile = symbol.classfile;
    if (classfile == null
        || classfile.getKind() != JavaFileObject.Kind.CLASS
        || isInLocalClassDirectory(classfile, localClassDirectories)) {
      return null;
    }
    Map<String, List<ExecutableElement>> result = new HashMap<>();
    for (Symbol member : members) {
      if (member instanceof Symbol.MethodSymbol && (member.flags() & Flags.BRIDGE) != 0) {
        ExecutableElement method = (Symbol.MethodSymbol) member;
        result
            .computeIfAbsent(method.getSimpleName().toString(), name -> new ArrayList<>())
            .add(method);
      }
    }
    return result;
  }

  private static boolean isInLocalClassDirectory(
      JavaFileObject classfile, List<Path> localClassDirectories) {
    URI uri = classfile.toUri();
    if (localClassDirectories.isEmpty() || !"file".equals(uri.getScheme())) {
      return false;
    }
    Path path = toCanonicalPath(Paths.get(uri));
    for (Path directory : localClassDirectories) {
      if (path.startsWith(toCanonicalPath(directory))) {
        return true;
      }
    }
    return false;
  }

  // javac reports class files by canonical path while the configured directories may spell
  // them through symlinks: compare canonical paths so the two spellings match.
  private static Path toCanonicalPath(Path path) {
    try {
      return path.toRealPath();
    } catch (IOException e) {
      return path.toAbsolutePath().normalize();
    }
  }
}
