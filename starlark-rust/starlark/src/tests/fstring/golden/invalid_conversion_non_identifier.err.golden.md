# @generated
# To regenerate, run:
# ```
# STARLARK_RUST_REGENERATE_GOLDEN_TESTS=1 cargo test -p starlark --lib
# ```

error: invalid f-string conversion specifier, expected 's' or 'r'
 --> assert.bzl:1:5
  |
1 | f'{x!1}'
  |     ^
