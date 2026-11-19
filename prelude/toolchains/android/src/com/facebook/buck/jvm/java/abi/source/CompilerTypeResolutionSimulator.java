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
import com.facebook.infer.annotation.Nullsafe;
import com.facebook.infer.annotation.PropagatesNullable;
import com.sun.source.tree.AnnotatedTypeTree;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.IdentifierTree;
import com.sun.source.tree.MemberSelectTree;
import com.sun.source.tree.ParameterizedTypeTree;
import com.sun.source.util.TreePath;
import com.sun.source.util.TreePathScanner;
import com.sun.source.util.Trees;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import javax.lang.model.element.NestingKind;
import javax.lang.model.element.TypeElement;
import org.jspecify.annotations.Nullable;

/**
 * Simulates how {@code javac} resolves types, at least so far as to determine whether a given
 * resolution would succeed or not under source-only ABI.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
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
        TypeElement typeElement =
            (TypeElement) Objects.requireNonNull(trees.getElement(getCurrentPath()));
        ResolvedType resolvedType = newResolvedType(typeElement);
        if (typeElement.getNestingKind() == NestingKind.MEMBER) {
          Set<String> missingSupertypes = missingMemberSupertypes(typeElement);
          if (!missingSupertypes.isEmpty()) {
            return new ResolvedType(
                ResolvedTypeKind.ERROR_TYPE,
                typeElement,
                resolvedType.missingDependencies,
                missingSupertypes);
          }
        }
        return resolvedType;
      }

      private Set<String> missingMemberSupertypes(TypeElement member) {
        Set<String> bestPath = null;
        TypeElement scope = enclosingMemberScope();
        while (scope != null) {
          if (bestPath != null
              && scope.getTypeParameters().stream()
                  .anyMatch(
                      parameter ->
                          parameter.getSimpleName().contentEquals(member.getSimpleName()))) {
            return bestPath;
          }
          if (hasAvailableOtherMemberType(scope, member)) {
            return missingOrEmpty(bestPath);
          }
          bestPath = shorterPath(bestPath, missingPathTo(scope, member));
          if (bestPath != null && bestPath.isEmpty()) {
            return bestPath;
          }
          if (!(scope.getEnclosingElement() instanceof TypeElement)) {
            break;
          }
          scope = (TypeElement) scope.getEnclosingElement();
        }
        return missingImportedMemberSupertypes(member, bestPath);
      }

      @Nullable
      private TypeElement enclosingMemberScope() {
        TypeElement scope =
            Objects.requireNonNull(trees.getScope(getCurrentPath())).getEnclosingClass();
        if (scope == null) {
          return null;
        }
        TreePath child = getCurrentPath();
        for (TreePath parent = child.getParentPath();
            parent != null;
            parent = parent.getParentPath()) {
          if (parent.getLeaf() instanceof ClassTree) {
            ClassTree declaration = (ClassTree) parent.getLeaf();
            // getScope includes the declared class even in its header.
            if (scope.equals(trees.getElement(parent))
                && !declaration.getMembers().contains(child.getLeaf())) {
              return scope.getEnclosingElement() instanceof TypeElement
                  ? (TypeElement) scope.getEnclosingElement()
                  : null;
            }
            break;
          }
          child = parent;
        }
        return scope;
      }

      private Set<String> missingImportedMemberSupertypes(
          TypeElement member, @Nullable Set<String> bestPath) {
        if (imports != null) {
          TypeElement singleImport = imports.getSingleTypeImport(member.getSimpleName());
          if (singleImport != null) {
            return singleImport.equals(member) ? Set.of() : missingOrEmpty(bestPath);
          }
          Set<TypeElement> namedContainers =
              imports.getSingleStaticTypeImportContainers(member.getSimpleName());
          if (!namedContainers.isEmpty()) {
            for (TypeElement container : namedContainers) {
              if (hasAvailableOtherImportedMemberType(container, member)) {
                return missingOrEmpty(bestPath);
              }
            }
            for (TypeElement container : namedContainers) {
              bestPath = shorterPath(bestPath, missingPathTo(container, member));
            }
            return missingOrEmpty(bestPath);
          }
          TypeElement packageType = imports.getTopLevelTypeInPackage(member.getSimpleName());
          if (packageType != null && fileManager.typeWillBeAvailable(packageType)) {
            return missingOrEmpty(bestPath);
          }
          for (TypeElement other : imports.getOnDemandTypes(member.getSimpleName())) {
            if (!other.equals(member) && fileManager.typeWillBeAvailable(other)) {
              return missingOrEmpty(bestPath);
            }
          }
          for (TypeElement container :
              imports.getOnDemandStaticTypeImportContainers(member.getSimpleName())) {
            if (hasAvailableOtherImportedMemberType(container, member)) {
              return missingOrEmpty(bestPath);
            }
          }
          if (imports.isOnDemandImported(member)) {
            return Set.of();
          }
          for (TypeElement container : imports.getOnDemandStaticImportContainers(member)) {
            bestPath = shorterPath(bestPath, missingPathTo(container, member));
          }
        }
        return missingOrEmpty(bestPath);
      }

      private boolean hasAvailableOtherMemberType(TypeElement scope, TypeElement member) {
        return imports != null
            && hasAvailableMember(scope, imports.getOtherMemberTypesWithSameName(scope, member));
      }

      private boolean hasAvailableOtherImportedMemberType(TypeElement scope, TypeElement member) {
        return imports != null
            && hasAvailableMember(
                scope, imports.getOtherStaticallyImportedMemberTypes(scope, member));
      }

      private boolean hasAvailableMember(TypeElement scope, Set<TypeElement> members) {
        for (TypeElement member : members) {
          Set<String> path = missingPathTo(scope, member);
          if (path != null && path.isEmpty()) {
            return true;
          }
        }
        return false;
      }

      @Nullable
      private Set<String> shorterPath(@Nullable Set<String> first, @Nullable Set<String> second) {
        return first == null || (second != null && second.size() < first.size()) ? second : first;
      }

      private Set<String> missingOrEmpty(@Nullable Set<String> missing) {
        return missing == null ? Set.of() : missing;
      }

      @Nullable
      private Set<String> missingPathTo(TypeElement start, TypeElement member) {
        Map<TypeElement, Set<String>> paths = new HashMap<>();
        Deque<TypeElement> pending = new ArrayDeque<>();
        paths.put(
            start,
            fileManager.typeWillBeAvailable(start)
                ? Set.of()
                : Set.of(start.getQualifiedName().toString()));
        pending.add(start);
        while (!pending.isEmpty()) {
          TypeElement current = pending.removeFirst();
          if (imports != null && !imports.exposesMember(current, member)) {
            continue;
          }
          Set<String> missing = paths.get(current);
          if (current == member.getEnclosingElement()) {
            return missing;
          }
          List<TypeElement> parents = new ArrayList<>();
          TypeElement superclass = MoreElements.getSuperclass(current);
          if (superclass != null) {
            parents.add(superclass);
          }
          MoreElements.getInterfaces(current).forEach(parents::add);
          for (TypeElement parent : parents) {
            boolean available = fileManager.typeWillBeAvailable(parent);
            Set<String> next = new HashSet<>(Objects.requireNonNull(missing));
            if (!available) {
              next.add(parent.getQualifiedName().toString());
            }
            Set<String> previous = paths.get(parent);
            if (previous != null && previous.size() <= next.size()) {
              continue;
            }
            paths.put(parent, next);
            if (available) {
              pending.addFirst(parent);
            } else {
              pending.addLast(parent);
            }
          }
        }
        return null;
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
        TypeElement type = (TypeElement) Objects.requireNonNull(trees.getElement(getCurrentPath()));
        return new Object() {
          private ResolvedTypeKind kind = ResolvedTypeKind.RESOLVED_TYPE;
          private final Set<String> missingDependencies = new HashSet<>();
          private final Set<String> missingMemberSupertypes = new HashSet<>();

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
              missingMemberSupertypes.addAll(referencedEnclosingType.missingMemberSupertypes);
              TypeElement qualifier = referencedEnclosingType.type;
              if (qualifier != null && type.getEnclosingElement() != qualifier) {
                Set<String> missing = missingPathTo(qualifier, type);
                if (missing != null) {
                  missingMemberSupertypes.addAll(missing);
                }
              }
              resolveMemberType(type, qualifier);

              if (referencedEnclosingType.kind == ResolvedTypeKind.ERROR_TYPE) {
                // If the originally referenced enclosing type was not found, the compiler would
                // actually have stopped right there. We kept going because we wanted to get all
                // the things that would be necessary to convert that error type into a resolved
                // type
                kind = ResolvedTypeKind.ERROR_TYPE;
              }
            }
            if (!missingMemberSupertypes.isEmpty()) {
              kind = kind.merge(ResolvedTypeKind.ERROR_TYPE);
            }
            return new ResolvedType(kind, type, missingDependencies, missingMemberSupertypes);
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
