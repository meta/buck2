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
import com.google.common.base.Preconditions;
import com.google.common.base.Suppliers;
import com.google.common.collect.ImmutableCollection;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.Sets;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.Objects;
import java.util.function.Function;
import java.util.function.Supplier;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class DirectedAcyclicGraph<T> implements TraversableGraph<T> {

  private final ImmutableSet<T> nodes;
  private final ImmutableMap<T, ImmutableSet<T>> outgoingEdges;
  private final Supplier<ImmutableMap<T, ImmutableCollection<T>>> incomingEdges;

  /** Constructor used by builders. */
  private DirectedAcyclicGraph(
      ImmutableSet<T> nodes, ImmutableMap<T, ImmutableSet<T>> outgoingEdges) {
    Preconditions.checkState(
        TraversableGraphs.isAcyclic(nodes, n -> outgoingEdges.getOrDefault(n, ImmutableSet.of())),
        "Graph must by acyclic");
    this.nodes = nodes;
    this.outgoingEdges = outgoingEdges;
    this.incomingEdges =
        Suppliers.memoize(() -> computeIncomingEdges(this.nodes, this.outgoingEdges));
  }

  /** Copy constructor. */
  protected DirectedAcyclicGraph(DirectedAcyclicGraph<T> other) {
    this.nodes = other.nodes;
    this.outgoingEdges = other.outgoingEdges;
    this.incomingEdges = other.incomingEdges;
  }

  /**
   * Legacy constructor to convert form {@link MutableDirectedGraph}.
   *
   * @deprecated Use {@link DirectedAcyclicGraph#serialBuilder()} to build a graph.
   */
  @Deprecated
  @SuppressWarnings({"unchecked", "rawtypes"})
  public DirectedAcyclicGraph(MutableDirectedGraph<T> graph) {
    Preconditions.checkState(
        TraversableGraphs.isAcyclic(graph.getNodes(), graph::getOutgoingNodesFor),
        "Graph must by acyclic");
    this.nodes = graph.createImmutableCopyOfNodes();
    this.outgoingEdges = graph.createImmutableCopyOfOutgoingEdges();
    this.incomingEdges =
        Suppliers.ofInstance((ImmutableMap) graph.createImmutableCopyOfIncomingEdges());
  }

  /**
   * Consume a {@link Map} of map of nodes to builders and produce an {@link ImmutableMap} of their
   * built collections. The input map will be empty once complete.
   *
   * @return an {@link ImmutableMap} built from a {@link Map} of builders.
   */
  private static <T, C extends ImmutableCollection<T>, B> ImmutableMap<T, C> toImmutableMap(
      Function<B, C> build, Map<T, B> map) {
    ImmutableMap.Builder<T, C> builder = ImmutableMap.builderWithExpectedSize(map.size());
    for (Iterator<Map.Entry<T, B>> itr = map.entrySet().iterator(); itr.hasNext(); ) {
      Map.Entry<T, B> entry = itr.next();
      // Remove the builder from the map before building it.  This allows the builder to be garbage
      // collected while we're processing remaining builders, to help prevent this function from
      // requiring 2x the size of the input map to run.
      itr.remove();
      builder.put(entry.getKey(), Preconditions.checkNotNull(build.apply(entry.getValue())));
    }
    return builder.build();
  }

  /**
   * @return the map of incoming node edges, computed from the map of outgoing node edges.
   */
  private static <T> ImmutableMap<T, ImmutableCollection<T>> computeIncomingEdges(
      ImmutableSet<T> nodes, ImmutableMap<T, ImmutableSet<T>> outgoingEdges) {
    Map<T, ImmutableList.Builder<T>> builder = new HashMap<>();
    for (T source : nodes) {
      for (T sink : outgoingEdges.getOrDefault(source, ImmutableSet.of())) {
        builder.computeIfAbsent(sink, n -> ImmutableList.builder()).add(source);
      }
    }
    return toImmutableMap(ImmutableList.Builder::build, builder);
  }

  @Override
  public ImmutableSet<T> getOutgoingNodesFor(T source) {
    return outgoingEdges.getOrDefault(source, ImmutableSet.of());
  }

  @Override
  public ImmutableCollection<T> getIncomingNodesFor(T sink) {
    return incomingEdges.get().getOrDefault(sink, ImmutableSet.of());
  }

  @Override
  public ImmutableSet<T> getNodesWithNoOutgoingEdges() {
    return ImmutableSet.copyOf(Sets.difference(nodes, outgoingEdges.keySet()));
  }

  @Override
  public ImmutableSet<T> getNodesWithNoIncomingEdges() {
    return ImmutableSet.copyOf(Sets.difference(nodes, incomingEdges.get().keySet()));
  }

  @Override
  public ImmutableSet<T> getNodes() {
    return nodes;
  }

  @Override
  public boolean equals(@Nullable Object other) {
    if (!(other instanceof DirectedAcyclicGraph)) {
      return false;
    }

    DirectedAcyclicGraph<?> that = (DirectedAcyclicGraph<?>) other;
    return Objects.equals(this.nodes, that.nodes)
        && Objects.equals(this.outgoingEdges, that.outgoingEdges);
  }

  @Override
  public int hashCode() {
    return Objects.hash(nodes, outgoingEdges, incomingEdges);
  }

  public static <T> Builder<T> serialBuilder() {
    return new DirectedAcyclicGraph.Builder<>();
  }

  public static class Builder<T> {

    // To avoid copying, we only support a single use of this builder.
    protected boolean finished = false;

    protected Collection<T> nodes = new ArrayList<>();
    protected Map<T, Collection<T>> outgoingEdges = new HashMap<>();

    private Builder() {}

    /** Insert node into the graph. */
    public Builder<T> addNode(T node) {
      Preconditions.checkState(!finished);
      nodes.add(node);
      return this;
    }

    protected void putEdge(Collection<T> edges, T edge) {
      edges.add(edge);
    }

    /** Add an edge from {@code source} to {@code sink}. */
    public Builder<T> addEdge(T source, T sink) {
      Preconditions.checkState(!source.equals(sink));
      Preconditions.checkState(!finished);
      nodes.add(source);
      nodes.add(sink);
      putEdge(outgoingEdges.computeIfAbsent(source, s -> new ArrayList<>()), sink);
      return this;
    }

    /** Consumes the given node collection and converts to an {@link ImmutableSet}. */
    private ImmutableSet<T> buildImmutableSet(Collection<T> nodes) {
      // NOTE: `ImmutableSet.copyOf()` doesn't pre-size the builder, so we do it ourselves as, in
      // practice, we don't add duplicate nodes/edges.
      ImmutableSet.Builder<T> builder =
          ImmutableSet.<T>builderWithExpectedSize(nodes.size()).addAll(nodes);

      // Clear out the existing node list before building, to potentially free up memory.
      nodes.clear();
      if (nodes instanceof ArrayList) {
        ((ArrayList<T>) nodes).trimToSize();
      }

      return builder.build();
    }

    /**
     * Build a {@link DirectedAcyclicGraph}. This builder cannot be modified once this has been
     * called.
     */
    public DirectedAcyclicGraph<T> build() {
      Preconditions.checkState(!finished);
      finished = true;

      ImmutableSet<T> nodes = buildImmutableSet(this.nodes);
      ImmutableMap<T, ImmutableSet<T>> outgoingEdges =
          DirectedAcyclicGraph.toImmutableMap(this::buildImmutableSet, this.outgoingEdges);
      return new DirectedAcyclicGraph<>(nodes, outgoingEdges);
    }
  }
}
