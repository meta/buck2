Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

Full E2E: messy unsorted loads with unused symbols get cleaned and formatted
  $ cat <<'EOF' > messy_loads.bzl
  > load(   "//z:z.bzl"   ,    "z_unused"   ,   "z_c"   ,   "z_a"   )
  > load(  "//a:a.bzl"  ,   "a_unused"   ,   "a_b"  ,    "a_a"   )
  > load(    "@external//ext:ext.bzl"    ,     "ext_used"    ,    "ext_unused"    )
  > load(   "//m:m.bzl"  ,    "m_only"   )
  > result   =   a_a(   a_b(   z_a(   z_c(   ext_used(   )   )   )   )   )
  > EOF
  $ starlark-fmt messy_loads.bzl
   INFO process_file: messy_loads.bzl: formatted
  $ cat messy_loads.bzl
  load("@external//ext:ext.bzl", "ext_used")
  load("//a:a.bzl", "a_a", "a_b")
  load("//z:z.bzl", "z_a", "z_c")
  
  result = a_a(a_b(z_a(z_c(ext_used()))))


Keyword args sorted after positional, unused removed
  $ cat <<'EOF' > keyword_messy.bzl
  > load(   ":lib.bzl"   ,   "c_func"   ,   "a_func"   ,   "unused_func"   ,   a_alias   =   "a_orig"   ,   z_alias   =   "z_orig"   )
  > x   =   a_func(   c_func(   a_alias(   z_alias(   )   )   )   )
  > EOF
  $ starlark-fmt keyword_messy.bzl
   INFO process_file: keyword_messy.bzl: formatted
  $ cat keyword_messy.bzl
  load(":lib.bzl", "a_func", "c_func", a_alias = "a_orig", z_alias = "z_orig")
  x = a_func(c_func(a_alias(z_alias())))

Multiple load groups with interleaved code - loads moved to top
  $ cat <<'EOF' > multi_group_messy.bzl
  > load(    "//d:d.bzl"   ,    "d_func"   ,   "d_unused"   )
  > load(   "//b:b.bzl"  ,   "b_func"  )
  > first_result   =   b_func(   d_func(   )   )
  > load(  "//z:z.bzl"   ,    "z_func"   )
  > load(   "//x:x.bzl"  ,  "x_func"  ,  "x_unused"  )
  > second_result   =   x_func(   z_func(   )   )
  > EOF
  $ starlark-fmt multi_group_messy.bzl
   INFO process_file: multi_group_messy.bzl: formatted
  $ cat multi_group_messy.bzl
  load("//b:b.bzl", "b_func")
  load("//d:d.bzl", "d_func")
  load("//x:x.bzl", "x_func")
  load("//z:z.bzl", "z_func")
  
  first_result = b_func(d_func())
  second_result = x_func(z_func())

Entire load removed when all symbols unused
  $ cat <<'EOF' > all_unused_messy.bzl
  > load(   "//used:used.bzl"   ,    "used_func"   )
  > load(  "//unused:unused.bzl"  ,   "totally_unused"   ,   "also_unused"   )
  > result   =   used_func(   "arg"   )
  > EOF
  $ starlark-fmt all_unused_messy.bzl
   INFO process_file: all_unused_messy.bzl: formatted
  $ cat all_unused_messy.bzl
  load("//used:used.bzl", "used_func")
  
  result = used_func("arg")





Complex rule calls with messy formatting
  $ cat <<'EOF' > complex_rules.bzl
  > load(   "//rules:rules.bzl"   ,    "my_rule"   ,   "helper"   ,   "unused_rule"   )
  > load(  "//lib:lib.bzl"  ,   "lib_func"  )
  > my_rule(
  >     name   =   "example"   ,
  >     srcs   =   [   "a.txt"   ,   "b.txt"   ]   ,
  >     deps   =   [   helper(   lib_func(   )   )   ]   ,
  > )
  > EOF
  $ starlark-fmt complex_rules.bzl
   INFO process_file: complex_rules.bzl: formatted
  $ cat complex_rules.bzl
  load("//lib:lib.bzl", "lib_func")
  load("//rules:rules.bzl", "helper", "my_rule")
  my_rule(
      name = "example",
      srcs = ["a.txt", "b.txt"],
      deps = [helper(lib_func())],
  )

