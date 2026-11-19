# @generated
# To regenerate, run:
# ```
# STARLARK_RUST_REGENERATE_GOLDEN_TESTS=1 cargo test -p starlark --lib
# ```

error: Parse error: f-string expression is missing closing `}`
 --> assert.bzl:1:11
  |
1 | f'foo {bar'
  |           ^
