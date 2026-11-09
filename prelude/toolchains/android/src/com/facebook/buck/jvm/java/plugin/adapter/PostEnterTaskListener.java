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
import com.sun.source.tree.ClassTree;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;
import com.sun.source.util.TreePathScanner;
import java.util.Collections;
import java.util.LinkedHashSet;
import java.util.Objects;
import java.util.Set;
import java.util.function.Consumer;
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeElement;
import javax.lang.model.util.Elements;
import javax.tools.JavaFileObject;

/** A {@link TaskListener} that runs some code after the final enter phase. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PostEnterTaskListener implements TaskListener {

  private final BuckJavacTask task;
  private final Consumer<Set<Element>> callback;
  private final Set<Element> topLevelElements = new LinkedHashSet<>();

  private boolean annotationProcessing = false;
  private int pendingEnterCalls = 0;

  public PostEnterTaskListener(BuckJavacTask task, Consumer<Set<Element>> callback) {
    this.task = task;
    this.callback = callback;
  }

  @Override
  public void started(TaskEvent e) {
    switch (e.getKind()) {
      case ANNOTATION_PROCESSING:
        // The raw event stream from javac will start with this event if there are any APs present
        if (!topLevelElements.isEmpty()) {
          // Though we can get multiple annotation processing rounds within the context of
          // annotation processing, annotation processing at a high level shouldn't happen more than
          // once.
          throw new IllegalStateException("Annotation processing happened more than once");
        }
        annotationProcessing = true;
        break;
      case ENTER:
        if (pendingEnterCalls == 0) {
          // We may enter the tree multiple times as part of multiple annotation processing rounds.
          // Make sure we grab the most up-to-date top level elements from the last time they are
          // entered.
          topLevelElements.clear();
        }
        pendingEnterCalls += 1;
        break;
      // $CASES-OMITTED$
      default:
        break;
    }
  }

  @Override
  public void finished(TaskEvent e) {
    switch (e.getKind()) {
      case ANNOTATION_PROCESSING:
        annotationProcessing = false;
        break;
      case ENTER:
        pendingEnterCalls -= 1;
        CompilationUnitTree compilationUnit = e.getCompilationUnit();
        new TreePathScanner<Void, Void>() {
          @Override
          public Void visitCompilationUnit(CompilationUnitTree node, Void aVoid) {
            if (Objects.requireNonNull(e.getSourceFile())
                .isNameCompatible("package-info", JavaFileObject.Kind.SOURCE)) {
              Elements elements = task.getElements();
              Element packageElement =
                  Objects.requireNonNull(
                      elements.getPackageElement(
                          Objects.requireNonNull(node.getPackageName()).toString()));
              topLevelElements.add(packageElement);
            }

            // Technically getTypeDecls can also return some imports due to a bug in javac, but
            // that's fine in this case because we'll just ignore them.
            return scan(node.getTypeDecls(), null);
          }

          @Override
          public Void visitClass(ClassTree node, Void aVoid) {
            TypeElement typeElement = (TypeElement) task.getTrees().getElement(getCurrentPath());
            topLevelElements.add(typeElement);
            return null;
          }
        }.scan(compilationUnit, null);
        break;
      // $CASES-OMITTED$
      default:
        break;
    }

    // For source ABI generation, we want to short circuit the compiler as early as possible to reap
    // the performance benefits. When annotation processing isn't involved, the last ENTER finished
    // event, which comes before the ANALYZE and GENERATE phases, tells us the right time to do
    // this. In Java 9+, the last set of ENTER events happen right *before* the
    // ANNOTATION_PROCESSING finished event, so we have to rely on the ANNOTATION_PROCESSING
    // finished event itself.
    if ((e.getKind() == TaskEvent.Kind.ENTER && !annotationProcessing && pendingEnterCalls == 0)
        || e.getKind() == TaskEvent.Kind.ANNOTATION_PROCESSING) {
      Set<Element> unmodifiableTopLevelElements = Collections.unmodifiableSet(topLevelElements);
      callback.accept(unmodifiableTopLevelElements);
    }
  }
}
