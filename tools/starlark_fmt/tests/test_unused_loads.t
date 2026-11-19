Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

Unused load removed and file formatted
  $ cat <<'EOF' > unformatted_unused.bzl
  > load(   "//defs:defs.bzl"  ,    "used"  ,   "unused"  )
  > my_rule(  name  =  "test"  ,   deps  =  [  used(  )  ]  )
  > EOF
  $ starlark-fmt unformatted_unused.bzl
   INFO process_file: unformatted_unused.bzl: formatted
  $ cat unformatted_unused.bzl
  load("//defs:defs.bzl", "used")
  my_rule(name = "test", deps = [used()])

Entire load removed when all symbols unused
  $ cat <<'EOF' > all_unused_unformatted.bzl
  > load(  "//defs:defs.bzl"  ,     "foo"   ,      "bar"   )
  > other_rule(   name  =   "example"  ,    srcs   =   [   "a.txt"  ,   "b.txt"   ]   )
  > EOF
  $ starlark-fmt all_unused_unformatted.bzl
   INFO process_file: all_unused_unformatted.bzl: formatted
  $ cat all_unused_unformatted.bzl
  other_rule(name = "example", srcs = ["a.txt", "b.txt"])

Mixed keyword and positional loads
  $ cat <<'EOF' > mixed_messy.bzl
  > load(    "//lib:lib.bzl"  ,  "unused1"  ,     "used_func"   ,   renamed   =   "also_unused"   )
  > result   =   used_func(    "arg1"  ,     "arg2"    )
  > EOF
  $ starlark-fmt mixed_messy.bzl
   INFO process_file: mixed_messy.bzl: formatted
  $ cat mixed_messy.bzl
  load("//lib:lib.bzl", "used_func")
  result = used_func("arg1", "arg2")

Multiple loads with some entirely unused
  $ cat <<'EOF' > multi_load_messy.bzl
  > load(   "//a:a.bzl"   ,     "a_used"  ,  "a_unused"  )
  > load(  "//b:b.bzl"  ,  "b_all_unused"  )
  > load(    "//c:c.bzl"   ,      "c_used"    )
  > rule1(   name   =   "r1"   ,    dep   =   a_used(   )   )
  > rule2(   name   =   "r2"   ,    dep   =   c_used(   )   )
  > EOF
  $ starlark-fmt multi_load_messy.bzl
   INFO process_file: multi_load_messy.bzl: formatted
  $ cat multi_load_messy.bzl
  load("//a:a.bzl", "a_used")
  load("//c:c.bzl", "c_used")
  rule1(name = "r1", dep = a_used())
  rule2(name = "r2", dep = c_used())

