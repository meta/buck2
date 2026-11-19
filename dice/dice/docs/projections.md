# Projection Computations

DICE supports a special type of synchronous computation called "Projections".
These are synchronous computations that are derived from the result of a larger
parallel async computation.

This allows computations to depend on only "portions" of the result of another
computation, allowing the parent computation to be resurrected more often and
not need be recomputed if only the unused portions of the dependent result
changes. For example, you may have a computation that retrieves and parses JSON.
Now you have an expensive computation that requires a single value of the JSON.
Rather than depending on the entirety of the JSON and having to rerun the
expensive computation whenever any of the JSON value changes, you can write a
new Projection Computation that provides access to specific values from the
JSON. Now you can have the expensive computation depend on the projection, which
will avoid needing to rerun the expensive computation unless that specific
projected value changes.

## API

To create a Projection Computation, create a struct and implement
`ProjectionKey`.

```rust
struct MyProjection;

impl dice::api::ProjectionKey for MyProjection {
    type DeriveFromKey = BaseComputeKey;
    type Value = MyOutput;

    fn compute(
        &self,
        derive_from: &BaseComputeKey::Value,
        _ctx: &DiceProjectionComputations,
    ) -> Self::Value {
        derive_from.get("foo")
    }

    fn equality(x: &Self::Value, y: &Self::Value) -> bool {
        unimplemented!("...")
    }
}
```

The `BaseComputeKey` is the async computation for which the projected values are
based off of.

To request the projection, you must compute the base via
`DiceComputations::compute_opaque(Key)` which returns a `OpaqueValue`. Then,
request the projection via `OpaqueValue::projection(ProjectionKey)`.

Similar to normal keys, buck2 often hides the keys to make code look more
natural via traits.

```rust
#[async_trait]
trait MyComputationTrait {
    /// This style directly if you want to support directly getting a projected value asynchronously from the ctx
    async fn direct_projection(&self, a: i32) -> i32;

    /// This style to create the intermediate opaque value, for which you can have a separate synchronous call for projecting
    async fn opaque_indirection(&self, b: &str) -> OpaqueValue<BaseKey>;
}

/// This style to create the intermediate opaque value, for which you can have a separate synchronous call for projecting
trait SyncProjectionTrait {
    fn sync_projection(&self, b: i32) -> i32;
}

#[async_trait]
impl MyComputationTrait for DiceComputations {
    async fn direct_projection(&self, b: &str) -> i32 {
        self.compute_opaque(BaseComputeKey(b.to_owned())).await.projection(ProjectionA)
    }

    async fn opaque_indirection(&self, b: &str) -> OpaqueValue<BaseKey> {
        self.compute_opaque(BaseComputeKey(a.to_owned()))
    }
}

impl SyncProjectionTrait for OpaqueValue<BaseKey> {
    fn sync_projection(&self, b: i32) -> i32 {
        self.projection(ProjectionB(b))
    }
}
```

This will let you write the more natural code as follows instead of having to
explicitly create and refer to Keys everywhere in the code base:

```rust
use MyComputationTrait;
use SyncProjectionTrait;

async fn main() {
    let dice = Dice::builder().build();
    let root = dice.new_root().await;

    let ctx = dice.updater(root).commit();

    ctx.direct_projection("a").await;
    let opaque = ctx.opaque_indirection("b").await;

    {
        // this block can be synchronous
        opaque.sync_projection(1);
    }
}
```
