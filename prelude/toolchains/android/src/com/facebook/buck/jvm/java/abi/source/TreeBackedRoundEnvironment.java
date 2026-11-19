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
import java.lang.annotation.Annotation;
import java.util.LinkedHashSet;
import java.util.Set;
import java.util.stream.Collector;
import java.util.stream.Collectors;
import javax.annotation.processing.RoundEnvironment;
import javax.lang.model.element.Element;
import javax.lang.model.element.TypeElement;

@Nullsafe(Nullsafe.Mode.LOCAL)
class TreeBackedRoundEnvironment implements RoundEnvironment {
  private final FrontendOnlyJavacTask task;
  private final RoundEnvironment javacRoundEnvironment;

  public TreeBackedRoundEnvironment(
      FrontendOnlyJavacTask task, RoundEnvironment javacRoundEnvironment) {
    this.task = task;
    this.javacRoundEnvironment = javacRoundEnvironment;
  }

  @Override
  public boolean processingOver() {
    return javacRoundEnvironment.processingOver();
  }

  @Override
  public boolean errorRaised() {
    return javacRoundEnvironment.errorRaised();
  }

  @Override
  public Set<? extends Element> getRootElements() {
    return javacRoundEnvironment.getRootElements().stream()
        .map(task.getElements()::getCanonicalElement)
        .collect(toSet());
  }

  @Override
  public Set<? extends Element> getElementsAnnotatedWith(TypeElement a) {
    return javacRoundEnvironment
        .getElementsAnnotatedWith(task.getElements().getJavacElement(a))
        .stream()
        .map(task.getElements()::getCanonicalElement)
        .collect(toSet());
  }

  @Override
  public Set<? extends Element> getElementsAnnotatedWith(Class<? extends Annotation> a) {
    return javacRoundEnvironment.getElementsAnnotatedWith(a).stream()
        .map(task.getElements()::getCanonicalElement)
        .collect(toSet());
  }

  private Collector<Element, ?, Set<Element>> toSet() {
    return Collectors.toCollection(LinkedHashSet::new);
  }
}
