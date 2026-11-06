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
import com.sun.source.tree.BlockTree;
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.MethodTree;
import com.sun.source.tree.Tree;
import com.sun.source.tree.TypeParameterTree;
import com.sun.source.tree.VariableTree;
import com.sun.source.util.TreePathScanner;
import com.sun.source.util.Trees;
import java.util.HashMap;
import java.util.Map;
import javax.lang.model.element.Element;

/**
 * This class is more complete than {@link Trees#getTree(Element)} in that it will return a tree for
 * method and type parameters. It is also faster for doing a lot of lookups within a given
 * compilation unit.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ElementTreeFinder {

  public static ElementTreeFinder forCompilationUnit(CompilationUnitTree tree, Trees trees) {
    // Trees.getTree, in addition to being blind to method and type parameters, does an expensive
    // linear-time iteration whenever asked for a tree for a method or variable. We scan the trees
    // once using the much cheaper Trees.getElement and build a map.
    Map<Element, Tree> elementToTreeMap = new HashMap<>();
    new TreePathScanner<Void, Void>() {
      @Override
      public Void visitClass(ClassTree node, Void aVoid) {
        elementToTreeMap.put(trees.getElement(getCurrentPath()), node);
        return super.visitClass(node, aVoid);
      }

      @Override
      public Void visitMethod(MethodTree node, Void aVoid) {
        elementToTreeMap.put(trees.getElement(getCurrentPath()), node);
        return super.visitMethod(node, aVoid);
      }

      @Override
      public Void visitVariable(VariableTree node, Void aVoid) {
        elementToTreeMap.put(trees.getElement(getCurrentPath()), node);
        // Don't recurse into variable initializers
        return null;
      }

      @Override
      public Void visitTypeParameter(TypeParameterTree node, Void aVoid) {
        elementToTreeMap.put(trees.getElement(getCurrentPath()), node);
        return super.visitTypeParameter(node, aVoid);
      }

      @Override
      public Void visitBlock(BlockTree node, Void aVoid) {
        // Don't recurse into method bodies
        return null;
      }
    }.scan(tree, null);

    return new ElementTreeFinder(elementToTreeMap);
  }

  private final Map<Element, Tree> elementToTreeMap;

  private ElementTreeFinder(Map<Element, Tree> elementToTreeMap) {
    this.elementToTreeMap = elementToTreeMap;
  }

  @Nullable
  public Tree getTree(Element element) {
    return elementToTreeMap.get(element);
  }
}
