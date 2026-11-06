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

import com.facebook.buck.jvm.java.abi.source.api.SourceCodeWillNotCompileException;
import com.facebook.buck.jvm.java.abi.source.api.StopCompilation;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTask;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.tree.Tree;
import com.sun.source.util.JavacTask;
import com.sun.source.util.Trees;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Objects;
import java.util.Set;
import java.util.stream.Collectors;
import java.util.stream.StreamSupport;
import javax.annotation.processing.Processor;
import javax.lang.model.element.Element;
import javax.lang.model.type.TypeMirror;
import javax.lang.model.util.Elements;
import javax.lang.model.util.Types;
import javax.tools.JavaFileObject;
import org.jspecify.annotations.Nullable;

/**
 * An implementation of {@link JavacTask} that implements only the frontend portions of the task,
 * using only the parse phase of the underlying compiler and without requiring a complete classpath.
 * This effectively does the same thing as the Enter phase of the compiler (see
 * http://openjdk.java.net/groups/compiler/doc/compilation-overview/index.html), but applying
 * heuristics when dependencies are missing. This necessarily requires some assumptions to be made
 * about references to symbols defined in those dependencies. See the documentation of {@link
 * com.facebook.buck.jvm.java.abi.source} for details.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class FrontendOnlyJavacTask extends BuckJavacTask {
  private final JavacTask javacTask;

  @Nullable private TreeBackedElements elements;
  @Nullable private TreeBackedTrees trees;
  @Nullable private TreeBackedTypes types;

  @Nullable private Iterable<? extends CompilationUnitTree> parsedCompilationUnits;
  @Nullable private List<Element> topLevelElements;
  private boolean stopCompilationAfterEnter = false;

  public FrontendOnlyJavacTask(JavacTask task) {
    super(task);
    javacTask = task;

    // Add the entering plugin first so that all other plugins and annotation processors will
    // run with the TreeBackedElements already entered
    addPlugin(new EnteringPlugin());
  }

  @Override
  public Iterable<? extends CompilationUnitTree> parse() throws IOException {
    if (parsedCompilationUnits == null) {
      parsedCompilationUnits = javacTask.parse();
    }

    return parsedCompilationUnits;
  }

  @Override
  public Iterable<? extends Element> enter() throws IOException {
    Iterable<? extends Element> javacTopLevelElements = super.enter();

    topLevelElements =
        StreamSupport.stream(javacTopLevelElements.spliterator(), false)
            .map(getElements()::getCanonicalElement)
            .collect(Collectors.toList());

    return topLevelElements;
  }

  @Override
  public Iterable<? extends Element> analyze() {
    throw new UnsupportedOperationException("Code analysis not supported");
  }

  @Override
  public Iterable<? extends JavaFileObject> generate() {
    throw new UnsupportedOperationException("Code generation not supported");
  }

  @Override
  public TypeMirror getTypeMirror(Iterable<? extends Tree> path) {
    throw new UnsupportedOperationException();
  }

  @Override
  public TreeBackedElements getElements() {
    if (elements == null) {
      initUtils();
    }
    return Objects.requireNonNull(elements);
  }

  @Override
  public TreeBackedTrees getTrees() {
    if (trees == null) {
      initUtils();
    }
    return Objects.requireNonNull(trees);
  }

  @Override
  public TreeBackedTypes getTypes() {
    if (types == null) {
      initUtils();
    }
    return Objects.requireNonNull(types);
  }

  private void initUtils() {
    Elements javacElements = javacTask.getElements();
    Trees javacTrees = super.getTrees();
    Types javacTypes = javacTask.getTypes();
    elements = new TreeBackedElements(javacElements, javacTypes, javacTrees);
    this.types = new TreeBackedTypes(javacTypes);
    trees =
        new TreeBackedTrees(
            javacTrees, new PostEnterCanonicalizer(elements, this.types, javacTrees));
  }

  @Override
  public void setProcessors(Iterable<? extends Processor> processors) {
    javacTask.setProcessors(wrap(processors));
  }

  private List<TreeBackedProcessorWrapper> wrap(Iterable<? extends Processor> processors) {
    List<TreeBackedProcessorWrapper> result = new ArrayList<>();
    for (Processor processor : processors) {
      result.add(new TreeBackedProcessorWrapper(this, processor));
    }

    return result;
  }

  @Override
  public void setLocale(Locale locale) {
    throw new UnsupportedOperationException("NYI");
  }

  @Override
  public Boolean call() {
    try {
      stopCompilationAfterEnter = true;
      return javacTask.call();
    } catch (RuntimeException e) {
      if (e.getCause() instanceof StopCompilation) {
        return true;
      } else if (e.getCause() instanceof SourceCodeWillNotCompileException) {
        return false;
      }

      throw e;
    }
  }

  @Override
  protected void onPostEnter(Set<Element> topLevelElements) {
    super.onPostEnter(topLevelElements);

    if (stopCompilationAfterEnter) {
      throw new StopCompilation();
    }
  }
}
