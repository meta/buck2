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

import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacPlugin;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTask;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;
import java.util.Objects;

/**
 * Watches the phases of the compiler and keeps our parallel symbol table in sync. Put task
 * listeners that need access to our symbol tables inside this one.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
class EnteringPlugin implements BuckJavacPlugin {
  @Override
  public String getName() {
    return "EnteringPlugin";
  }

  @Override
  public void init(BuckJavacTask task, String... args) {
    FrontendOnlyJavacTask frontendTask = (FrontendOnlyJavacTask) task;
    task.addTaskListener(
        new EnteringTaskListener(
            frontendTask.getElements(), frontendTask.getTypes(), frontendTask.getTrees()));
  }

  private static class EnteringTaskListener implements TaskListener {
    private final TreeBackedElements elements;
    private final TreeBackedTrees trees;
    private final TreeBackedEnter enter;
    private int classesEnteredThisRound = 0;
    private int entersInProgress = 0;

    public EnteringTaskListener(
        TreeBackedElements elements, TreeBackedTypes types, TreeBackedTrees trees) {
      this.elements = elements;
      this.trees = trees;
      enter = new TreeBackedEnter(this.elements, types, this.trees.getJavacTrees());
    }

    @Override
    public void started(TaskEvent e) {
      if (e.getKind() == TaskEvent.Kind.ENTER) {
        entersInProgress += 1;
        if (classesEnteredThisRound == 0) {
          // We want to clear our tables between rounds, and leave them intact after the last enter
          // phase. However, javac sends the round start after the enter phase, so we must do some
          // bookkeeping to detect the first enter event *after* a round and clear the table then.
          elements.clear();
          trees.clear();
        }
      }
    }

    @Override
    public void finished(TaskEvent e) {
      if (e.getKind() == TaskEvent.Kind.ENTER) {
        enter.enter(Objects.requireNonNull(e.getCompilationUnit()));
        classesEnteredThisRound += 1;
        entersInProgress -= 1;

        if (entersInProgress == 0) {
          elements.complete();
        }
      } else if (e.getKind() == TaskEvent.Kind.ANNOTATION_PROCESSING_ROUND) {
        // Reset counter for next round
        classesEnteredThisRound = 0;
      }
    }
  }
}
