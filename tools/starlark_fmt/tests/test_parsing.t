Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1

Passing a directory exits non-zero
  $ mkdir mydir
  $ starlark-fmt mydir 2>&1
  ERROR mydir: path is a directory, expected a file
  ERROR 1 file failed to format
  [1]

Nonexistent file exits non-zero
  $ starlark-fmt nonexistent.bzl 2>&1
  ERROR nonexistent.bzl: * (glob)
  ERROR 1 file failed to format
  [1]

Valid files are still formatted when bad ones are mixed in
  $ cat <<'EOF' > good.bzl
  > load("//a:a.bzl", "a")
  > EOF
  $ starlark-fmt good.bzl nonexistent.bzl 2>&1 | sort
   INFO process_file: good.bzl: formatted
  ERROR 1 file failed to format
  ERROR nonexistent.bzl: * (glob)
  [1]
  $ cat good.bzl
  

Multiple bad files reports all errors
  $ starlark-fmt no1.bzl no2.bzl no3.bzl 2>&1 | sort
  ERROR 3 files failed to format
  ERROR no1.bzl: * (glob)
  ERROR no2.bzl: * (glob)
  ERROR no3.bzl: * (glob)
  [1]

Directory mixed with valid file still formats the valid file
  $ cat <<'EOF' > ok.bzl
  > load("//ok:ok.bzl", "ok")
  > EOF
  $ starlark-fmt ok.bzl mydir 2>&1 | sort
   INFO process_file: ok.bzl: formatted
  ERROR 1 file failed to format
  ERROR mydir: path is a directory, expected a file
  [1]
  $ cat ok.bzl
  

No arguments exits non-zero with usage
  $ starlark-fmt 2>&1
  error: the following required arguments were not provided:
    <FILE>...
  
  Usage: starlark_fmt --config <CONFIG_PATH> fmt <FILE>...
  
  For more information, try '--help'.
  [2]



All valid files exits zero
  $ cat <<'EOF' > a.bzl
  > load("//a:a.bzl", "a")
  > EOF
  $ cat <<'EOF' > b.bzl
  > load("//b:b.bzl", "b")
  > EOF
  $ starlark-fmt a.bzl b.bzl
   INFO process_file: *.bzl: formatted (glob)
   INFO process_file: *.bzl: formatted (glob)
  $ echo $?
  0

Broken syntax shows filepath in error message
  $ cat <<'EOF' > broken.bzl
  > x = {
  > EOF
  $ starlark-fmt broken.bzl 2>&1
  ERROR broken.bzl:2:1: failed to parse module: unexpected EOF while parsing
  ERROR 1 file failed to format
  [1]

Multiple files with one broken still formats the valid ones
  $ cat <<'EOF' > valid.bzl
  > x   =   1
  > EOF
  $ starlark-fmt valid.bzl broken.bzl 2>&1 | sort
   INFO process_file: valid.bzl: formatted
  ERROR 1 file failed to format
  ERROR broken.bzl:2:1: failed to parse module: unexpected EOF while parsing
  [1]
  $ cat valid.bzl
  x = 1