Buildifier sort order: external (@), then cell-path, then relative (:)
  $ cat <<'EOF' > relative_loads.bzl
  > load(   ":local.bzl"   ,    "local_func"   )
  > load(  "//path:defs.bzl"  ,   "path_func"  )
  > load(   "//aaa:aaa.bzl"   ,    "aaa_func"   )
  > result   =   local_func(   path_func(   aaa_func(   )   )   )
  > EOF
  $ starlark-fmt relative_loads.bzl
   INFO process_file: relative_loads.bzl: formatted
  $ cat relative_loads.bzl
  load("//aaa:aaa.bzl", "aaa_func")
  load("//path:defs.bzl", "path_func")
  load(":local.bzl", "local_func")
  result = local_func(path_func(aaa_func()))

External @fbsource loads sorted before cell-path, before relative
  $ cat <<'EOF' > fbsource_loads.bzl
  > load(   ":SOCKETS.bzl"   ,    "SOCKET_A"   )
  > load(  "@fbsource//tools:defs.bzl"  ,   "tool_func"  )
  > load(   "//lib:lib.bzl"   ,    "lib_func"   )
  > load(  "@external//ext:ext.bzl"  ,   "ext_func"  )
  > result   =   lib_func(   tool_func(   ext_func(   SOCKET_A(   )   )   )   )
  > EOF
  $ starlark-fmt fbsource_loads.bzl
   INFO process_file: fbsource_loads.bzl: formatted
  $ cat fbsource_loads.bzl
  load("@external//ext:ext.bzl", "ext_func")
  load("@fbsource//tools:defs.bzl", "tool_func")
  load("//lib:lib.bzl", "lib_func")
  load(":SOCKETS.bzl", "SOCKET_A")
  result = lib_func(tool_func(ext_func(SOCKET_A())))

Inline comments preserved when sorting loads
  $ cat <<'EOF' > inline_comments.bzl
  > load("//z:z.bzl", "z_used")  # z comment
  > load("//a:a.bzl", "a_used")  # a comment
  > result = a_used(z_used())
  > EOF
  $ starlark-fmt inline_comments.bzl
   INFO process_file: inline_comments.bzl: formatted
  $ cat inline_comments.bzl
  load("//a:a.bzl", "a_used")  # a comment
  load("//z:z.bzl", "z_used")  # z comment
  result = a_used(z_used())

Leading comments preserved when sorting loads
  $ cat <<'EOF' > leading_comments.bzl
  > # Comment for z
  > load("//z:z.bzl", "z_used")
  > # Comment for a
  > load("//a:a.bzl", "a_used")
  > result = a_used(z_used())
  > EOF
  $ starlark-fmt leading_comments.bzl
   INFO process_file: leading_comments.bzl: formatted
  $ cat leading_comments.bzl
  # Comment for a
  load("//a:a.bzl", "a_used")
  # Comment for z
  load("//z:z.bzl", "z_used")
  result = a_used(z_used())

Mixed inline and leading comments preserved when sorting
  $ cat <<'EOF' > mixed_comments.bzl
  > # Z module provides z_func
  > load("//z:z.bzl", "z_func")  # important!
  > # A module provides a_func
  > load("//a:a.bzl", "a_func")  # also important
  > result = a_func(z_func())
  > EOF
  $ starlark-fmt mixed_comments.bzl
   INFO process_file: mixed_comments.bzl: formatted
  $ cat mixed_comments.bzl
  # A module provides a_func
  load("//a:a.bzl", "a_func")  # also important
  # Z module provides z_func
  load("//z:z.bzl", "z_func")  # important!
  result = a_func(z_func())

Unsorted loads with unused symbols and inline comments
  $ cat <<'EOF' > unsorted_unused_inline.bzl
  > load("//z:z.bzl", "z_used", "z_unused")  # z stuff
  > load("//a:a.bzl", "a_used", "a_unused")  # a stuff
  > result = a_used(z_used())
  > EOF
  $ starlark-fmt unsorted_unused_inline.bzl
   INFO process_file: unsorted_unused_inline.bzl: formatted
  $ cat unsorted_unused_inline.bzl
  load("//a:a.bzl", "a_used")  # a stuff
  load("//z:z.bzl", "z_used")  # z stuff
  result = a_used(z_used())

