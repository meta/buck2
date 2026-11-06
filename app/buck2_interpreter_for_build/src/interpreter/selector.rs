/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Display;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::types::select_fail::StarlarkSelectFail;
use buck2_interpreter::types::select_incompatible::StarlarkSelectIncompatible;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

/// Representation of `select()` in Starlark.
#[derive(Debug, ProvidesStaticType, Trace, Freeze, Allocative, StarlarkPagable)]
#[repr(C)]
pub enum StarlarkSelector<'v> {
    /// Simplest form, backed by dictionary representation
    /// wrapped into `select` function call.
    Primary(ValueOf<'v, DictType<StringValue<'v>, Value<'v>>>),
    Sum(Value<'v>, Value<'v>),
}

impl<'v> Display for StarlarkSelector<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StarlarkSelector::Primary(v) => {
                f.write_str("select(")?;
                Display::fmt(v, f)?;
                f.write_str(")")
            }
            StarlarkSelector::Sum(l, r) => {
                Display::fmt(l, f)?;
                f.write_str(" + ")?;
                Display::fmt(r, f)
            }
        }
    }
}

impl<'v> Serialize for StarlarkSelector<'v> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            StarlarkSelector::Primary(dict_value) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("__type", "selector")?;
                map.serialize_entry("entries", &dict_value.to_value())?;
                map.end()
            }
            StarlarkSelector::Sum(left, right) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("__type", "concat")?;
                map.serialize_entry("items", &[left.to_value(), right.to_value()])?;
                map.end()
            }
        }
    }
}

starlark_complex_value!(pub StarlarkSelector);

impl<'v> StarlarkSelector<'v> {
    pub fn new(d: ValueOf<'v, DictType<StringValue<'v>, Value<'v>>>) -> Self {
        StarlarkSelector::Primary(d)
    }

    fn sum(left: Value<'v>, right: Value<'v>, heap: Heap<'v>) -> Value<'v> {
        heap.alloc(StarlarkSelector::Sum(left, right))
    }

    pub fn from_concat<I>(iter: I, heap: Heap<'v>) -> buck2_error::Result<Value<'v>>
    where
        I: IntoIterator<Item = Value<'v>>,
    {
        fn values_to_selector<'v, I>(
            selector: Option<StarlarkSelector<'v>>,
            values: &mut I,
            heap: Heap<'v>,
        ) -> buck2_error::Result<NoneOr<StarlarkSelector<'v>>>
        where
            I: Iterator<Item = Value<'v>>,
        {
            match (selector, values.next()) {
                (None, None) => Ok(NoneOr::None),
                (None, Some(v)) => {
                    if let Some(next_v) = values.next() {
                        let head = StarlarkSelector::Sum(v, next_v);
                        values_to_selector(Some(head), values, heap)
                    } else {
                        let v = ValueOf::unpack_value_err(v.to_value())
                            .internal_error("validated at construction")?;
                        Ok(NoneOr::Other(StarlarkSelector::new(v)))
                    }
                }
                (Some(s), None) => Ok(NoneOr::Other(s)),
                (Some(s), Some(v)) => {
                    let head = Some(StarlarkSelector::Sum(heap.alloc(s), v));
                    values_to_selector(head, values, heap)
                }
            }
        }
        let selector = values_to_selector(None, &mut iter.into_iter(), heap)?;
        Ok(heap.alloc(selector))
    }

    fn select_map<'a, const RECURSE: bool>(
        eval: &mut Evaluator<'a, '_, '_>,
        val: Value<'a>,
        func: Value<'a>,
    ) -> starlark::Result<Value<'a>> {
        // The recursion depth of this function is determined by the depth of
        // the input `select()` objects, which could be unbounded. Detect such
        // cases and return an error nicely rather than panic'ing.
        buck2_util::threads::check_stack_overflow()?;

