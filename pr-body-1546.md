# Fix sorted() to handle incomparable items gracefully

## Summary

This PR fixes issue #1546 where `sorted()` would crash the process when attempting to sort lists containing incomparable items (e.g., mixing integers and functions). The crash occurred because the comparison function violated the total order contract, causing Rust's sort algorithm to panic with "user-provided comparison function does not correctly implement a total order".

## Changes

Modified `starlark-rust/starlark/src/stdlib/funcs/other.rs`:

1. **Added validation pass**: Before sorting, the function now compares the first element with all other elements to detect incomparability early
2. **Clean error handling**: When incomparable items are detected, the function returns a proper Starlark error instead of panicking
3. **Added test**: New test `test_sorted_incomparable_items()` verifies that incomparable items produce an error rather than crashing

## Technical Details

The original implementation returned `Ordering::Equal` for incomparable pairs, intending to surface the comparison error after the sort completed. However, this violates the total order contract (e.g., if `len` compares Equal to both `0` and `9`, but `0 < 9`, transitivity is broken). Since Rust 1.81, the standard library's stable sort detects this inconsistency and panics before the deferred error check is reached.

The fix adds an O(n) validation pass that attempts to compare the first element with all others before sorting. This catches type incompatibility issues early and returns the comparison error as a clean Starlark error message:

```
Operation `compare` not supported for types `int` and `function`
```

## Testing

- Added new test case with the reproducer from the issue
- All existing tests in the starlark crate pass (`cargo test` in `starlark-rust/starlark`)
- Verified the test fails with a panic on the original code
- Verified the test passes with proper error handling on the fixed code

### Reproducer (from issue)

```python
sorted([5, 4, 3, 2, 1, 0, len, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, len, 9, 8, 7, 6])
```

Before: Process panics/aborts
After: Returns clean error message

Fixes #1546
