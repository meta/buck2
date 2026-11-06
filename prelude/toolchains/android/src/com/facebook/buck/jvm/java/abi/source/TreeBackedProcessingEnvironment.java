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
import com.sun.source.util.Trees;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;
import javax.annotation.processing.Filer;
import javax.annotation.processing.Messager;
import javax.annotation.processing.ProcessingEnvironment;
import javax.lang.model.SourceVersion;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
class TreeBackedProcessingEnvironment implements ProcessingEnvironment {
  private final FrontendOnlyJavacTask task;
  private final ProcessingEnvironment javacProcessingEnvironment;
  private final TreeBackedMessager messager;
  private final TreeBackedFiler filer;
  private final Map<String, String> options = new HashMap<>();

  public TreeBackedProcessingEnvironment(
      FrontendOnlyJavacTask task, ProcessingEnvironment javacProcessingEnvironment) {
    this.task = task;
    this.javacProcessingEnvironment = javacProcessingEnvironment;
    messager = new TreeBackedMessager(task, javacProcessingEnvironment.getMessager());
    filer = new TreeBackedFiler(task, javacProcessingEnvironment.getFiler());

    options.putAll(javacProcessingEnvironment.getOptions());
    options.put("com.facebook.buck.java.generating_abi", "true");
  }

  @Override
  public Map<String, String> getOptions() {
    return options;
  }

  @Override
  public Messager getMessager() {
    return messager;
  }

  @Override
  public Filer getFiler() {
    return filer;
  }

  @Override
  public Elements getElementUtils() {
    return task.getElements();
  }

  @Override
  public Types getTypeUtils() {
    return task.getTypes();
  }

  public Trees getTreeUtils() {
    return task.getTrees();
  }

  @Override
  public SourceVersion getSourceVersion() {
    return javacProcessingEnvironment.getSourceVersion();
  }

  @Override
  @Nullable
  public Locale getLocale() {
    return javacProcessingEnvironment.getLocale();
  }
}
