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

import com.sun.source.tree.AnnotatedTypeTree;
import com.sun.source.tree.IdentifierTree;
import com.sun.source.tree.MemberSelectTree;
import com.sun.source.tree.ParameterizedTypeTree;
import com.sun.source.tree.Tree;
import com.sun.source.tree.Tree.Kind;
import com.sun.source.util.TreePath;
import com.sun.source.util.TreePathScanner;
import com.sun.source.util.Trees;
import java.util.Objects;
import javax.lang.model.element.Element;
import javax.lang.model.element.PackageElement;
import javax.lang.model.element.TypeElement;
import javax.lang.model.util.SimpleElementVisitor8;
import org.jspecify.annotations.Nullable;

/**
 * A {@link TreePathScanner} that forms the scaffolding of any kind of type reference resolution
 * logic. Scans a {@link TreePath} that represents a reference to a type element and
 */
abstract class TreePathScannerForTypeResolution<R, P> extends TreePathScanner<R, P> {

  private final Trees trees;

  protected TreePathScannerForTypeResolution(Trees trees) {
    this.trees = trees;
  }

  @Override
  @Nullable
  public final R visitAnnotatedType(AnnotatedTypeTree node, P p) {
    return scan(node.getUnderlyingType(), p);
  }

  @Override
  @Nullable
  public final R visitParameterizedType(ParameterizedTypeTree node, P p) {
    return scan(node.getType(), p);
  }

  @Override
  @Nullable
  public final R visitMemberSelect(MemberSelectTree node, P p) {
    TreePath referencingPath = getCurrentPath();
    Element referencedElement = Objects.requireNonNull(trees.getElement(referencingPath));
    return resolveElement(referencingPath, referencedElement, p);
  }

  @Override
  @Nullable
  public final R visitIdentifier(IdentifierTree node, P p) {
    TreePath referencingPath = getCurrentPath();
    Element referencedElement = Objects.requireNonNull(trees.getElement(referencingPath));
    return resolveElement(referencingPath, referencedElement, p);
  }

  @Nullable
  protected final R resolveEnclosingElement(P p) {
    Tree leaf = getCurrentPath().getLeaf();
    if (leaf.getKind() == Kind.MEMBER_SELECT) {
      MemberSelectTree memberSelect = (MemberSelectTree) leaf;
      return super.visitMemberSelect(memberSelect, p);
    } else {
      return null;
    }
  }

  @Nullable
  private R resolveElement(TreePath referencingPath, Element referencedElement, P p) {
    return referencedElement.accept(
        new SimpleElementVisitor8<R, P>() {
          @Override
          @Nullable
          public R visitType(TypeElement e, P p) {
            return resolveType(referencingPath, e, p);
          }

          @Override
          @Nullable
          public R visitPackage(PackageElement e, P p) {
            return resolvePackage(referencingPath, e, p);
          }

          @Override
          protected R defaultAction(Element e, P p) {
            throw new IllegalArgumentException(
                String.format("Unexpected element of kind %s: %s", e.getKind(), e));
          }
        },
        p);
  }

  @Nullable
  protected abstract R resolveType(TreePath referencingPath, TypeElement referencedType, P p);

  @Nullable
  protected abstract R resolvePackage(
      TreePath referencingPath, PackageElement referencedPackage, P p);

  @Override
  @Nullable
  public final R scan(Tree tree, P p) {
    switch (tree.getKind()) {
      case ANNOTATED_TYPE:
      case IDENTIFIER:
      case MEMBER_SELECT:
      case PARAMETERIZED_TYPE:
        return super.scan(tree, p);
      // $CASES-OMITTED$
      default:
        throw new IllegalArgumentException(
            String.format("Unexpected tree of kind %s: %s", tree.getKind(), tree));
    }
  }
}
