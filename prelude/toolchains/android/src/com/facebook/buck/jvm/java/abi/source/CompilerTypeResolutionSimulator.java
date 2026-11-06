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

import com.facebook.buck.jvm.java.lang.model.MoreElements;
import com.facebook.buck.util.liteinfersupport.PropagatesNullable;
import com.sun.source.tree.AnnotatedTypeTree;
import com.sun.source.tree.IdentifierTree;
import com.sun.source.tree.MemberSelectTree;
import com.sun.source.tree.ParameterizedTypeTree;
import com.sun.source.util.TreePath;
import com.sun.source.util.TreePathScanner;
import com.sun.source.util.Trees;
import java.util.HashSet;
import java.util.Set;
import javax.lang.model.element.NestingKind;
import javax.lang.model.element.TypeElement;
import org.jspecify.annotations.Nullable;

/**
 * Simulates how {@code javac} resolves types, at least so far as to determine whether a given
 * resolution would succeed or not under source-only ABI.
 */
class CompilerTypeResolutionSimulator {

  private final Trees trees;
  private final FileManagerSimulator fileManager;
  private final CompletionSimulator completer;

  @Nullable private ImportsTracker imports = null;

  public CompilerTypeResolutionSimulator(Trees trees, FileManagerSimulator fileManager) {
    this.trees = trees;
    this.fileManager = fileManager;
    this.completer = new CompletionSimulator(fileManager);
  }

  public void setImports(ImportsTracker imports) {
    this.imports = imports;
  }

  public ResolvedType resolve(@PropagatesNullable TreePath referencingPath) {
    if (referencingPath == null) {
      return null;
    }

    return new TreePathScanner<ResolvedType, Void>() {
      @Override
      public ResolvedType visitAnnotatedType(AnnotatedTypeTree node, Void aVoid) {
        return scan(node.getUnderlyingType(), aVoid);
      }

      @Override
      public ResolvedType visitParameterizedType(ParameterizedTypeTree node, Void aVoid) {
        return scan(node.getType(), aVoid);
      }

      @Override
      public ResolvedType visitIdentifier(IdentifierTree node, Void aVoid) {
        TypeElement typeElement = (TypeElement) trees.getElement(getCurrentPath());
        return newResolvedType(typeElement);
      }

      private ResolvedType newResolvedType(TypeElement typeElement) {
        ResolvedTypeKind kind = ResolvedTypeKind.RESOLVED_TYPE;
        Set<String> missingDependencies = new HashSet<>();

        if (!fileManager.typeWillBeAvailable(typeElement)) {
          kind = ResolvedTypeKind.ERROR_TYPE;
          missingDependencies.add(fileManager.getOwningTarget(typeElement));
        }

        return new ResolvedType(kind, typeElement, missingDependencies);
      }

      @Override
      public ResolvedType visitMemberSelect(MemberSelectTree node, Void aVoid) {
        TypeElement type = (TypeElement) trees.getElement(getCurrentPath());
        return new Object() {
          private ResolvedTypeKind kind = ResolvedTypeKind.RESOLVED_TYPE;
          private final Set<String> missingDependencies = new HashSet<>();

          {
            CompletionSimulator.CompletedType completedType = completer.complete(type, false);
            if (completedType.kind != CompletedTypeKind.COMPLETED_TYPE) {
              kind = kind.merge(ResolvedTypeKind.ERROR_TYPE);
              missingDependencies.addAll(completedType.getMissingDependencies());
            }
          }

          public ResolvedType resolve() {
            if (type.getNestingKind() != NestingKind.TOP_LEVEL) {
              ResolvedType referencedEnclosingType = scan(node.getExpression(), aVoid);
              missingDependencies.addAll(referencedEnclosingType.missingDependencies);
              resolveMemberType(type, referencedEnclosingType.type);

              if (referencedEnclosingType.kind == ResolvedTypeKind.ERROR_TYPE) {
                // If the originally referenced enclosing type was not found, the compiler would
                // actually have stopped right there. We kept going because we wanted to get all
                // the things that would be necessary to convert that error type into a resolved
                // type
                kind = ResolvedTypeKind.ERROR_TYPE;
              }
            }
            return new ResolvedType(kind, type, missingDependencies);
          }

          private boolean resolveMemberType(
              TypeElement type, @Nullable TypeElement referencedEnclosingType) {
            if (referencedEnclosingType == null) {
              // Ran out of places to look
              return false;
            }

            if (type.getEnclosingElement() == referencedEnclosingType) {
              // Found it!
              return true;
            }

            TypeElement superclass = MoreElements.getSuperclass(referencedEnclosingType);
            markAsCrashIfNotResolvable(superclass);
            if (resolveMemberType(type, superclass)) {
              return true;
            }

            Iterable<TypeElement> interfaces =
                MoreElements.getInterfaces(referencedEnclosingType)::iterator;
            for (TypeElement interfaceElement : interfaces) {
              markAsCrashIfNotResolvable(interfaceElement);
              if (resolveMemberType(type, interfaceElement)) {
                return true;
              }
            }

            return false;
          }

          private void markAsCrashIfNotResolvable(@Nullable TypeElement type) {
            if (type == null) {
              return;
            }

            CompletionSimulator.CompletedType completedType = completer.complete(type, false);
            if (completedType.kind != CompletedTypeKind.COMPLETED_TYPE) {
              kind = kind.merge(ResolvedTypeKind.CRASH);
              missingDependencies.addAll(completedType.getMissingDependencies());
            }
          }
        }.resolve();
      }
    }.scan(referencingPath, null);
  }
}
