Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

Diff shows only changed lines by default
  $ cat <<'EOF' > messy.bzl
  > load(   "//z:z.bzl"   ,   "z_func"   )
  > load(   "//a:a.bzl"   ,   "a_func"   )
  > result   =   a_func(   z_func(   )   )
  > EOF
  $ starlark-diff messy.bzl
  --- messy.bzl
  -load(   "//z:z.bzl"   ,   "z_func"   )
  -load(   "//a:a.bzl"   ,   "a_func"   )
  -result   =   a_func(   z_func(   )   )
  +load("//a:a.bzl", "a_func")
  +load("//z:z.bzl", "z_func")
  +result = a_func(z_func())

Diff with --show-all includes unchanged lines
  $ cat <<'EOF' > partial.bzl
  > # This comment stays the same
  > load(   "//lib:lib.bzl"   ,   "func"   )
  > # Another unchanged comment
  > result = func()
  > EOF
  $ starlark-diff --show-all partial.bzl
  --- partial.bzl
   # This comment stays the same
  -load(   "//lib:lib.bzl"   ,   "func"   )
  +load("//lib:lib.bzl", "func")
   # Another unchanged comment
   result = func()

Diff reports no changes when file is already formatted
  $ cat <<'EOF' > clean.bzl
  > load("//lib:lib.bzl", "func")
  > result = func()
  > EOF
  $ starlark-diff clean.bzl
   INFO clean.bzl: no changes