Unsorted loads with unused symbols and leading comments
  $ cat <<'EOF' > unsorted_unused_leading.bzl
  > # Z module - we use z_used
  > load("//z:z.bzl", "z_used", "z_unused")
  > # A module - we use a_used
  > load("//a:a.bzl", "a_used", "a_unused")
  > result = a_used(z_used())
  > EOF
  $ starlark-fmt unsorted_unused_leading.bzl
   INFO process_file: unsorted_unused_leading.bzl: formatted
  $ cat unsorted_unused_leading.bzl
  # A module - we use a_used
  load("//a:a.bzl", "a_used")
  # Z module - we use z_used
  load("//z:z.bzl", "z_used")
  result = a_used(z_used())

Multiline load with inline comments on symbols - unused removed, comments preserved
  $ cat <<'EOF' > multiline_inline_comments.bzl
  > load(
  >     "//lib:lib.bzl",
  >     "used_a",  # important function
  >     "unused_b",  # this one is not used
  >     "used_c",  # another important one
  > )
  > result = used_a(used_c())
  > EOF
  $ starlark-fmt multiline_inline_comments.bzl
   INFO process_file: multiline_inline_comments.bzl: formatted
  $ cat multiline_inline_comments.bzl
  load(
      "//lib:lib.bzl",
      "used_a",  # important function
      "used_c",  # another important one
  )
  result = used_a(used_c())

Multiline load with leading comments on symbols - unused removed, comments preserved
  $ cat <<'EOF' > multiline_leading_comments.bzl
  > load(
  >     "//lib:lib.bzl",
  >     # Function A is critical
  >     "used_a",
  >     # Function B is not needed
  >     "unused_b",
  >     # Function C is also critical
  >     "used_c",
  > )
  > result = used_a(used_c())
  > EOF
  $ starlark-fmt multiline_leading_comments.bzl
   INFO process_file: multiline_leading_comments.bzl: formatted
  $ cat multiline_leading_comments.bzl
  load(
      "//lib:lib.bzl",
      # Function A is critical
      "used_a",
      # Function C is also critical
      "used_c",
  )
  result = used_a(used_c())

Complex: multiple unsorted loads with mixed comments and unused symbols
  $ cat <<'EOF' > complex_comments.bzl
  > # Utilities from z module
  > load("//z:z.bzl", "z_used", "z_unused")  # z utilities
  > # Core functions from m module
  > load("//m:m.bzl", "m_used")  # the m func
  > # Helpers from a module
  > load("//a:a.bzl", "a_used", "a_helper", "a_unused")  # a helpers
  > result = a_used(m_used(z_used()))
  > EOF
  $ starlark-fmt complex_comments.bzl
   INFO process_file: complex_comments.bzl: formatted
  $ cat complex_comments.bzl
  # Helpers from a module
  load("//a:a.bzl", "a_used")  # a helpers
  # Core functions from m module
  load("//m:m.bzl", "m_used")  # the m func
  # Utilities from z module
  load("//z:z.bzl", "z_used")  # z utilities
  result = a_used(m_used(z_used()))

Multiline loads reordered with leading comments (multiline loads with leading comments are preserved as-is)
  $ cat <<'EOF' > multiline_reorder.bzl
  > # Z module loads
  > load("//z:z.bzl", "z_func")
  > # A module loads
  > load("//a:a.bzl", "a_func")
  > result = a_func(z_func())
  > EOF
  $ starlark-fmt multiline_reorder.bzl
   INFO process_file: multiline_reorder.bzl: formatted
  $ cat multiline_reorder.bzl
  # A module loads
  load("//a:a.bzl", "a_func")
  # Z module loads
  load("//z:z.bzl", "z_func")
  result = a_func(z_func())

