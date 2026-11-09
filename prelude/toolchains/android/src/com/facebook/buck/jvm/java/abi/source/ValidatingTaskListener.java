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

import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory.SourceOnlyAbiRuleInfo;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTask;
import com.facebook.buck.jvm.java.plugin.adapter.BuckJavacTaskProxyImpl;
import com.facebook.buck.jvm.java.plugin.adapter.PostEnterTaskListener;
import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskProxy;
import com.facebook.infer.annotation.Nullsafe;
import com.sun.source.tree.CompilationUnitTree;
import com.sun.source.util.TaskEvent;
import com.sun.source.util.TaskListener;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.function.Supplier;
import javax.tools.Diagnostic;
import org.jspecify.annotations.Nullable;

/**
 * A {@link TaskListener} that is used during full compilation to validate the guesses made by
 * source-based ABI generation.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ValidatingTaskListener implements TaskListener {
  private final BuckJavacTask javacTask;
  private final List<CompilationUnitTree> compilationUnits = new ArrayList<>();
  private final SourceOnlyAbiRuleInfo ruleInfo;
  private final Diagnostic.Kind messageKind;
  private final boolean inheritedMemberTypesOnly;
  private final PostEnterTaskListener postEnterListener;

  @Nullable private InterfaceValidator validator;

  public ValidatingTaskListener(
      BuckJavacTaskProxy task,
      SourceOnlyAbiRuleInfo ruleInfo,
      Supplier<Boolean> errorsExist,
      Diagnostic.Kind messageKind) {
    this(task, ruleInfo, errorsExist, messageKind, false);
  }

  public ValidatingTaskListener(
      BuckJavacTaskProxy task,
      SourceOnlyAbiRuleInfo ruleInfo,
      Supplier<Boolean> errorsExist,
      Diagnostic.Kind messageKind,
      boolean inheritedMemberTypesOnly) {
    this.javacTask = ((BuckJavacTaskProxyImpl) task).getInner();
    this.ruleInfo = ruleInfo;
    this.messageKind = messageKind;
    this.inheritedMemberTypesOnly = inheritedMemberTypesOnly;
    this.postEnterListener =
        new PostEnterTaskListener(
            javacTask,
            ignored -> {
              if (!errorsExist.get()) {
                getValidator().validate(compilationUnits);
              }
            });
  }

  private InterfaceValidator getValidator() {
    // We can't do this on construction, because the Task might not be fully initialized yet
    if (validator == null) {
      validator =
          new InterfaceValidator(messageKind, javacTask, ruleInfo, inheritedMemberTypesOnly);
    }
    return Objects.requireNonNull(validator);
  }

  @Override
  public void started(TaskEvent e) {
    postEnterListener.started(e);
  }

  @Override
  public void finished(TaskEvent e) {
    if (e.getKind() == TaskEvent.Kind.PARSE) {
      compilationUnits.add(e.getCompilationUnit());
    }
    postEnterListener.finished(e);
  }
}
