# Private, and in package `//lib` (there is no buildfile in `lib/sub`), so
# `//lib` may load it, and it may load `//lib`'s private `priv.bzl`.
load("//lib:priv.bzl", "PRIVSYM")

visibility("private")

HSYM = PRIVSYM
