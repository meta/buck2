# @generated
# To regenerate, run:
# ```
# STARLARK_RUST_REGENERATE_GOLDEN_TESTS=1 cargo test -p starlark --lib
# ```

# scale

## pack

<pre class="language-python"><code>def pack(
    thing: <a to="/path/to/Crate">crate</a>,
) -> <a to="/path/to/Parcel">Parcel</a></code></pre>

---

## weigh

<pre class="language-python"><code>def weigh(
    thing: <a to="/path/to/Weighable">Weighable</a>,
) -> <a to="/path/to/standard/int">int</a></code></pre>

The weight of one thing.

---

## weigh\_all

<pre class="language-python"><code>def weigh_all(
    load: <a to="/path/to/Load">Load</a>,
) -> <a to="/path/to/standard/int">int</a></code></pre>

The weight of everything on the scale.
