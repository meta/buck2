# Basic API

Everything starts at the base struct `Dice`.

From it, you can get the `TransactionUpdater` where you can report the changes
to your computation graph you expect to see this time. You can dirty nodes to be
recomputed via `TransactionUpdater::changed`, or report new
[Injected Values](writing_computations.md#injected-keys) via
`TransactionUpdater::changed_to`. These changes recorded will not be seen by
anyone until you commit them via `TransactionUpdater::commit`, which returns you
an instance of `DiceTransaction` containing all changes recorded up until this
moment (but not any future changes).

`DiceTransaction` instances that have the same set of reported changes are
considered to be of the same state. Computations can be requested from them
concurrently, and work will be shared. However, concurrent requests to
`DiceTransactions` with different states are NOT supported. DICE currently does
NOT enforce this rule. Please be aware.

The function `DiceComputations::compute(Key)` is used to compute a specific Key,
returning a future to the result. `DiceTransaction` derefs into a
`DiceComputations` such computations can be called directly on the transaction.

To make code look natural, we often employ the pattern of writing computation
traits as follows:

```rust
#[async_trait]
trait MyComputationTrait {
    async fn some_compute(&self, a: i32) -> i32;

    async fn more_compute(&self, b: &str) -> MyOutputType;
}

#[async_trait]
impl MyComputationTrait for DiceComputations {
    async fn more_compute(&self, b: &str) -> MyOutputType {
        self.compute(MyKey(b.to_owned()))
    }

    async fn some_compute(&self, a: i32) -> i32 {
        self.compute(MyOtherKey(a))
    }
}
```

This will let you write the more natural code as follows instead of having to
explicitly create and refer to Keys everywhere in the code base:

```rust
use MyComputationTrait;

async fn main() {
    let dice = Dice::builder().build();
    let root = dice.new_root().await;

    let ctx = dice.updater(root).commit();

    ctx.more_compute("a").await;
    ctx.some_compute(1).await;
}
```

You can group computations in traits however you wish, with whatever Key types
you desire. In buck2, we tend to group related computations together (i.e all
parsing computations in one trait, or all action computations in one trait) to
better organize our modules and limit the number of dependencies pulled in.
