# Delegating `visibility()` to a loaded helper is rejected.
load(":helper.bzl", "set_visibility")

set_visibility()

X = 1
