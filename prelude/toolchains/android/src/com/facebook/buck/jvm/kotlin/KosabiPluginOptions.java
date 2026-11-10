/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import java.util.Map;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KosabiPluginOptions {
  private final ImmutableMap<String, AbsPath> resolvedKosabiPluginOptionPath;

  public KosabiPluginOptions(ImmutableMap<String, AbsPath> resolvedKosabiPluginOptionPath) {
    this.resolvedKosabiPluginOptionPath = resolvedKosabiPluginOptionPath;
  }

  public ImmutableMap<String, AbsPath> getAllKosabiPlugins() {
    return resolvedKosabiPluginOptionPath;
  }

  private ImmutableMap<String, AbsPath> getAllKosabiPluginsExceptStubgen() {
    // The stubsgen plugin and the rokoko stubgen .so are stubgen-step tools,
    // not main-compile plugins: keep them out of -Xplugin args.
    return resolvedKosabiPluginOptionPath.entrySet().stream()
        .filter(entry -> !KosabiConfig.PROPERTY_KOSABI_STUBS_GEN_K2_PLUGIN.equals(entry.getKey()))
        .filter(entry -> !KosabiConfig.PROPERTY_ROKOKO_STUBGEN_SO.equals(entry.getKey()))
        .collect(ImmutableMap.toImmutableMap(Map.Entry::getKey, Map.Entry::getValue));
  }

  public ImmutableMap<String, AbsPath> getKosabiPlugins() {
    return getAllKosabiPluginsExceptStubgen();
  }
}