        fn invoke<'v>(
            eval: &mut Evaluator<'v, '_, '_>,
            val: Value<'v>,
            func: Value<'v>,
        ) -> starlark::Result<Value<'v>> {
            eval.eval_function(func, &[val], &[])
        }

        if let Some(selector) = StarlarkSelector::from_value(val) {
            match *selector {
                StarlarkSelector::Primary(selector) => {
                    let selector = selector.dict();
                    let mut mapped = BuckMutInsertionMap::with_capacity(selector.len());
                    for (k, v) in selector.iter_hashed() {
                        if StarlarkSelectFail::from_value(v).is_some()
                            || StarlarkSelectIncompatible::from_value(v).is_some()
                        {
                            // mappings don't get applied to SelectFail/SelectIncompatible values.
                            mapped.insert_hashed(k, v);
                        } else {
                            mapped.insert_hashed(
                                k,
                                if RECURSE {
                                    Self::select_map::<RECURSE>(eval, v, func)
                                } else {
                                    invoke(eval, v, func)
                                }?,
                            );
                        }
                    }
                    Ok(eval.heap().alloc(StarlarkSelector::new(
                        ValueOf::unpack_value_err(eval.heap().alloc(mapped))
                            .internal_error("validated at construction")?,
                    )))
                }
                StarlarkSelector::Sum(left, right) => Ok(eval.heap().alloc(StarlarkSelector::Sum(
                    Self::select_map::<RECURSE>(eval, left, func)?,
                    Self::select_map::<RECURSE>(eval, right, func)?,
                ))),
            }
        } else if StarlarkSelectFail::from_value(val).is_some()
            || StarlarkSelectIncompatible::from_value(val).is_some()
        {
            // mappings don't get applied to SelectFail/SelectIncompatible values.
            Ok(val)
        } else {
            invoke(eval, val, func)
        }
    }

    fn select_test<'a>(
        val: Value<'a>,
        eval: &mut Evaluator<'a, '_, '_>,
        func: Value<'a>,
    ) -> starlark::Result<bool> {
        fn invoke<'v>(
            eval: &mut Evaluator<'v, '_, '_>,
            func: Value<'v>,
            val: Value<'v>,
        ) -> starlark::Result<bool> {
            eval.eval_function(func, &[val], &[])?
                .unpack_bool()
                .ok_or_else(|| {
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Expected testing function to have a boolean return type"
                    )
                    .into()
                })
        }

        if let Some(selector) = StarlarkSelector::from_value(val) {
            match *selector {
                StarlarkSelector::Primary(selector) => {
                    let selector = selector.dict();
                    for v in selector.values() {
                        // select_test only applies the test to non-SelectFail/SelectIncompatible values.
                        if StarlarkSelectFail::from_value(v).is_none()
                            && StarlarkSelectIncompatible::from_value(v).is_none()
                        {
                            let result = invoke(eval, func, v)?;
                            if result {
                                return Ok(true);
                            }
                        }
                    }
                    Ok(false)
                }
                StarlarkSelector::Sum(left, right) => {
                    Ok(Self::select_test(left, eval, func)?
                        || Self::select_test(right, eval, func)?)
                }
            }
        } else if StarlarkSelectFail::from_value(val).is_some()
            || StarlarkSelectIncompatible::from_value(val).is_some()
        {
            // select_test only applies the test to non-SelectFail/SelectIncompatible values.
            Ok(false)
        } else {
            invoke(eval, func, val)
        }
    }
}

#[starlark_value(type = "Select")]
impl<'v> StarlarkValue<'v> for StarlarkSelector<'v> {
    fn to_bool(&self) -> bool {
        true
    }

    fn radd(&self, left: Value<'v>, heap: Heap<'v>) -> Option<starlark::Result<Value<'v>>> {
        let right = heap.alloc(match self {
            StarlarkSelector::Primary(x) => StarlarkSelector::Primary(*x),
            StarlarkSelector::Sum(x, y) => StarlarkSelector::Sum(x.to_value(), y.to_value()),
        });
        Some(Ok(StarlarkSelector::sum(left, right, heap)))
    }

    fn add(&self, other: Value<'v>, heap: Heap<'v>) -> Option<starlark::Result<Value<'v>>> {
        match self {
            Self::Primary(v) => {
                let this = heap.alloc(StarlarkSelector::new(*v));
                Some(Ok(StarlarkSelector::sum(this, other, heap)))
            }
            Self::Sum(l, r) => {
                let this = StarlarkSelector::sum(l.to_value(), r.to_value(), heap);
                Some(Ok(StarlarkSelector::sum(this, other, heap)))
            }
        }
    }

    // used to provide the type documentation here
    fn get_methods() -> Option<&'static Methods> {
        Some(SELECTOR_METHODS.methods())
    }
}

