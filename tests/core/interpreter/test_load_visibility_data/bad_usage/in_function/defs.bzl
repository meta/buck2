# `visibility()` must be a top-level call, not made from inside a function.
def _set():
    visibility("private")

_set()

X = 1
