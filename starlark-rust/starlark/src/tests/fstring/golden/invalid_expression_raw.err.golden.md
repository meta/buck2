# @generated
# To regenerate, run:
# ```
# STARLARK_RUST_REGENERATE_GOLDEN_TESTS=1 cargo test -p starlark --lib
# ```

error: Parse error: unexpected identifier 'baz', expected f-string expression end '}'
 --> assert.bzl:1:13
  |
1 | fr'foo {bar baz}'
  |             ^^^
