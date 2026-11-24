# @generated
# To regenerate, run:
# ```
# STARLARK_RUST_REGENERATE_GOLDEN_TESTS=1 cargo test -p starlark --lib
# ```

# Load

What a scale takes: something that can be weighed, or a list of these.

## Definition

```python
Load = list[Load] | Weighable
```
