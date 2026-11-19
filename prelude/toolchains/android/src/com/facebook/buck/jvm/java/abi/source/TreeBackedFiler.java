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
import java.io.IOException;
import javax.annotation.processing.Filer;
import javax.lang.model.element.Element;
import javax.tools.FileObject;
import javax.tools.JavaFileManager;
import javax.tools.JavaFileObject;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
class TreeBackedFiler implements Filer {
  private final FrontendOnlyJavacTask task;
  private final Filer javacFiler;

  public TreeBackedFiler(FrontendOnlyJavacTask task, Filer javacFiler) {
    this.task = task;
    this.javacFiler = javacFiler;
  }

  @Override
  public JavaFileObject createSourceFile(CharSequence name, Element... originatingElements)
      throws IOException {
    return javacFiler.createSourceFile(
        name, task.getElements().getJavacElements(originatingElements));
  }

  @Override
  public JavaFileObject createClassFile(
      CharSequence name, Element @Nullable ... originatingElements) throws IOException {
    return javacFiler.createClassFile(
        name, task.getElements().getJavacElements(originatingElements));
  }

  @Override
  public FileObject createResource(
      JavaFileManager.Location location,
      CharSequence pkg,
      CharSequence relativeName,
      Element @Nullable ... originatingElements)
      throws IOException {
    return javacFiler.createResource(
        location, pkg, relativeName, task.getElements().getJavacElements(originatingElements));
  }

  @Override
  public FileObject getResource(
      JavaFileManager.Location location, CharSequence pkg, CharSequence relativeName)
      throws IOException {
    return javacFiler.getResource(location, pkg, relativeName);
  }
}
