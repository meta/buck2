# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:expect.bzl", "expect")

def pre_order_traversal(
    graph: dict[typing.Any, list[typing.Any]],
    node_formatter: typing.Callable[[typing.Any], str] = str,
    edge_explainer: typing.Callable[[typing.Any, typing.Any], list[str]] = lambda _src, _dest: ["Unknown"],
) -> list[typing.Any]:
    """
    Perform a pre-order (topologically sorted) traversal of `graph` and return the ordered nodes
    """

    # A repeated dep is counted once per occurrence here and decremented once per occurrence below,
    # so it needs no deduplication (Kahn's algorithm works correctly for multigraphs).
    in_degrees = {node: 0 for node in graph}
    for _node, deps in graph.items():
        for dep in deps:
            in_degrees[dep] += 1

    queue = []

    for node, in_degree in in_degrees.items():
        if in_degree == 0:
            queue.append(node)

    ordered = []

    for _ in range(len(in_degrees)):
        if len(queue) == 0:
            fail_cycle(graph, node_formatter, edge_explainer)

        node = queue.pop()
        ordered.append(node)

        for dep in graph[node]:
            in_degrees[dep] -= 1
            if in_degrees[dep] == 0:
                queue.append(dep)

    expect(not queue, "finished before processing nodes: {}".format([node_formatter(node) for node in queue]))
    expect(len(ordered) == len(graph), "missing or duplicate nodes in sort")

    return ordered

def post_order_traversal(
    graph: dict[typing.Any, list[typing.Any]],
    node_formatter: typing.Callable[[typing.Any], str] = str,
    edge_explainer: typing.Callable[[typing.Any, typing.Any], list[str]] = lambda _src, _dest: ["Unknown"],
) -> list[typing.Any]:
    """
    Performs a post-order traversal of `graph`.
    """

    out_degrees = {}
    rdeps = {node: [] for node in graph}
    for node, deps in graph.items():
        deps = dedupe(deps)
        out_degrees[node] = len(deps)
        for dep in deps:
            rdeps[dep].append(node)

    queue = [node for node, out_degree in out_degrees.items() if out_degree == 0]

    ordered = []

    for _ in range(len(out_degrees)):
        if len(queue) == 0:
            fail_cycle(graph, node_formatter, edge_explainer)

        node = queue.pop()
        ordered.append(node)

        for dep in rdeps[node]:
            out_degrees[dep] -= 1
            if out_degrees[dep] == 0:
                queue.append(dep)

    expect(not queue, "finished before processing nodes: {}".format([node_formatter(node) for node in queue]))
    expect(len(ordered) == len(graph), "missing or duplicate nodes in sort")

    return ordered

def fail_cycle(
    graph: dict[typing.Any, list[typing.Any]],
    node_formatter: typing.Callable[[typing.Any], str],
    edge_explainer: typing.Callable[[typing.Any, typing.Any], list[str]],
) -> typing.Never:
    cycle = find_cycle(graph)
    if cycle:
        errors = []
        for i, c in enumerate(cycle):
            indented_number = (" -> " if i > 0 else "    ") + "" * (3 - len(str(i))) + str(i + 1) + ": "
            edge_explanation = ""
            if i > 0:
                edge_explanation = "\n" + " " * 9 + "Reason for edge:"
                edge_explanation += "".join(["\n" + " " * 11 + e for e in edge_explainer(cycle[i - 1], c)])
            errors.append(indented_number + node_formatter(c) + edge_explanation)
        fail(
            "cycle detected between the following targets:\n    {}\n\ncycle details:\n{}\n\n".format(
                "\n -> ".join(
                    [node_formatter(c) for c in cycle],
                ),
                "\n\n".join(errors),
            ),
        )

    fail("expected cycle, but found none")

def find_cycle(graph: dict[typing.Any, list[typing.Any]]) -> list[typing.Any] | None:
    visited = {}
    OUTPUT = 1
    VISIT = 2
    current_parents = []
    work = [(VISIT, n) for n in graph.keys()]
    for _ in range(2000000000):
        if not work:
            break
        kind, node = work.pop()
        if kind == VISIT:
            if node not in visited:
                visited[node] = True
                current_parents.append(node)

                work.append((OUTPUT, node))
                for dep in graph[node]:
                    if dep in current_parents:
                        return current_parents[current_parents.index(dep) :] + [dep]
                    if dep not in visited:
                        work.append((VISIT, dep))
        else:
            current_parents.pop()

    return None

