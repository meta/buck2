# A Moving Garbage Collector

This page describes a two-space garbage collector that can deal with cycles.

In Starlark, this pattern is used both when doing a real garbage collection, and
when freezing. For both cases, it starts out with a memory block, which has
pointers referring to things inside it, and ends up with a new memory block with
equivalent pointers inside it. However, only pointers reachable from outside the
original memory block are available in the new memory block. The garbage
collector can deal with cyclic data structures and the time spent is
proportional to the amount of live data in the heap (memory that is dropped is
not even visited).

## A worked example

Given a heap with the following layout:

```bash
X := Data("world")
Y := Data("hello", X, Y)
Z := Data("universe")
```

All of `X`, `Y` and `Z` are memory locations. The `Y` memory location has both
some data of its own (`"hello"`) and two pointers (`X` and `Y` itself).

The pointers from outside the heap into the heap are known as _roots_.

Assuming, in the above example, that `Y` is the only root, then, since `Y` is
used from outside, `Y` must be moved to the new memory block. Consequently, the
data `X` needs to be copied, but `Z` can be dropped.

Following are the required steps for using a garbage collector:

1. To copy `Y`, allocate a reservation in the new heap at `A`. Then, turn `Y`
   into a `Forward(A)` pointer, so that if anyone else in this cycle tries to
   collect `Y` they immediately "forward" to the new value and the data from
   `Y` is grabbed so its pointers can be traversed. That results in the
   following:

   ```bash
   X := Data("world")
   Y := Forward(A)
   Z := Data("universe")

   A := Reservation
   ```

   With `Data("hello", X, Y)` as the current item being processed.

2. Walk the pointers of the current value, performing a garbage collection on
   each of them. To copy `Y`, it can be seen that `Y` points at a `Forward(A)`
   node, so there's no need to do anything. To copy `X`, follow the process
   starting at step 1, but for `X` (which ends up at `B`). Performing that move
   leads to the following:

   ```bash
   X := Forward(B)
   Y := Forward(A)
   Z := Data("universe")

   A := Reservation
   B := Data("world")
   ```

3. Replace all the pointers with the forwarded value, and write it back over the
   reservation in `A`. This gives the following:

   ```bash
   X := Forward(B)
   Y := Forward(A)
   Z := Data("universe")

   A := Data("hello", B, A)
   B := Data("world")
   ```

4. Adjust any roots pointing at `Y` to point at `A` and throw away the original
   heap, which produces the following:

   ```bash
   A := Data("hello", B, A)
   B := Data("world")
   ```

These above four steps successfully garbage collects a cyclic data structure,
while preserving the cycles and getting rid of the unused data.
