# Error Handling

Buck2 uses `buck2_error` replacing both `anyhow` and `thiserror`.

Use of `anyhow` or `thiserror` in `buck2/app` is banned except where there are pre-existing
exceptions or when extremely strongly justified.

## Result type

```rust
fn my_function() -> buck2_error::Result<String> {
    // ...
}
```

## Defining custom error types

Use `#[derive(Debug, buck2_error::Error)]`, with an API similar to `thiserror`. Every error
must carry an `ErrorTag`:

```rust
#[derive(Debug, buck2_error::Error)]
#[error("My error message: {field}")]
#[buck2(tag = Input)]
struct MyError {
    field: String,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum MyErrors {
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Missing required field: {0}")]
    MissingField(String),
}
```

## Error tags

Tags are defined in `app/buck2_data/error.proto`. Common generic tags:

- `Input` — user input errors (invalid arguments, malformed build files, ...)
- `Tier0` — critical infrastructure failures
- `Environment` — external environment issues (system config, external services,
  network/certs, filesystem)

Using just one of these three tags is fine for most errors. When adding additional tags other than
those, reuse existing tags only when appropriate.

## Ad-hoc errors

```rust
use buck2_error::buck2_error;

if some_condition {
    return Err(buck2_error!(
        buck2_error::ErrorTag::Input,
        "Invalid value: expected {}, got {}",
        expected,
        actual
    ));
}
```

## Internal errors (bugs in Buck2 itself)

When reaching a condition that represents an invariant violation and should never fire, panicking
via `.expect()`, `.unwrap()`, etc. is ok for file-local invariants. For non-file-local, prefer a
variant of internal error:

```rust
use buck2_error::internal_error;

let value = map.get(key).internal_error("Key must exist")?;

return Err(internal_error!(
    "Unexpected state: {} should not be empty",
    collection_name
));
```

For `Option`, prefer `.internal_error(...)` / `.with_internal_error(|| ...)`
over `.context(...)` or `.expect(...)`.

## Adding context

`buck2_error` supports `buck_error_context` APIs akin to anyhow's context:

```rust
use buck2_error::BuckErrorContext;

result.buck_error_context("Failed to process file")?;

result.with_buck_error_context(|| format!("Failed to process file: {}", path))?;
```

Be somewhat conservative in the use of context, more is not always better.

## Conversion

`buck2_error::Error` impls `From` for many common error types, including many from std and common
dependencies. When none exists, follow existing patterns, add one if semantically appropriate, and
otherwise use an ad-hoc conversion:

```rust
use buck2_error::conversion::from_any_with_tag;

some_result.map_err(|e| from_any_with_tag(e, ErrorTag::Tier0))?;
```

## Soft errors and deprecations

Both report through the event log to Logview without failing the command. They differ in who
the report is for, and that decides everything else, so pick by audience:

- `soft_error!("category", err)`: something went wrong but the command can go on. The buck2
  oncall watches these per category. It is not shown to the user.
- `warn_user!("category", err)`: the same, and the person running the command is told, because
  they can act on it: their checkout, their environment, their invocation. Buck2 does not print
  warnings that are really addressed to someone else, so this is the only thing it prints.
- `deprecation!("category", err, owner: "unixname", started: "YYYY-MM-DD")`: a use of something
  that is being removed. Only `owner`, the person driving the removal, cares about the count;
  nothing is shown to anyone. Once the count is zero the call site becomes a hard error.
  `started` is the day the deprecation went in: one that is six months old with nobody driving
  it gets deleted and replaced with a `FIXME`.

Under `BUCK2_HARD_ERROR=true`, which buck2's integration tests set, a report fails the command:
the macro returns the error from the enclosing function, the way `?` would, so the condition
fails the test. A call site with nowhere to return an error to uses the `_or_panic` form of the
macro (`soft_error_or_panic!` and so on), and the report panics there instead. A test that
expects the condition opts out with `allow_soft_errors=True`. Open source builds report nothing
and fail nothing.

## Worked example

```rust
fn process_artifact(&self, artifact: &Artifact) -> buck2_error::Result<()> {
    let path = artifact.path()
        .buck_error_context("Failed to get artifact path")?;

    if !path.exists() {
        return Err(buck2_error!(
            buck2_error::ErrorTag::Input,
            "Artifact does not exist: {}",
            path
        ));
    }

    Ok(())
}
```
