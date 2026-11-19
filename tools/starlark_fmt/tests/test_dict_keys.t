Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

Basic dict key sorting
  $ cat <<'EOF' > unsorted_dict.bzl
  > config   =   {   "zebra"   :   1   ,   "apple"   :   2   ,   "mango"   :   3   }
  > EOF
  $ starlark-fmt unsorted_dict.bzl
   INFO process_file: unsorted_dict.bzl: formatted
  $ cat unsorted_dict.bzl
  config = {"apple": 2, "mango": 3, "zebra": 1}

Dict keys already sorted remain unchanged
  $ cat <<'EOF' > sorted_dict.bzl
  > config   =   {   "a"   :   1   ,   "b"   :   2   ,   "c"   :   3   }
  > EOF
  $ starlark-fmt sorted_dict.bzl
   INFO process_file: sorted_dict.bzl: formatted
  $ cat sorted_dict.bzl
  config = {"a": 1, "b": 2, "c": 3}

Nested dicts all get sorted
  $ cat <<'EOF' > nested_dict.bzl
  > config   =   {   "outer_z"   :   {   "inner_b"   :   2   ,   "inner_a"   :   1   }   ,   "outer_a"   :   {   "deep_z"   :   3   ,   "deep_m"   :   4   }   }
  > EOF
  $ starlark-fmt nested_dict.bzl
   INFO process_file: nested_dict.bzl: formatted
  $ cat nested_dict.bzl
  config = {"outer_a": {"deep_m": 4, "deep_z": 3}, "outer_z": {"inner_a": 1, "inner_b": 2}}

Starred elements preserved in place
  $ cat <<'EOF' > starred_dict.bzl
  > base   =   {   "z_key"   :   1   }
  > config   =   {   "c_key"   :   1   ,   **base   ,   "b_key"   :   2   ,   "a_key"   :   3   }
  > EOF
  $ starlark-fmt starred_dict.bzl
   INFO process_file: starred_dict.bzl: formatted
  $ cat starred_dict.bzl
  base = {"z_key": 1}
  config = {"a_key": 3, **base, "b_key": 2, "c_key": 1}

Non-string keys preserved in position
  $ cat <<'EOF' > variable_key.bzl
  > KEY   =   "dynamic"
  > config   =   {   KEY   :   1   ,   "z_key"   :   2   ,   "a_key"   :   3   }
  > EOF
  $ starlark-fmt variable_key.bzl
   INFO process_file: variable_key.bzl: formatted
  $ cat variable_key.bzl
  KEY = "dynamic"
  config = {KEY: 1, "a_key": 3, "z_key": 2}

@unsorted-dict-items annotation preserves order (inline comment)
  $ cat <<'EOF' > unsorted_inline.bzl
  > config   =   {   # @unsorted-dict-items
  >     "z_key"   :   1   ,
  >     "a_key"   :   2   ,
  > }
  > EOF
  $ starlark-fmt unsorted_inline.bzl
   INFO process_file: unsorted_inline.bzl: formatted
  $ cat unsorted_inline.bzl
  config = {  # @unsorted-dict-items
      "z_key": 1,
      "a_key": 2,
  }

Empty dict unchanged
  $ cat <<'EOF' > empty_dict.bzl
  > config   =   {   }
  > EOF
  $ starlark-fmt empty_dict.bzl
   INFO process_file: empty_dict.bzl: formatted
  $ cat empty_dict.bzl
  config = {}

Single element dict unchanged
  $ cat <<'EOF' > single_dict.bzl
  > config   =   {   "only"   :   1   }
  > EOF
  $ starlark-fmt single_dict.bzl
   INFO process_file: single_dict.bzl: formatted
  $ cat single_dict.bzl
  config = {"only": 1}
