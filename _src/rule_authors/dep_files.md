---
id: dep_files
title: Dep Files
---

Dep files allow commands to declare which subset of their inputs were used when
the command executed.

When a command produces a dep file and is later invalidated due to an inputs
change, Buck2 uses the dep file to check whether the inputs that changed were in
the set that the command reported as having used. If none of the inputs that
changed were in that set, Buck2 omits re-running the command and reuses the
previous result.

## Use Cases

Dep files are used to make dependencies finer grained than what exists in the
target graph, but they're not a substitute for avoiding unused dependencies.
They're often useful when targets export many outputs (such as C++ headers) that
aren't all used by all their dependents.

Dep files are currently used to skip recompilation steps in C++ when an unused
header changed. They're also used in Java to skip recompilation when an unused
class changed.

## Using dep files

To use dep files, you need to do the following:

- Declare what output is a dep file and associate it with your command.
- Declare which inputs are covered by the dep file (this can be a subset of your
  inputs).
- Have your command produce the dep file in a format Buck2 can use.

## Declaring the dep files and associating inputs

To declare a dep file and associate it with your command, you need to tag your
artifacts.

Specifically, you'll tag the output (the dep file) and the inputs it covers, as
shown in the following code:

```python
# First, create a tag

headers_tag = ctx.actions.artifact_tag()

# Then, tag inputs and the dep file itself in your command line.
# You do this using the `tag_artifacts` method on your tag.
# This method does not mutate the input, it wraps it, so you use the output.
# Any command-line-arg-like can be tagged.

tagged_headers = headers_tag.tag_artifacts(headers)

dep_file = ctx.actions.declare_output("deps").as_output()
tagged_dep_file = headers_tag.tag_artifacts(dep_file)

# Finally, declare your action.
# Use the tagged artifacts as you would regular command-line-arg-likes.
# Pass the tag in `dep_files` and give a name (this is used for logging).

ctx.actions.run(
  ["mycc", "-I", tagged_headers, "-MD", "-MF", tagged_dep_file, "-o", ...],
  dep_files = { "headers": headers_tag }
)

```

## Producing the dep file

Your command must produce dep files in the format Buck2 expects, which is simply
a list of all the inputs that were used, one per line.

The paths must be the paths Buck2 would use for your inputs, which means paths
relative to the project root.

If this is not the format your tool produces, use a wrapper to take whatever
output your command produces and rewrite it in the format Buck2 expects.

## Testing dep files

When writing a command that produces a dep file, you should test it! At a
minimum, check that the inputs you expect are tagged properly.

To do so, build your target, then use
`buck2 audit dep-files TARGET CATEGORY IDENTIFIER`, which will show you the set
of inputs your command used and how they're tagged.

## Extra notes to the implementer

### Limitations

Dep files only work if a previous invocation of the command is known to your
Buck2 daemon. Dep files are dropped when the daemon restarts or when you run
`buck2 debug flush-dep-files`.

This means that, for example, if you change an unused header, then run a build
on a fresh daemon, Buck2 will still need to execute this command in order to
identify that the header was in fact unused. In contrast, if you did the build
(and got a remote cache hit on the command), then applied your change and
re-built, Buck2 would use the dep file on the second execution, and you wouldn't
need to execute anything.

### Dep files don't need to be covering

It's OK for the dep file to only cover a subset of the inputs of your action.
However, within that subset, the dep file must declare all the inputs that were
used.

If you fail to report some inputs you used, then your command will not re-run
when they change, and you'll get stale output.

### Dep files are lazy

Dep files aren't parsed by Buck2 unless the command needs to re-run. If the
command ran on RE, they aren't even downloaded until then. This ensures dep
files don't cause a performance hit unless they are used, at which point they
stand a chance of giving a performance boost instead.

This means that if you produce an invalid dep file, Buck2 will not report this
until your command runs again, at which point Buck2 will report that the dep
file is invalid and refuse to proceed (note: you can unblock yourself using
`buck2 debug flush-dep-files`).

To flush out issues during development, you can pass `--eager-dep-files` to
Buck2 to force Buck2 to parse your dep files as they are produced.

## Dep files will traverse symlinks

If your dep file reports that a symlink was used, Buck2 will track the symlink's
target as covered by this dep file.

## Remote dep files

Since dep files only work if a previous invocation of the command is known to
your Buck2 daemon, Buck2 also supports "remote dep files". For actions with
`allow_dep_file_cache_upload = True`, Buck2 will upload dep files to the remote
cache. The dep file is keyed on the current version control revision, in
addition to information about the action itself.

For those same actions, Buck2 will look for a "remote dep file", and if it finds
one it will download dep file and use it exactly as it would if it found one
locally (i.e. it compares the inputs to see if only unused inputs have changed
and it can therefore skip the action execution)
