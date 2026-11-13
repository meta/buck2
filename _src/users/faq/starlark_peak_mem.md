---
id: starlark_peak_mem
title: Debugging Excess Starlark Peak Memory
---

import { FbInternalOnly, OssOnly } from
'docusaurus-plugin-internaldocs-fb/internal';

## Wut memory?

Peak memory is the maximum amount of memory used during evaluation of that
particular Starlark file. The memory is usually released after we finish the
evaluation of the file. Because Starlark is only garbage collected in between
top-level statements in the BUCK file, but not garbage collected inside function
calls/macros, on large servers with 64 hardware threads (or more), memory usage
might accumulate, causing slowdowns or OOMs <FbInternalOnly> or even SEVs (e.g.
S372092). See
[this post](https://fb.workplace.com/groups/1267349253953900/permalink/1312921066063385/)
for more details on how Starlark's current GC works </FbInternalOnly> .

To prevent such issues until proper GC is implemented, we have set a hard `2GB`
memory limit for Starlark's evaluation of build files. This is a per-file limit.

Note that this is different than the actual process memory which might include
other things apart from Starlark’s evaluation.

## How do I see my build file's peak memory usage?

To see the Starlark peak memory usage of a build file, you can inspect the event
log for your build file. Run a uquery command for your target and then inspect
the event log:

```bash
buck2 uquery //path/to/your:target --output-all-attributes
buck2 log show
```

Here is an example entry from the event log showing that a target uses 1.5GB:

```json
{"Event":{..."data":{"Load":{"module_id":"target:BUCK","cell":"...","error":null,"starlark_peak_allocated_bytes":1610608640}}}}
```

## Profiler to the rescue!

If you want to see more detailed breakdown where the memory is used, you should
profile Starlark's evaluation of build files. See
[this page](../../../rule_authors/optimization/#starlark-profiling) for details
of profiling in the loading stage. This is a great starting point for
troubleshooting.

## How do I reduce memory footprint?

There are many reasons why Starlark's evaluation of your build file might use a
lot of memory. We list a few common cases below but there might be more
cases.<FbInternalOnly> See
[this post](https://fb.workplace.com/groups/buck2eng/permalink/3309329642697846/)
for a few real world examples of debugging Starlark peak memory usage of core
Android macros that have saved over 5.7GB peak memory!</FbInternalOnly>

High level guidance is to pay attention to loops as a starting point. Are there
any unnecessary computations? Can you shave them off?

### Repeatedly allocating memory unnecessarily in a loop

A common case where memory usage might accumulate is repeatedly allocating
memory in a loop. For instance, below we call a memory intensive function in a
loop unnecessarily:

```python
for target in huge_target_list:
    memory_intensive_fun(x,y)
    ...
```

Instead, if we know that arguments `x` and `y` don't change, we could hoist the
call to `memory_intensive_fun` outside of the loop as follows:

```python
memory_intensive_fun(x,y)
for target in huge_target_list:
    ...
```

### Simply allocating very big data-structures!

Another reason why Starlark uses a lot of memory could simply be because the
build file allocates a very big-data structure. For instance, below we allocate
a list with 1 billion integers!

```python
million_list = [1 for i in range(1 << 20)]
billion_list = million_list * (1 << 10)

```

As a workaround, could you think of splitting the list?

### Algorithmically inefficient code

Another reason could be because memory efficiency of your code is bad, i.e. you
are unnecessarily allocating a lot of memory. Let's look at an example where we
try to process a bunch of targets inefficiently:

```python
targets = generate_targets(n)
for target in targets:
    process(target)
```

If `targets` list is big **and** each target takes a lot of space in memory,
memory usage might exceed the limit. Instead, a more efficient version might be
to process each target as you generate it:

```python
for i in range(n):
    target = generate_target(i)
    process(target)
```

In this version, each target is processed as it is generated so we never need to
store more than one target in memory.

### Usage of inefficient library calls

A more subtle reason could be unknowingly invoking library calls that allocate
each time they are called. A well-known case of this is the `dict.items()` call.

```python
for project, version in constraints.items():
    # process each project ....
```

We do an allocation on every call to `constraints.items()`. Especially if this
is a hot code in Starlark, this could cause an OOM. Instead, a potential fix is
to hoist the call out:

```python
constraints = constraints.items()
for project, version in constraints:
    # process each project ....
```

However, you need to ensure that the dictionary is not mutated inside, otherwise
you would get functionally different code. A similar case occurs for
`dict.keys()` where it returns a new list for containing the keys.

### Allocating for rare cases

Finally, another pattern is allocating memory for the rare cases. For instance,
consdier the following example

```python
for target in huge_target_list:
    if memory_intensive_condition([target])
        fail(...)
```

Above program could be optimized as follows:

```python
if memory_intensive_condition(huge_target_list)
    for target in huge_target_list:
        if memory_intensive_condition([target])
            fail(...)
```

so that in the common non-failure case, we don't end up allocating excessive
memory.

## I still need more help!

If you still can not figure out how to reduce Starlark memory footprint of your
build files, <FbInternalOnly>please post in
[Buck2 Users](https://fb.workplace.com/groups/buck2users)</FbInternalOnly><OssOnly>raise
[an issue](https://github.com/facebook/buck2/issues) in our Github
project</OssOnly>.
