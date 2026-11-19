Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

Empty file produces empty output
  $ touch empty.bzl
  $ starlark-fmt empty.bzl
  $ cat empty.bzl

CRLF line endings are normalized to LF
  $ printf 'x   =   1\r\ny   =   2\r\n' > crlf.bzl
  $ starlark-fmt crlf.bzl
   INFO process_file: crlf.bzl: formatted
  $ cat crlf.bzl
  x = 1
  y = 2
  $ od -c crlf.bzl | grep -c '\\r'
  0
  [1]

Invalid UTF-8 produces error
  $ printf 'x = "\xff\xfe"' > invalid_utf8.bzl
  $ starlark-fmt invalid_utf8.bzl 2>&1
  ERROR invalid_utf8.bzl: stream did not contain valid UTF-8
  ERROR 1 file failed to format
  [1]

UTF-8 characters in strings are preserved
  $ cat <<'EOF' > utf8_chars.bzl
  > EMOJI   =   "🎉🚀"
  > CJK   =   "你好世界"
  > MIXED   =   {   "キー"   :   "値"   ,   "clé"   :   "valeur"   }
  > EOF
  $ starlark-fmt utf8_chars.bzl
   INFO process_file: utf8_chars.bzl: formatted
  $ cat utf8_chars.bzl
  EMOJI = "\xf0\x9f\x8e\x89\xf0\x9f\x9a\x80" (esc)
  CJK = "\xe4\xbd\xa0\xe5\xa5\xbd\xe4\xb8\x96\xe7\x95\x8c" (esc)
  MIXED = {"cl\xc3\xa9": "valeur", "\xe3\x82\xad\xe3\x83\xbc": "\xe5\x80\xa4"} (esc)

Broken syntax shows filepath in error
  $ cat <<'EOF' > broken.bzl
  > x = {
  > EOF
  $ starlark-fmt broken.bzl 2>&1
  ERROR broken.bzl:2:1: failed to parse module: unexpected EOF while parsing
  ERROR 1 file failed to format
  [1]

fmt:off and fmt:on disable formatting for a region (including autofixes)
  $ cat <<'EOF' > fmt_off.bzl
  > x   =   1
  > # fmt: off
  > y    =    {    "zebra"   :   1   ,   "apple"   :   2   }
  > z    =    [    3   ,   2   ,   1    ]
  > # fmt: on
  > w   =   4
  > EOF
  $ starlark-fmt fmt_off.bzl
   INFO process_file: fmt_off.bzl: formatted
  $ cat fmt_off.bzl
  x = 1
  # fmt: off
  y    =    {    "zebra"   :   1   ,   "apple"   :   2   }
  z    =    [    3   ,   2   ,   1    ]
  # fmt: on
  w = 4

Idempotency: formatting twice produces byte-identical output. Covers
each autofix (dict-key sort, list-arg sort, load sort + symbol sort +
same-origin merge, BZL # keep sorted, blank-line collapse, kwarg spaces,
triple-quoted string preservation).
  $ cat <<'EOF' > idempotent_BUCK
  > load(   "//z:z.bzl"   ,    "z_unused"   ,   "z_c"   ,   "z_a"   )
  > load(  "//a:a.bzl"  ,   "a_b"  ,    "a_a"   )
  > load("//a:a.bzl", "a_extra")
  > load(    "@external//ext:ext.bzl"    ,     "ext_used"    )
  > 
  > MULTI = """triple
  > quoted
  > string"""
  > 
  > MAPPING = {"zebra": 1, "apple": 2, "DEFAULT": 99, "mango": 3}
  > 
  > my_rule(
  >     name   =   "thing"   ,
  >     deps   =   [   ":z"   ,   ":a"   ,   ":m"   ]   ,
  >     srcs   =   [   "z.rs"   ,   "a.rs"   ]   ,
  > )
  > 
  > result   =   a_a(   a_b(   z_a(   z_c(   ext_used(   )   )   )   )   )
  > extra   =   a_extra(   )
  > EOF
  $ starlark-fmt idempotent_BUCK
   INFO process_file: idempotent_BUCK: formatted
  $ cp idempotent_BUCK idempotent.first
  $ starlark-fmt idempotent_BUCK
  $ cmp idempotent_BUCK idempotent.first

Idempotency for .bzl files (no rule-arg sort, but # keep sorted lists,
loads, dicts, blank-line collapse, and triple-quoted strings still apply).
  $ cat <<'EOF' > idempotent.bzl
  > load(   "//z:z.bzl"   ,    "z_c"   ,   "z_a"   )
  > load(  "//a:a.bzl"  ,   "a_b"  ,    "a_a"   )
  > load("//a:a.bzl", "a_extra")
  > 
  > # keep sorted
  > KEEP_SORTED = [
  >     "z",
  >     "a",
  >     "m",
  > ]
  > 
  > MULTI = """triple
  > quoted
  > string"""
  > 
  > MAPPING = {"zebra": 1, "apple": 2, "DEFAULT": 99, "mango": 3}
  > 
  > result = a_a(a_b(z_a(z_c(a_extra()))))
  > EOF
  $ starlark-fmt idempotent.bzl
   INFO process_file: idempotent.bzl: formatted
  $ cp idempotent.bzl idempotent_bzl.first
  $ starlark-fmt idempotent.bzl
  $ cmp idempotent.bzl idempotent_bzl.first
