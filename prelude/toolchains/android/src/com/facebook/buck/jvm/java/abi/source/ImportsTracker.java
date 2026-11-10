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
import com.sun.source.util.TreePath;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import javax.lang.model.element.Element;
import javax.lang.model.element.Modifier;
import javax.lang.model.element.Name;
import javax.lang.model.element.PackageElement;
import javax.lang.model.element.QualifiedNameable;
import javax.lang.model.element.TypeElement;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import org.jspecify.annotations.Nullable;

/** Simulates import resolution scopes in a source-only ABI classpath. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ImportsTracker {
  private final Map<TypeElement, TreePath> importedTypes = new HashMap<>();
  private final Map<QualifiedNameable, TreePath> importedOwners = new HashMap<>();
  private final Set<TypeElement> staticImportedOwners = new HashSet<>();
  private final Map<Name, Set<TypeElement>> staticImports = new HashMap<>();
  private final Types types;
  private final Elements elements;
  private final PackageElement enclosingPackage;

  public ImportsTracker(Elements elements, Types types, PackageElement enclosingPackage) {
    this.types = types;
    this.elements = elements;
    this.enclosingPackage = enclosingPackage;
    importMembers(Objects.requireNonNull(elements.getPackageElement("java.lang")), null);
    importMembers(enclosingPackage, null);
  }

  public void importType(TypeElement type, TreePath location) {
    importedTypes.put(type, location);
  }

  public void importMembers(QualifiedNameable typeOrPackage, @Nullable TreePath location) {
    importedOwners.put(typeOrPackage, location);
  }

  public void importStatic(TypeElement container, Name identifier) {
    staticImports.computeIfAbsent(identifier, ignored -> new HashSet<>()).add(container);
  }

  public void importStaticMembers(TypeElement container) {
    staticImportedOwners.add(container);
  }

  public boolean isStaticImported(TypeElement element) {
    return getStaticImportContainer(element) != null;
  }

  public boolean nameIsStaticImported(Name name) {
    return staticImports.containsKey(name);
  }

  @Nullable
  public TypeElement getStaticImportContainer(TypeElement element) {
    if (!element.getModifiers().contains(Modifier.STATIC)) {
      return null;
    }
    for (TypeElement container : staticImports.getOrDefault(element.getSimpleName(), Set.of())) {
      if (types.isSubtype(
          container.asType(), Objects.requireNonNull(element.getEnclosingElement()).asType())) {
        return container;
      }
    }
    return null;
  }

  public Set<TypeElement> getOnDemandStaticImportContainers(TypeElement element) {
    Set<TypeElement> containers = new HashSet<>();
    if (canStaticallyImport(element)) {
      containers.addAll(staticImportedOwners);
      containers.removeIf(owner -> !exposesMember(owner, element));
    }
    return containers;
  }

  @Nullable TypeElement getSingleTypeImport(Name name) {
    for (TypeElement type : importedTypes.keySet()) {
      if (type.getSimpleName().contentEquals(name)) {
        return type;
      }
    }
    return null;
  }

  Set<TypeElement> getSingleStaticTypeImportContainers(Name name) {
    return getStaticTypeImportContainers(staticImports.getOrDefault(name, Set.of()), name);
  }

  Set<TypeElement> getOnDemandStaticTypeImportContainers(Name name) {
    return getStaticTypeImportContainers(staticImportedOwners, name);
  }

  private Set<TypeElement> getStaticTypeImportContainers(Set<TypeElement> owners, Name name) {
    Set<TypeElement> containers = new HashSet<>();
    for (TypeElement owner : owners) {
      for (Element candidate : elements.getAllMembers(owner)) {
        if (candidate instanceof TypeElement
            && candidate.getSimpleName().contentEquals(name)
            && canStaticallyImport((TypeElement) candidate)
            && exposesMember(owner, (TypeElement) candidate)) {
          containers.add(owner);
          break;
        }
      }
    }
    return containers;
  }

  Set<TypeElement> getOtherStaticallyImportedMemberTypes(TypeElement owner, TypeElement member) {
    Set<TypeElement> candidates = getOtherMemberTypesWithSameName(owner, member);
    candidates.removeIf(other -> !canStaticallyImport(other) || !exposesMember(owner, other));
    return candidates;
  }

  Set<TypeElement> getOnDemandTypes(Name name) {
    Set<TypeElement> candidates = new HashSet<>();
    for (QualifiedNameable owner : importedOwners.keySet()) {
      if (owner instanceof PackageElement) {
        TypeElement candidate = getPackageType((PackageElement) owner, name);
        if (candidate != null && canImport(candidate)) {
          candidates.add(candidate);
        }
      } else {
        for (Element candidate : owner.getEnclosedElements()) {
          if (candidate instanceof TypeElement
              && candidate.getSimpleName().contentEquals(name)
              && canImport((TypeElement) candidate)) {
            candidates.add((TypeElement) candidate);
          }
        }
      }
    }
    return candidates;
  }

  @Nullable TypeElement getTopLevelTypeInPackage(Name name) {
    return getPackageType(enclosingPackage, name);
  }

  @Nullable
  private TypeElement getPackageType(PackageElement owner, Name name) {
    String prefix = owner.isUnnamed() ? "" : owner.getQualifiedName() + ".";
    return elements.getTypeElement(prefix + name);
  }

  private boolean canStaticallyImport(TypeElement element) {
    return element.getModifiers().contains(Modifier.STATIC) && canImport(element);
  }

  private boolean canImport(TypeElement element) {
    Set<Modifier> modifiers = element.getModifiers();
    return !modifiers.contains(Modifier.PRIVATE)
        && (modifiers.contains(Modifier.PUBLIC)
            || elements.getPackageOf(element).equals(enclosingPackage));
  }

  Set<TypeElement> getOtherMemberTypesWithSameName(TypeElement owner, TypeElement member) {
    Set<TypeElement> candidates = new HashSet<>();
    for (Element candidate : elements.getAllMembers(owner)) {
      if (candidate instanceof TypeElement
          && !candidate.equals(member)
          && candidate.getSimpleName().contentEquals(member.getSimpleName())) {
        candidates.add((TypeElement) candidate);
      }
    }
    return candidates;
  }

  boolean exposesMember(TypeElement owner, TypeElement member) {
    boolean found = false;
    for (Element candidate : elements.getAllMembers(owner)) {
      if (candidate.equals(member)) {
        found = true;
      } else if (candidate instanceof TypeElement && elements.hides(candidate, member)) {
        return false;
      }
    }
    return found;
  }

  public boolean isSingleTypeImported(Element element) {
    return getSingleTypeImportLocation(element) != null;
  }

  @Nullable
  public TreePath getSingleTypeImportLocation(Element element) {
    return importedTypes.get(element);
  }

  public boolean isOnDemandImported(Element element) {
    return getOnDemandImportLocation(element) != null;
  }

  @Nullable
  public TreePath getOnDemandImportLocation(Element element) {
    return importedOwners.get(element.getEnclosingElement());
  }

  public boolean isOnDemandStaticImported(TypeElement element) {
    return getOnDemandStaticImportContainer(element) != null;
  }

  @Nullable
  public TypeElement getOnDemandStaticImportContainer(TypeElement element) {
    if (!element.getModifiers().contains(Modifier.STATIC)) {
      return null;
    }

    TypeElement container = (TypeElement) Objects.requireNonNull(element.getEnclosingElement());
    for (TypeElement staticImportedOwner : staticImportedOwners) {
      if (types.isSubtype(staticImportedOwner.asType(), container.asType())) {
        return staticImportedOwner;
      }
    }

    return null;
  }
}