@unused annotation preserves symbol even when not referenced
  $ cat <<'EOF' > unused_annotation.bzl
  > load(
  >     "//lib:lib.bzl",
  >     "used_func",
  >     "kept_for_type",  # @unused - needed for type checking
  >     "actually_unused",
  > )
  > result = used_func()
  > EOF
  $ starlark-fmt unused_annotation.bzl
   INFO process_file: unused_annotation.bzl: formatted
  $ cat unused_annotation.bzl
  load(
      "//lib:lib.bzl",
      "kept_for_type",  # @unused - needed for type checking
      "used_func",
  )
  result = used_func()

Sandwich pattern: a same-origin merged load between two kept loads must not bleed into output
  $ cat <<'EOF' > sandwich_loads.bzl
  > load("//a:a.bzl", "x")
  > load("//a:a.bzl", "z")
  > load("//b:b.bzl", "y")
  > result = x(z(y()))
  > EOF
  $ starlark-fmt sandwich_loads.bzl
   INFO process_file: sandwich_loads.bzl: formatted
  $ cat sandwich_loads.bzl
  load("//a:a.bzl", "x", "z")
  load("//b:b.bzl", "y")
  result = x(z(y()))

Move loads to top: loads after statements are moved to top
  $ cat <<'EOF' > move_loads.bzl
  > x = 1
  > load("//a:a.bzl", "a")
  > y = a()
  > load("//b:b.bzl", "b")
  > z = b()
  > EOF
  $ starlark-fmt move_loads.bzl
   INFO process_file: move_loads.bzl: formatted
  $ cat move_loads.bzl
  load("//a:a.bzl", "a")
  load("//b:b.bzl", "b")
  
  x = 1
  y = a()
  z = b()

Move loads preserves docstring at top
  $ cat <<'EOF' > move_with_docstring.bzl
  > """Module docstring."""
  > x = 1
  > load("//a:a.bzl", "a")
  > y = a()
  > EOF
  $ starlark-fmt move_with_docstring.bzl
   INFO process_file: move_with_docstring.bzl: formatted
  $ cat move_with_docstring.bzl
  """Module docstring."""
  
  load("//a:a.bzl", "a")
  
  x = 1
  y = a()




Move loads preserves workspace() call at top
  $ cat <<'EOF' > move_with_workspace.bzl
  > workspace(name = "my_workspace")
  > x = 1
  > load("//a:a.bzl", "a")
  > y = a()
  > EOF
  $ starlark-fmt move_with_workspace.bzl
   INFO process_file: move_with_workspace.bzl: formatted
  $ cat move_with_workspace.bzl
  workspace(name = "my_workspace")
  load("//a:a.bzl", "a")
  
  x = 1
  y = a()

Move loads preserves leading comments
  $ cat <<'EOF' > move_with_comments.bzl
  > # This is a comment
  > x = 1
  > # Load comment
  > load("//a:a.bzl", "a")
  > y = a()
  > EOF
  $ starlark-fmt move_with_comments.bzl
   INFO process_file: move_with_comments.bzl: formatted
  $ cat move_with_comments.bzl
  # This is a comment
  # Load comment
  load("//a:a.bzl", "a")
  
  x = 1
  y = a()

Disable comment prevents moving loads
  $ cat <<'EOF' > disable_move.bzl
  > # buildifier: disable=load-on-top
  > x   =   1
  > load("//a:a.bzl", "a")
  > y = a()
  > EOF
  $ starlark-fmt disable_move.bzl
   INFO process_file: disable_move.bzl: formatted
  $ cat disable_move.bzl
  # buildifier: disable=load-on-top
  x = 1
  load("//a:a.bzl", "a")
  y = a()

Move loads to top then merge: loads from same file in different positions get merged after moving
  $ cat <<'EOF' > move_and_merge.bzl
  > x = 1
  > load("//a:a.bzl", "a_func")
  > y = a_func()
  > load("//b:b.bzl", "b_func")
  > z = b_func()
  > load("//a:a.bzl", "a_helper")
  > w = a_helper()
  > EOF
  $ starlark-fmt move_and_merge.bzl
   INFO process_file: move_and_merge.bzl: formatted
  $ cat move_and_merge.bzl
  load("//a:a.bzl", "a_func", "a_helper")
  load("//b:b.bzl", "b_func")
  
  x = 1
  y = a_func()
  z = b_func()
  w = a_helper()