starlark::methods_static!(SELECTOR_METHODS = selector_methods);

#[starlark_module]
#[starlark_types(StarlarkSelector<'_> as Select)]
pub fn register_select(globals: &mut GlobalsBuilder) {
    fn select<'v>(
        #[starlark(require = pos)] d: ValueOf<'v, DictType<StringValue<'v>, Value<'v>>>,
    ) -> starlark::Result<StarlarkSelector<'v>> {
        Ok(StarlarkSelector::new(d))
    }

    /// Create a value to be used in select() statements to indicate a failure case.
    ///
    /// This function is used within a `select()` statement to provide a clear error message
    /// when a particular configuration condition is selected but should not be valid.
    ///
    /// # Example
    /// ```python
    /// # Fail with a custom message if the build is configured for iOS but the feature is not supported
    /// some_attr = select({
    ///     "//conditions:is_android": "android_value",
    ///     "//conditions:is_ios": select_fail("This feature is not supported on iOS"),
    ///     "DEFAULT": "default_value",
    /// })
    /// ```
    ///
    /// During configuration, if the iOS case is selected, the build will fail with a message that includes
    /// "This feature is not supported on iOS". If the target is loaded but never configured, it's not an error
    /// and if the target is configured such that the iOS case is not selected, the build will not fail.
    fn select_fail<'v>(
        #[starlark(require = pos)] msg: StringValue<'v>,
    ) -> starlark::Result<StarlarkSelectFail<'v>> {
        Ok(StarlarkSelectFail::new(msg))
    }

    /// Create a value to be used in select() statements to mark a target as incompatible.
    ///
    /// This function is used within a `select()` statement to indicate that the target is
    /// incompatible with the platform when a particular configuration condition is selected.
    /// The target will be skipped gracefully, just like targets with unsatisfied
    /// `target_compatible_with` constraints.
    ///
    /// # Example
    /// ```python
    /// # Mark the target as incompatible when building for iOS
    /// some_attr = select({
    ///     "//conditions:is_android": "android_value",
    ///     "//conditions:is_ios": select_incompatible("This target is not supported on iOS"),
    ///     "DEFAULT": "default_value",
    /// })
    /// ```
    ///
    /// During configuration, if the iOS case is selected, the target will be skipped as
    /// incompatible rather than causing a build error. If the target is loaded but never
    /// configured, it's not an error, and if the target is configured such that the iOS case
    /// is not selected, the target will build normally.
    fn select_incompatible<'v>(
        #[starlark(require = pos)] msg: StringValue<'v>,
    ) -> starlark::Result<StarlarkSelectIncompatible<'v>> {
        Ok(StarlarkSelectIncompatible::new(msg))
    }

    /// Maps a selector.
    ///
    /// For selector additions values:
    ///
    ///   Each value on each side of an addition is always passed to select_map
    ///   recursively.
    ///
    /// For selector maps values, for each value within a selector:
    ///
    ///   If recurse is False, the value will be passed to the mapping function.
    ///   This may be a selector, or not.
    ///
    ///   If recurse is True, the value will be passed to select_map()
    ///   recursively
    ///
    /// For non-selector value, the value will be passed to the mapping
    /// function.
    ///
    /// Generally the returned selector will have the same key/depth structure
    /// as the input selector. In the recurse=False case a mapping function that
    /// turns a selector into a non-selector may prune the structure. In the
    /// recurse=True case, the mapping functino cannot change the existing
    /// structure. In either case the mapping function can return a selector for
    /// a non-selector input, causing the selector structure to get deeper.
    ///
    /// The mapping is not applied to `select_fail()` or `select_incompatible()` values.
    ///
    /// Ex:
    /// ```python
    /// def increment_items(a):
    ///     return [v + 1 for v in a]
    ///
    /// select_map([1, 2] + select({"c": [2]}), increment_items, False) == [2, 3] + select({"c": [3]})
    ///
    /// select_apply([1, 2] + select({"c": select({"d": [2]}}), increment_items, True) == [2, 3] + select({"c": select("d": [3]})})
    /// ```
    fn select_map<'v>(
        #[starlark(require = pos)] d: Value<'v>,
        #[starlark(require = pos)] func: Value<'v>,
        // TODO(T245559941): change the default to recurse=true
        #[starlark(require = named, default = false)] recurse: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        if recurse {
            StarlarkSelector::select_map::<true>(eval, d, func)
        } else {
            StarlarkSelector::select_map::<false>(eval, d, func)
        }
    }

    /// Test values in the select expression using the given function.
    ///
    /// Returns True, if any value in the select passes, else False. The function is not tested against select_fail or select_incompatible values.
    ///
    /// Ex:
    /// ```python
    /// select_test([1] + select({"c": [1]}), lambda a: len(a) > 1) == False
    /// select_test([1, 2] + select({"c": [1]}), lambda a: len(a) > 1) == True
    /// select_test([1] + select({"c": [1, 2]}), lambda a: len(a) > 1) == True
    /// select_test(select({"c": select_fail("")}), lambda a: True) == False
    /// ```
    fn select_test<'v>(
        #[starlark(require = pos)] d: Value<'v>,
        #[starlark(require = pos)] func: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<bool> {
        StarlarkSelector::select_test(d, eval, func)
    }
}
#[starlark_module]
pub fn register_select_internal(globals: &mut GlobalsBuilder) {
    /// Tests that two selects are equal to each other. For testing use only.
    /// We simply compare their string representations.
    fn select_equal<'v>(
        #[starlark(require = pos)] left: Value<'v>,
        #[starlark(require = pos)] right: Value<'v>,
    ) -> starlark::Result<bool> {
        Ok(left.to_repr() == right.to_repr())
    }
}

