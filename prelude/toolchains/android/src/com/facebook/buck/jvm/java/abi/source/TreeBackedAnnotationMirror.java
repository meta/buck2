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
import com.sun.source.tree.AnnotationTree;
import com.sun.source.tree.AssignmentTree;
import com.sun.source.tree.ExpressionTree;
import com.sun.source.tree.IdentifierTree;
import com.sun.source.tree.Tree;
import com.sun.source.util.TreePath;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.stream.Collectors;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.AnnotationValue;
import javax.lang.model.element.ExecutableElement;
import javax.lang.model.element.Name;
import javax.lang.model.type.DeclaredType;

@Nullsafe(Nullsafe.Mode.LOCAL)
class TreeBackedAnnotationMirror implements ArtificialAnnotationMirror {
  private final AnnotationMirror underlyingAnnotationMirror;
  private final TreePath treePath;
  private final AnnotationTree tree;
  private final PostEnterCanonicalizer canonicalizer;

  @Nullable private DeclaredType type;
  @Nullable private Map<ExecutableElement, TreeBackedAnnotationValue> elementValues;

  TreeBackedAnnotationMirror(
      AnnotationMirror underlyingAnnotationMirror,
      TreePath treePath,
      PostEnterCanonicalizer canonicalizer) {
    this.underlyingAnnotationMirror = underlyingAnnotationMirror;
    this.treePath = treePath;
    this.tree = (AnnotationTree) treePath.getLeaf();
    this.canonicalizer = canonicalizer;
  }

  /* package */ AnnotationMirror getUnderlyingAnnotationMirror() {
    return underlyingAnnotationMirror;
  }

  public TreePath getTreePath() {
    return treePath;
  }

  @Override
  public DeclaredType getAnnotationType() {
    if (type == null) {
      type =
          (DeclaredType)
              canonicalizer.getCanonicalType(
                  underlyingAnnotationMirror.getAnnotationType(),
                  treePath,
                  tree.getAnnotationType());
    }
    return type;
  }

  @Override
  public Map<ExecutableElement, TreeBackedAnnotationValue> getElementValues() {
    if (elementValues == null) {
      Map<ExecutableElement, TreeBackedAnnotationValue> result = new LinkedHashMap<>();
      Map<String, Tree> trees = new HashMap<>();

      List<? extends ExpressionTree> arguments = tree.getArguments();
      for (ExpressionTree argument : arguments) {
        if (argument.getKind() != Tree.Kind.ASSIGNMENT) {
          trees.put("value", argument);
        } else {
          AssignmentTree assignment = (AssignmentTree) argument;
          IdentifierTree nameTree = (IdentifierTree) assignment.getVariable();
          trees.put(nameTree.getName().toString(), argument);
        }
      }

      for (Map.Entry<? extends ExecutableElement, ? extends AnnotationValue> entry :
          underlyingAnnotationMirror.getElementValues().entrySet()) {
        ExecutableElement underlyingKeyElement = entry.getKey();

        Tree valueTree =
            Objects.requireNonNull(trees.get(entry.getKey().getSimpleName().toString()));

        result.put(
            canonicalizer.getCanonicalElement(underlyingKeyElement),
            new TreeBackedAnnotationValue(
                entry.getValue(), new TreePath(treePath, valueTree), canonicalizer));
      }

      elementValues = Collections.unmodifiableMap(result);
    }
    return elementValues;
  }

  @Override
  public String toString() {
    StringBuilder result = new StringBuilder();
    result.append("@");
    result.append(getAnnotationType());
    Map<ExecutableElement, TreeBackedAnnotationValue> elementValues = getElementValues();
    if (!elementValues.isEmpty()) {
      result.append("(");
      result.append(
          elementValues.entrySet().stream()
              .map(
                  entry -> {
                    Name key = entry.getKey().getSimpleName();
                    TreeBackedAnnotationValue value = entry.getValue();
                    if (elementValues.size() == 1 && key.contentEquals("value")) {
                      return value.toString();
                    } else {
                      return String.format("%s=%s", key, value);
                    }
                  })
              .collect(Collectors.joining(", ")));
      result.append(")");
    }

    return result.toString();
  }
}
