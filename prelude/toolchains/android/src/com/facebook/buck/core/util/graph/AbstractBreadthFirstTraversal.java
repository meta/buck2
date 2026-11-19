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

/** Performs a breadth-first traversal of dependencies of a graph node. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class AbstractBreadthFirstTraversal<Node>
    extends AbstractBreadthFirstThrowingTraversal<Node, RuntimeException> {

  public AbstractBreadthFirstTraversal(Node initialNode) {
    super(initialNode);
  }

  public AbstractBreadthFirstTraversal(Iterable<? extends Node> initialNodes) {
    super(initialNodes);
  }

  /**
   * Traverse a graph without explicitly creating a {@code new
   * AbstractBreadthFirstThrowingTraversal} and overriding {@link #visit(Object)}
   *
   * @param visitor Typically a lambda expression
   */
  public static <Node> void traverse(Node initialNode, Visitor<Node, RuntimeException> visitor) {
    new StaticBreadthFirstTraversal<>(initialNode, visitor).start();
  }

  /**
   * Traverse a graph without explicitly creating a {@code new
   * AbstractBreadthFirstThrowingTraversal} and overriding {@link #visit(Object)}
   *
   * @param visitor Typically a lambda expression
   */
  public static <Node> void traverse(
      Iterable<? extends Node> initialNodes, Visitor<Node, RuntimeException> visitor) {
    new StaticBreadthFirstTraversal<>(initialNodes, visitor).start();
  }
}
