  $ source "$TESTDIR/setup.sh"

Basic formatting via stdin
  $ echo 'x = {"b": 1, "a": 2}' | starlark-stdin --path=test.bzl
  x = {"a": 2, "b": 1}

Preserves whitespace in formatted output
  $ printf 'x = 1\n\ny = 2' | starlark-stdin --path=test.bzl
  x = 1
  
  y = 2

Empty input produces empty output
  $ echo -n '' | starlark-stdin --path=test.bzl

Unused loads are removed
  $ printf 'load("@foo", "unused")\nx = 1' | starlark-stdin --path=test.bzl
  x = 1

Inline sort directives canonicalize non-literal list elements
  $ cat <<'EOF' | starlark-stdin --path=test.bzl
  > my_rule(
  >     # starlark-fmt: sort-by = {"first_of": [{"call_keyword": "name"}, "call_name"]}
  >     custom_items = [zebra(), factory(name = "alpha"), module.bravo()],
  > )
  > EOF
  my_rule(
      # starlark-fmt: sort-by = {"first_of": [{"call_keyword": "name"}, "call_name"]}
      custom_items = [factory(name = "alpha"), module.bravo(), zebra()],
  )

Configured sort keys apply even when ordinary list sorting is disabled
  $ cat > sort_config.json <<'EOF'
  > {
  >   "IsSortableListArg": {},
  >   "SortableBlacklist": {},
  >   "NamePriority": {},
  >   "Overrides": [{"Files": ["*.bzl"], "SortListArgs": false}],
  >   "ListSortKeys": {
  >     "custom_items": {"first_of": [{"call_keyword": "name"}, "call_name"]}
  >   }
  > }
  > EOF
  $ printf 'my_rule(custom_items=[zebra(), factory(name="alpha")])\n' | "$STARLARK_FMT_PATH" --config sort_config.json stdin --path=test.bzl
  my_rule(custom_items = [factory(name = "alpha"), zebra()])

Path argument is required
  $ echo 'x = {"b": 1, "a": 2}' | starlark-stdin 2>&1
  error: the following required arguments were not provided:
    --path <FILE>
  
  Usage: starlark_fmt --config <CONFIG_PATH> stdin --path <FILE>
  
  For more information, try '--help'.
  [2]

Broken syntax produces error with path context
  $ echo 'x = {' | starlark-stdin --path=test.bzl 2>&1
  \x1b[31mERROR\x1b[0m test.bzl:2:1: failed to parse module: unexpected EOF while parsing (esc)
  [1]

Formats code that needs formatting
  $ printf 'def foo(a,b,c):\n    x=1\n    y=2\n    return x+y' | starlark-stdin --path=test.bzl
  def foo(a, b, c):
      x = 1
      y = 2
      return x + y
