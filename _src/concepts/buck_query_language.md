---
id: buck_query_language
title: Buck Query Language
---

# Buck Query Language

Buck2's query language provides a powerful way to inspect and analyze the build
graph. The query language is shared across different query commands
(`buck2 uquery`, `buck2 cquery`, and `buck2 aquery`), though each command
operates on different graph representations and supports different sets of
operators.

## Query Parameters

The most common parameter for a Buck query operator is an expression that
evaluates to a build target or collection of build targets. Such an expression
could be:

- An explicit [build target](build_target.md)
- A [build target pattern](target_pattern.md)
- A [.buckconfig alias](buckconfig.md)
- The set of targets returned by another Buck query operator

**Tip:** You can pass an alias directly to the `buck2 query` command line to see
what it resolves to. For example:

```sh
buck2 uquery app
```

### Non-target Parameters

In addition to target parameters, some Buck query operators take string
parameters such as filenames (`owner()`) or regular expressions (`filter()`).

**Note:** Hover over parameters in query operator syntax to see their
descriptions in the command documentation.

## Quoting Arguments

It is not necessary to quote arguments if they comprise sequences of characters
drawn from the alphabet, numerals, forward slash (`/`), colon (`:`), period
(`.`), hyphen (`-`), underscore (`_`), or asterisk (`*`)—and they do not start
with a hyphen or period. For example, quoting `java_test` is unnecessary.

However, we **do recommend** that you quote arguments as a best practice even
when Buck2 doesn't require it.

You should always use quotes when writing scripts that construct `buck2 query`
expressions from user-supplied values.

Note that argument quoting for `buck2 query` is in addition to any quoting that
your shell requires. In the following example, double-quotes are used for the
shell and single-quotes are used for the build target expression:

```sh
buck2 uquery "'//foo:bar=wiz'"
```

## Algebraic Set Operations

Buck2's query language supports algebraic set operations for combining query
results.

### Set Operations: intersection, union, set difference

| Nominal     | Symbolic |
| ----------- | -------- |
| `intersect` | `^`      |
| `union`     | `+`      |
| `except`    | `-`      |

These three operators compute the corresponding set operations over their
arguments. Each operator has two forms: a nominal form (e.g., `intersect`) and a
symbolic form (e.g., `^`). The two forms are equivalent; the symbolic forms are
just faster to type.

For example:

```sh
buck2 uquery "deps('//foo:bar') intersect deps('//baz:lib')"
```

and

```sh
buck2 uquery "deps('//foo:bar') ^ deps('//baz:lib')"
```

both return the targets that appear in the transitive closure of `//foo:bar` and
`//baz:lib`.

**Properties:**

- The `intersect` (`^`) and `union` (`+`) operators are commutative
- The `except` (`-`) operator is not commutative
- The parser treats all three operators as left-associative and of equal
  precedence

We recommend that you use parentheses if you need to ensure a specific order of
evaluation. A parenthesized expression resolves to the value of the expression
it encloses. For example, the first two expressions are equivalent, but the
third is not:

```
x intersect y union z
(x intersect y) union z
x intersect (y union z)
```

### Group Targets: set()

**Syntax:**

```
set(<expr_a> <expr_b> <expr_c> ...)
```

The `set()` operator computes the union of a set of zero or more target
expressions. Separate the targets with white space (not commas). Quote the
targets to ensure they are parsed correctly.

If you want to invoke `buck2 query` on a list of targets, then `set()` is a way
to group this list in a query.

**Example:**

The following command line returns the target `main` in the build file in the
root of the Buck2 project and all the targets from the build file in the
`myclass` subdirectory of the root:

```sh
buck2 uquery "set( '//:main' '//myclass:' )"
```

**Example:**

The following command line returns the merged set (union) of dependencies for
the targets `main` and `subs` in the build file in the root of the Buck2
project:

```sh
buck2 uquery "deps( set( '//:main' '//:subs' ) )"
```

## Executing Multiple Queries at Once

Suppose you want to know the tests associated with a set of targets. This can be
done by combining query operators. For example:

```sh
buck2 cquery "testsof(deps(set('target1' 'target2' 'target3')))"
```

Suppose you now want to know the tests for **each** of these targets; the above
command returns the union of the tests. Instead of executing one query for the
entire set of targets, Buck2's query commands provide a way to repeat a query
with different targets using a single command. To do this, first define the
query expression format and then list the input targets, separated by spaces.
For example:

```sh
buck2 cquery "testsof(deps( %s ))" target1 target2 target3
```

The `%s` in the query expression is replaced by each of the listed targets, and
for each target, the resulting query expression is evaluated. If you add the
`--output-format json` parameter, the result of the command is grouped by input
target; otherwise, as in the previous example using `set()`, the command merges
the results and returns the union of the queries.

This syntax is also useful for subcommands that take arguments that are not
targets, such as `owner()`. Recall that the `set()` operator works only with
targets, but the `owner()` operator takes a filename as its argument:

```sh
buck2 uquery "owner( %s )" main.cpp myclass.cpp myclass.h
```

## Referencing Args Files

When running queries, arguments can be stored in external files, one argument
per line, and referenced with the `@` symbol. This is convenient when the number
of arguments is long or when you want to persist the query input in source
control.

```sh
buck2 cquery "testsof(deps(%s))" @/path/to/args-file
```

If you want to include all the targets in the `@`-file in a single query
execution, you can use the following alternative syntax. Note the addition of
the capital "S" in `%Ss`:

```sh
buck2 cquery "testsof(deps(%Ss))" @/path/to/args-file
```

In the example above, the lines of the file are converted to a set and
substituted for the `%Ss`. In addition, each line's contents are singly quoted.
In the example above, if the args file contains the following:

```
//foo:bar
//foo:baz
```

Then the query expression is equivalent to:

```sh
buck2 cquery "testsof(deps(set('//foo:bar' '//foo:baz')))"
```

## Query Environments

Buck2 provides different query environments that operate on different graph
representations:

- **Uquery (Unconfigured Query)**: Operates on the unconfigured target graph.
  Use this when you want to query targets before configurations are applied. See
  [unconfigured graph](glossary.md#unconfigured-graph) for more details.

- **Cquery (Configured Query)**: Operates on the configured target graph where
  `select()` statements are resolved and configurations are applied. Use this
  when you need to understand the actual build graph. See
  [configured graph](glossary.md#configured-graph) for more details.

- **Aquery (Action Query)**: Operates on the action graph, which represents the
  actual build actions that will be executed. See
  [action graph](glossary.md#action-graph) for more details.

Each query environment supports a different set of operators. Refer to the
specific command documentation for details on which operators are available in
each environment.

## See Also

- [Buck2 Cheat Sheet](../users/cheatsheet.md) for practical query examples
- [Glossary](glossary.md) for definitions of key concepts
- [Target Patterns](target_pattern.md) for more on specifying targets