def post_order_traversal_by(roots: typing.Iterable, get_nodes_to_traverse_func) -> list[typing.Any]:
    """
    Returns the post-order sorted list of the nodes in the traversal.

    This implementation simply performs a dfs. We maintain a work stack here.
    When visiting a node, we first add an item to the work stack to output that
    node, and then add items to visit all the children. While a work item for a
    child will not be added if it has already been visited, if there's an item in
    the stack for that child it will still be added. When popping the visit, if
    the node had been visited, it's ignored. This ensures that a node's children are
    all visited before we output that node.
    """
    ordered = []
    visited = {}
    OUTPUT = 1
    VISIT = 2
    queue = [(VISIT, n) for n in roots]
    for _ in range(2000000000):
        if not queue:
            break

        kind, node = queue.pop()
        if kind == VISIT:
            if not node in visited:
                queue.append((OUTPUT, node))
                for dep in get_nodes_to_traverse_func(node):
                    if dep not in visited:
                        queue.append((VISIT, dep))
        else:
            visited[node] = True
            ordered.append(node)
    return ordered

def pre_order_traversal_by(roots: typing.Iterable, get_nodes_to_traverse_func) -> list[typing.Any]:
    """
    Returns a topological sorted list of the nodes from a pre-order traversal.

    Note this gives a different order from `pre_order_traversal` above (to simplify the implementation).
    """
    ordered = post_order_traversal_by(roots, get_nodes_to_traverse_func)
    return ordered[::-1]

def depth_first_traversal(graph_nodes: dict[typing.Any, list[typing.Any]], roots: typing.Iterable) -> list[typing.Any]:
    """
    Like `depth_first_traversal_by` but the nodes are stored in the graph.
    """

    def lookup(x):
        return graph_nodes[x]

    return depth_first_traversal_by(graph_nodes, roots, lookup)

# With following graph
#
#          A
#        /   \
#      B      C
#     / \    / \
#    D   E  F   G
#
# preorder-left-to-right starting from A will go to left leg first
#                       A-B-D-E-C-F-G
#
# preorder-right-to-left starting from A will go to right leg first
#                       A-C-G-F-B-E-D
#
GraphTraversal = enum(
    "preorder-right-to-left",
    "preorder-left-to-right",
)

def depth_first_traversal_by(
    graph_nodes: [dict[typing.Any, typing.Any], None],
    roots: typing.Iterable,
    get_nodes_to_traverse_func: typing.Callable,
    traversal: GraphTraversal = GraphTraversal("preorder-right-to-left"),
    node_formatter: typing.Callable[[typing.Any], str] = str,
) -> list[typing.Any]:
    """
    Performs a depth first traversal of `graph_nodes`, beginning
    with the `roots` and queuing the nodes returned by `get_nodes_to_traverse_func`.
    Returns a list of all visisted nodes.

    get_nodes_to_traverse_func(node: '_a') -> ['_a']:

    Starlark does not offer while loops, so this implementation
    must make use of a for loop.
    """

    # Note, this relies on the Starlark guarantee that set() returns values in the order they were first added.
    visited = set(roots)
    stride = -1 if traversal == GraphTraversal("preorder-left-to-right") else 1
    stack = reversed(visited) if stride < 0 else list(visited)

    for _ in range(len(graph_nodes) if graph_nodes else 2000000000):
        if not stack:
            break
        node = stack.pop()
        if graph_nodes and node not in graph_nodes:
            fail("Expected node {} in graph nodes".format(node_formatter(node)))
        nodes_to_visit = get_nodes_to_traverse_func(node)
        if nodes_to_visit:
            # This is supposed to be equivalent to `for node in nodes_to_visit[::stride]:`
            # Unfortunately, Starlark allocates new arrays for slices.
            #
            # Deriving the indexes via `range()` helps alleviate the memory
            # usage of this function.
            range_traversal = range(len(nodes_to_visit) - 1, -1, -1) if stride == -1 else range(len(nodes_to_visit))
            for i in range_traversal:
                node = nodes_to_visit[i]
                if node not in visited:
                    visited.add(node)
                    stack.append(node)

    expect(not stack, "Expected to be done with graph traversal stack.")

    return list(visited)

# To support migration from a tset-based link strategy, we are trying to match buck's internal tset
# traversal logic here.  Look for implementation of TopologicalTransitiveSetIteratorGen
def rust_matching_topological_traversal(
    graph_nodes: [dict[typing.Any, typing.Any], None], roots: typing.Iterable, get_nodes_to_traverse_func: typing.Callable
) -> list[typing.Any]:
    counts = {}

    for label in depth_first_traversal_by(graph_nodes, roots, get_nodes_to_traverse_func, GraphTraversal("preorder-right-to-left")):
        for dep in get_nodes_to_traverse_func(label):
            if dep in counts:
                counts[dep] += 1
            else:
                counts[dep] = 1

    # some of the targets in roots might be transitive deps of others, we only put those that are true roots
    # in the stack at this point
    stack = [root_target for root_target in roots if not root_target in counts]
    true_roots = len(stack)

    result = []
    for _ in range(2000000000):
        if not stack:
            break
        next = stack.pop()
        result.append(next)
        deps = get_nodes_to_traverse_func(next)
        for child in deps[::-1]:  # reverse order ensures we put things on the stack in the same order as rust's tset traversal
            counts[child] -= 1
            if counts[child] == 0:
                stack.append(child)

    if len(result) != true_roots + len(counts):
        fail()  # fail_cycle

    return result
