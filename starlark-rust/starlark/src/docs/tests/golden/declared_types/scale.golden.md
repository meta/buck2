# @generated
# To regenerate, run:
# ```
# STARLARK_RUST_REGENERATE_GOLDEN_TESTS=1 cargo test -p starlark --lib
# ```

# scale

## pack

```python
def pack(thing: crate) -> Parcel
```

---

## weigh

```python
def weigh(thing: Weighable) -> int
```

The weight of one thing.

---

## weigh\_all

```python
def weigh_all(load: Load) -> int
```

The weight of everything on the scale.
