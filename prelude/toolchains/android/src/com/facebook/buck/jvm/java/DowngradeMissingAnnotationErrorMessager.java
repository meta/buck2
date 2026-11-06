/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.infer.annotation.Nullsafe;
import javax.annotation.processing.Messager;
import javax.lang.model.element.AnnotationMirror;
import javax.lang.model.element.AnnotationValue;
import javax.lang.model.element.Element;
import javax.tools.Diagnostic;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class DowngradeMissingAnnotationErrorMessager implements Messager {

  private final Messager delegate;

  public DowngradeMissingAnnotationErrorMessager(Messager delegate) {
    this.delegate = delegate;
  }

  @Override
  public void printMessage(Diagnostic.Kind kind, CharSequence charSequence) {
    delegate.printMessage(kind, charSequence);
  }

  @Override
  public void printMessage(Diagnostic.Kind kind, CharSequence charSequence, Element element) {
    delegate.printMessage(kind, charSequence, element);
  }

  @Override
  public void printMessage(
      Diagnostic.Kind kind,
      CharSequence charSequence,
      Element element,
      AnnotationMirror annotationMirror) {
    if (annotationMirror != null && kind == Diagnostic.Kind.ERROR) {
      delegate.printMessage(Diagnostic.Kind.WARNING, charSequence, element, annotationMirror);
    } else {
      delegate.printMessage(kind, charSequence, element, annotationMirror);
    }
  }

  @Override
  public void printMessage(
      Diagnostic.Kind kind,
      CharSequence charSequence,
      Element element,
      AnnotationMirror annotationMirror,
      AnnotationValue annotationValue) {
    delegate.printMessage(kind, charSequence, element, annotationMirror, annotationValue);
  }
}
