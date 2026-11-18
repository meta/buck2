/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi;

import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory.SourceOnlyAbiRuleInfo;
import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskListener;
import com.facebook.buck.jvm.java.plugin.api.BuckJavacTaskProxy;
import com.facebook.buck.jvm.java.plugin.api.PluginClassLoader;
import com.facebook.infer.annotation.Nullsafe;
import java.lang.reflect.Constructor;
import java.util.function.Supplier;
import javax.tools.Diagnostic;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public final class SourceBasedAbiStubber {
  public static BuckJavacTaskListener newInheritedMemberTypeValidatingTaskListener(
      PluginClassLoader pluginLoader,
      BuckJavacTaskProxy task,
      SourceOnlyAbiRuleInfo ruleInfo,
      Supplier<Boolean> errorsExist,
      Diagnostic.@Nullable Kind messageKind) {
    try {
      String className = "com.facebook.buck.jvm.java.abi.source.ValidatingTaskListener";
      Class<?> validatingTaskListenerClass = pluginLoader.loadClass(className, Object.class);
      if (validatingTaskListenerClass == null) {
        throw new ClassNotFoundException(className);
      }
      Constructor<?> constructor =
          validatingTaskListenerClass.getConstructor(
              BuckJavacTaskProxy.class,
              SourceOnlyAbiRuleInfo.class,
              Supplier.class,
              Diagnostic.Kind.class,
              boolean.class);

      return BuckJavacTaskListener.wrapRealTaskListener(
          pluginLoader, constructor.newInstance(task, ruleInfo, errorsExist, messageKind, true));
    } catch (ReflectiveOperationException e) {
      throw new RuntimeException(
          "Could not load source-generated ABI validator. Your compiler might not support this. "
              + "If it doesn't, you may need to disable source-based ABI generation.",
          e);
    }
  }

  private SourceBasedAbiStubber() {}
}
