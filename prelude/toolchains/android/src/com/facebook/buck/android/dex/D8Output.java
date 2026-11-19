/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.util.Collection;
import java.util.Map;

/** Info exposed from {@code d8}. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class D8Output {
  /** Referenced resources returned by D8 */
  private Collection<String> referencedResources;

  /**
   * Type descriptors ("Lcom/example/Foo;") of every class D8 wrote, as reported by the output
   * consumer. Includes classes invented by desugaring, which have no counterpart among the inputs.
   */
  private Collection<String> writtenClassDescriptors;

  /** Class descriptors emitted in each indexed dex output. */
  private final ImmutableMap<Integer, ImmutableSet<String>> outputClassDescriptors;

  /**
   * Internal names ("com/example/Foo") of each synthetic class D8 created, mapped to the class it
   * was synthesized from. Reported by D8 itself rather than derived from the synthetic's name,
   * whose mangling is an unstable implementation detail.
   */
  private final ImmutableMap<String, String> syntheticToSynthesizingContext;

  private final ImmutableMap<String, DexUnitInfo> dexUnits;

  public D8Output(
      Collection<String> referencedResources,
      Collection<String> writtenClassDescriptors,
      Map<Integer, ImmutableSet<String>> outputClassDescriptors,
      Map<String, String> syntheticToSynthesizingContext,
      Map<String, DexUnitInfo> dexUnits) {
    this.referencedResources = referencedResources;
    this.writtenClassDescriptors = writtenClassDescriptors;
    this.outputClassDescriptors = ImmutableMap.copyOf(outputClassDescriptors);
    this.syntheticToSynthesizingContext = ImmutableMap.copyOf(syntheticToSynthesizingContext);
    this.dexUnits = ImmutableMap.copyOf(dexUnits);
  }

  public ImmutableMap<String, DexUnitInfo> getDexUnits() {
    return dexUnits;
  }

  public Collection<String> getResources() {
    return referencedResources;
  }

  public Collection<String> getWrittenClassDescriptors() {
    return writtenClassDescriptors;
  }

  public ImmutableSet<String> getClassDescriptors(int outputIndex) {
    return outputClassDescriptors.getOrDefault(outputIndex, ImmutableSet.of());
  }

  public ImmutableMap<String, String> getSyntheticToSynthesizingContext() {
    return syntheticToSynthesizingContext;
  }
}
