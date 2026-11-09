/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Map;
import java.util.Objects;
import java.util.TreeMap;
import java.util.jar.Attributes;
import java.util.jar.Manifest;
import javax.annotation.Nullable;

/**
 * Registry for manifest attribute mergers. Provides a way to look up the appropriate merger for a
 * given attribute name. If no specific merger is registered for an attribute, the default merger is
 * used.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JarManifestMerger {

  private static final JarManifestMerger INSTANCE = new JarManifestMerger();

  public static JarManifestMerger get() {
    return INSTANCE;
  }

  private static final ManifestAttributeMerger DEFAULT_MERGER =
      (existingValue, newValue) -> newValue;

  private final Map<String, ManifestAttributeMerger> attributeMergers =
      new TreeMap<>(String.CASE_INSENSITIVE_ORDER);

  private JarManifestMerger() {
    register("Multi-Release", new MultiReleaseManifestAttributeMerger());
  }

  protected void register(String attributeName, ManifestAttributeMerger merger) {
    attributeMergers.put(attributeName, merger);
  }

  /**
   * Merge entries from two Manifests together. Attributes are merged using the appropriate {@link
   * ManifestAttributeMerger} for each attribute.
   *
   * @param into The Manifest to modify.
   * @param from The Manifest to copy from.
   */
  public void merge(Manifest into, @Nullable Manifest from) {
    if (into == null) {
      throw new IllegalArgumentException("into manifest cannot be null");
    }
    if (from == null) {
      return;
    }
    mergeAttributes(into.getMainAttributes(), from.getMainAttributes());

    final Map<String, Attributes> entries = from.getEntries();
    if (entries != null) {
      entries.forEach(
          (key, value) ->
              mergeAttributes(
                  into.getEntries().computeIfAbsent(key, k -> new Attributes()), value));
    }
  }

  private void mergeAttributes(Attributes target, Attributes source) {
    if (source == null || target == null) {
      return;
    }
    source.forEach((key, value) -> target.put(key, getMerger(key).merge(target.get(key), value)));
  }

  private ManifestAttributeMerger getMerger(final Object attributeKey) {
    return attributeMergers.getOrDefault(String.valueOf(attributeKey), DEFAULT_MERGER);
  }

  /**
   * Merger for the Multi-Release manifest attribute. If any jar has Multi-Release set to "true",
   * the final merged manifest will have Multi-Release as "true", regardless of subsequent jars
   * having it as "false".
   */
  static class MultiReleaseManifestAttributeMerger implements ManifestAttributeMerger {

    @Override
    public Object merge(@Nullable Object existingValue, Object newValue) {
      if ("true".equalsIgnoreCase(String.valueOf(existingValue))) {
        return Objects.requireNonNull(existingValue);
      }
      return newValue;
    }
  }
}
