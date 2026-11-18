/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.core.util.graph;

import com.facebook.infer.annotation.Nullsafe;

/** Minimal interface needed by {@link AbstractBottomUpTraversal} to traverse a graph. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface TraversableGraph<T> {

  /**
   * @return {@link Iterable} that the caller is not allowed to mutate.
   */
  Iterable<T> getNodesWithNoIncomingEdges();

  /**
   * @return {@link Iterable} that the caller is not allowed to mutate.
   */
  Iterable<T> getNodesWithNoOutgoingEdges();

  /**
   * @return {@link Iterable} that the caller is not allowed to mutate.
   */
  Iterable<T> getIncomingNodesFor(T sink);

  /**
   * @return {@link Iterable} that the caller is not allowed to mutate.
   */
  Iterable<T> getOutgoingNodesFor(T source);

  /**
   * @return an unmodifiable view of the nodes in this graph
   */
  Iterable<T> getNodes();
}
