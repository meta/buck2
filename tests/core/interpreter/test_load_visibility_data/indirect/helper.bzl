# A helper that calls `visibility()` on behalf of its caller. This is not
# allowed: `visibility()` must be called directly in the file it applies to.
def set_visibility():
    visibility(["PUBLIC"])