/// The `Select` type represents a conditional attribute value in Buck2.
///
/// `Select` objects are created using the `select()` function and enable build rules to have
/// different attribute values based on the target's configuration.
/// This is essential for cross-platform builds and conditional compilation.
///
/// # Resolution Timing
///
/// `Select` objects are resolved during Buck2's **configuration phase**, which happens after
/// BUCK file evaluation but before rule implementation. This means:
/// - Starlark code in BUCK files and macro rules cannot see resolved values
/// - Use `select_map()` or `select_test()` for macro-level operations
///
/// # Operations
///
/// `Select` supports the addition operator (`+`) for combining conditional values:
///
/// ```python
/// # Combine multiple selects
/// deps = select({
///     "//config:linux": ["//lib:linux"],
///     "DEFAULT": [],
/// }) + select({
///     "//config:debug": ["//debug:tools"],
///     "DEFAULT": [],
/// })
///
/// # Add to regular values
/// flags = ["-Wall"] + select({
///     "//config:optimize": ["-O3"],
///     "DEFAULT": ["-O0"],
/// })
/// ```
///
/// # Resolution Algorithm
///
/// When Buck2 resolves a `Select`:
///
/// 1. Evaluates each key of select options
/// 2. Collects all keys that match the current configuration
/// 3. Applies **refinement** to choose the most specific match:
///    - If zero matches: Use DEFAULT or error if no DEFAULT
///    - If one match: Use that value
///    - If multiple matches: Select the one that is most specific
/// 4. If ties exist (equally specific):
///    - Same values: OK, use that value
///    - Different values: Error
///
/// # Select Refinement Example
///
/// ```python
/// config_setting(
///     name = "linux",
///     constraint_values = ["//constraints:linux"],  # 1 constraint
/// )
///
/// config_setting(
///     name = "linux-arm64",
///     constraint_values = [
///         "//constraints:linux",
///         "//constraints:arm64",  # 2 constraints - more specific
///     ],
/// )
///
/// my_rule(
///     srcs = select({
///         ":linux": ["generic_linux.cpp"],
///         ":linux-arm64": ["optimized_arm64.cpp"],  # This wins when both match
///     }),
/// )
/// ```
///
/// **Key point**: Order in the dictionary doesn't matter; specificity always wins.
///
/// # Configuration System
///
/// ## Constraints
///
/// - **constraint_setting**: Defines a configuration dimension
///   ```python
///   constraint_setting(name = "os")
///   constraint_setting(name = "cpu")
///   ```
///
/// - **constraint_value**: Specific value for a dimension
///   ```python
///   constraint_value(name = "linux", constraint_setting = ":os")
///   constraint_value(name = "arm64", constraint_setting = ":cpu")
///   ```
///
/// - **platform**: Collection of constraint values
///   ```python
///   platform(
///       name = "linux-arm64",
///       constraint_values = [":linux", ":arm64"],
///   )
///   ```
///
/// ## config_setting
///
/// Matches specific configuration conditions:
///
/// ```python
/// # Match constraints
/// config_setting(
///     name = "linux-arm",
///     constraint_values = [
///         "//constraints:linux",
///         "//constraints:arm",
///     ],
/// )
///
/// # Match buckconfig values
/// config_setting(
///     name = "fastmode",
///     values = {
///         "build.fastmode": "true",
///     },
/// )
///
/// # Combine both
/// config_setting(
///     name = "linux-fastmode",
///     constraint_values = ["//constraints:linux"],
///     values = {"build.fastmode": "true"},
/// )
/// ```
///
/// # Platform Selection
///
/// Buck2 determines the target platform through:
///
/// 1. `--target-platforms` command-line flag (highest priority)
/// 2. `default_target_platform` attribute on the target
/// 3. Cell's default platform from buckconfig
///
/// Example:
/// ```bash
/// buck2 build //app:main --target-platforms //platforms:linux-x86_64
/// ```
///
/// # Target Compatibility
///
/// Buck2 provides two attributes for platform compatibility:
///
/// ## target_compatible_with
///
/// **ALL semantics**: Target is compatible only if **all** listed constraints match:
///
/// ```python
/// cxx_library(
///     name = "windows_dev_only",
///     target_compatible_with = [
///         "//constraints:windows",  # Must be Windows AND
///         "//constraints:dev",      # Must be dev mode
///     ],
/// )
/// ```
///
/// ## compatible_with
///
/// **ANY semantics**: Target is compatible if **any** listed constraint matches:
///
/// ```python
/// cxx_library(
///     name = "unix_compatible",
///     compatible_with = [
///         "//constraints:linux",    # Linux OR
///         "//constraints:macos",    # macOS (either works)
///     ],
/// )
/// ```
///
/// # Common Patterns
///
/// ## Platform-Specific Dependencies
///
/// ```python
/// cxx_library(
///     name = "mylib",
///     srcs = ["common.cpp"],
///     deps = [
///         "//common:base",
///     ] + select({
///         "//config:linux": ["//platform:linux_support"],
///         "//config:macos": ["//platform:macos_support"],
///         "//config:windows": ["//platform:windows_support"],
///     }),
/// )
/// ```
///
/// ## Build Mode Flags
///
/// ```python
/// cxx_binary(
///     name = "app",
///     compiler_flags = select({
///         "//config:debug": ["-g", "-O0"],
///         "//config:release": ["-O3", "-DNDEBUG"],
///         "DEFAULT": ["-O2"],
///     }),
/// )
/// ```
///
/// ## Architecture-Specific Optimizations
///
/// ```python
/// cxx_library(
///     name = "simd",
///     compiler_flags = select({
///         "//config:x86_64": ["-msse4.2"],
///         "//config:arm64": ["-march=armv8-a"],
///         "DEFAULT": [],
///     }),
/// )
/// ```
///
/// # Working with Selects in Macros
///
/// Since `select()` values aren't resolved during BUCK evaluation, use these functions:
///
/// - **select_map(value, func)**: Transform all possible values
///   ```python
///   # Add suffix to all possible values
///   new_deps = select_map(
///       deps_select,
///       lambda items: [item + "_wrapper" for item in items]
///   )
///   ```
///
/// - **select_test(value, func)**: Test if any value matches predicate
///   ```python
///   # Check if any branch has more than 2 items
///   has_many = select_test(deps_select, lambda items: len(items) > 2)
///   ```
///
/// # Error Messages
///
/// Common errors:
///
/// - **No match without DEFAULT**: "None of N conditions matched configuration ... and no default was set"
/// - **Ambiguous match**: "Both select keys `X` and `Y` match the configuration, but neither is more specific and they have different values"
/// - **Invalid usage**: Cannot use `select()` inside config_setting or other configuration rules
///
/// # See Also
///
/// - `select()` - Creates a Select object
/// - `select_map()` - Applies a function to all possible values
/// - `select_test()` - Tests if any value passes a predicate
/// - `target_compatible_with` - Platform compatibility filtering
/// - Buck2 docs: "Configurations By Example"
#[starlark_module]
fn selector_methods(builder: &mut MethodsBuilder) {}
