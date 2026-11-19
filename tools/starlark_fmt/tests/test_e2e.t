Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

E2E: comprehensive test with multiple transformations and comments
  $ cat <<'EOF' > comprehensive.bzl
  > # Header for z module
  > load(   "//z:z.bzl"  ,  "z_unused"  ,  "z_func"  )  # z inline
  > # Header for a module
  > load(  "//a:a.bzl" ,   "a_func" ,  "a_unused" )  # a inline
  > # Header for unused module - will be removed entirely
  > load( "//unused:unused.bzl"  , "totally_unused" )  # unused inline
  > # Multiline load with symbol comments
  > load(
  >  "//multi:multi.bzl"   ,
  >       "multi_z"  ,  # z symbol comment
  >    "multi_a"  ,  # a symbol comment
  >          "multi_unused"  ,  # unused symbol comment
  > )
  > # List constant with keep sorted
  > SORTED_LIST   =   [  # keep sorted
  >  "zebra"  ,
  >       "apple"  ,
  >    "mango"  ,
  > ]
  > # Rule with unsorted kwargs, dicts, and nested structures
  > my_rule(
  >  name   =   "example"  ,
  >     # Config dict with nested dicts
  >   config   =   {
  >    "outer_z"  :  {  # z outer inline
  >          "inner_b"  :  2  ,
  >       "inner_a"  :  1  ,
  >    }  ,
  >       "outer_a"  :  {  # a outer inline
  >    "deep_z"  :  z_func(  )  ,
  >          "deep_a"  :  a_func(  )  ,
  >       }  ,
  >  }  ,
  >     # Dict that should NOT be sorted
  >   unsorted_config   =   {  # @unsorted-dict-items
  >    "zebra"  :  1  ,
  >       "apple"  :  2  ,
  >          "mango"  :  3  ,
  >  }  ,
  >   deps   =   [
  >      multi_z(  )  ,
  >   multi_a(  )  ,
  >  ]  ,
  > )
  > EOF
  $ starlark-fmt comprehensive.bzl
   INFO process_file: comprehensive.bzl: formatted
  $ cat comprehensive.bzl
  # Header for a module
  load("//a:a.bzl", "a_func")  # a inline
  # Header for unused module - will be removed entirely
  # unused inline
  # Multiline load with symbol comments
  load(
      "//multi:multi.bzl",
      "multi_a",  # a symbol comment
      "multi_z",  # z symbol comment
  )
  # Header for z module
  load("//z:z.bzl", "z_func")  # z inline
  # List constant with keep sorted
  SORTED_LIST = [  # keep sorted
      "apple",
      "mango",
      "zebra",
  ]
  # Rule with unsorted kwargs, dicts, and nested structures
  my_rule(
      name = "example",
      # Config dict with nested dicts
      config = {
          "outer_a": {  # a outer inline
              "deep_a": a_func(),
              "deep_z": z_func(),
          },
          "outer_z": {  # z outer inline
              "inner_a": 1,
              "inner_b": 2,
          },
      },
      # Dict that should NOT be sorted
      unsorted_config = {  # @unsorted-dict-items
          "zebra": 1,
          "apple": 2,
          "mango": 3,
      },
      deps = [
          multi_z(),
          multi_a(),
      ],
  )

E2E: fmt: off/on respects regions - loads inside fmt:off preserved
  $ cat <<'EOF' > fmt_off_test.bzl
  > # This load group should have unused removed
  > load("//z:z.bzl", "z_used", "z_unused")
  > load("//a:a.bzl", "a_used")
  > # fmt: off
  > # These loads should NOT have unused removed
  > load("//z:protected.bzl", "z_protected_unused")
  > load("//a:protected.bzl", "a_protected_unused")
  > # fmt: on
  > # This load should have unused removed
  > load("//after:after.bzl", "after_used", "after_unused")
  > # Use the symbols
  > x = z_used()
  > y = a_used()
  > z = after_used()
  > EOF
  $ starlark-fmt fmt_off_test.bzl
   INFO process_file: fmt_off_test.bzl: formatted
  $ cat fmt_off_test.bzl
  # This load group should have unused removed
  load("//z:z.bzl", "z_used")
  load("//a:a.bzl", "a_used")
  # fmt: off
  # These loads should NOT have unused removed
  load("//z:protected.bzl", "z_protected_unused")
  load("//a:protected.bzl", "a_protected_unused")
  # fmt: on
  # This load should have unused removed
  load("//after:after.bzl", "after_used")
  # Use the symbols
  x = z_used()
  y = a_used()
  z = after_used()


