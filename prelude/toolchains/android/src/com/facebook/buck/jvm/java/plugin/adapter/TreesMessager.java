/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.plugin.adapter;

import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.util.TreePath;
import com.sun.source.util.Trees;
import java.util.Objects;
import javax.annotation.processing.Messager;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.AnnotationValue;
import javax.lang.model.element.Element;
import javax.tools.Diagnostic;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class TreesMessager implements Messager {
  private final Trees trees;

  public TreesMessager(Trees trees) {
    this.trees = trees;
  }

  @Override
  public void printMessage(Diagnostic.Kind kind, CharSequence msg) {
    throw new UnsupportedOperationException();
  }

  @Override
  public void printMessage(Diagnostic.Kind kind, CharSequence msg, Element e) {
    printMessage(kind, msg, Objects.requireNonNull(trees.getPath(e)));
  }

  @Override
  public void printMessage(Diagnostic.Kind kind, CharSequence msg, Element e, AnnotationMirror a) {
    printMessage(kind, msg, Objects.requireNonNull(trees.getPath(e, a)));
  }

  @Override
  public void printMessage(
      Diagnostic.Kind kind, CharSequence msg, Element e, AnnotationMirror a, AnnotationValue v) {
    printMessage(kind, msg, Objects.requireNonNull(trees.getPath(e, a, v)));
  }

  private void printMessage(Diagnostic.Kind kind, CharSequence msg, TreePath path) {
    trees.printMessage(kind, msg, path.getLeaf(), path.getCompilationUnit());
  }
}
