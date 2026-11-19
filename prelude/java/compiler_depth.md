# Android compiler depth (prototype)

`android_binary` and `android_bundle` accept an optional, inclusive
`max_jvm_compiler_depth` integer. Analysis fails if the depth exceeds the limit and
prints one longest compiler chain, dependencies first. `None` (the default)
disables the assertion; zero is valid, negative limits are rejected.

`android_binary_for_automation` accepts
`max_jvm_compiler_depth_by_package_type`, a dictionary that overrides
`max_jvm_compiler_depth` for the listed package types, such as `debug`, `exo`,
and `exo-native`. Unlisted package types retain the default limit. Under
`ovr_config//build_mode:optimization[opt]` the override is `None`, because opt
builds use class ABIs and their depth is not comparable to dev-mode limits.

```python
android_binary(
    name = "app",
    max_jvm_compiler_depth = 48,
    # ...
)
```

The `[jvm_compiler_depth]` subtarget contains JSON with `depth`, `chain`, and the
configured limit. To inspect it without running the app's existing validations:

```sh
buck2 aquery 'all_outputs("fb4a[jvm_compiler_depth]")' --output-attribute contents --json
```

Each chain entry identifies the configured target, compiler action category,
and action identifier. Ties retain one representative chain.

## Reducing compiler depth

Use the reported chain to identify dependency paths to shorten:

* Cut unnecessary dependencies between targets in the chain, or split a target
  so consumers can depend on a smaller API.
* Set `abi_generation_mode = "source_only"` on a library in the chain so its
  consumers need not wait for its full library compilation.
* For a source-only ABI step, refactor code to remove unnecessary
  `source_only_abi_deps` entries. If a dependency is no longer needed for ABI
  generation, set its `required_for_source_only_abi = False`.

Rebuild affected targets to validate ABI changes, then rerun analysis. Another
chain may become the longest after a change.

## Definition

The metric counts `javacd_jar`, `kotlincd_jar`, `source_abi_javacd_jar`,
`source_only_abi_javacd_jar`, and `source_only_abi_kotlincd_jar` actions.
Class ABI extraction, KSP, and other compiler backends add zero.

`JavaClasspathEntry` carries separate depths for the full library and the
selected ABI. A counted compiler action adds one to the maximum depth of its
compiling classpath inputs. Ordinary source ABI compilation adds one using the
full compiling classpath. Source-only ABI compilation uses only the entries
selected by `required_for_source_only_abi` and `source_only_abi_deps`. Kotlin
friend ABIs are also inputs: full compilation includes all friends, while
source-only ABI compilation includes only the friends retained by that same
filter. Class ABIs inherit the full library's depth.

`JavaPackagingInfo.packaging_deps` carries each library's full-JAR depth through
`JavaPackagingDep`. Android takes the maximum over its primary platform's
packaging closure, including runtime dependencies and depth metadata on entries
without a JAR. Provided dependencies affect compilation but are not independently
packaged. Exported classpaths retain their depths through targets without sources.
For an arm64 app, the primary
platform's dependencies are arm64; the report includes configurations so this
can be verified.

Transitive-set reductions compute classpath maxima without flattening the graph
at every target. Witnesses share predecessor records, and only the final report
materializes a full chain.

## Scope of the prototype

This is a classpath-based analysis metric, not a traversal of every action input.
Dependencies hidden in generated sources, compiler tools/plugins, raw prebuilt
artifacts, jar postprocessors, or other unmodelled artifact inputs are not
recovered. Such paths can make the full action-graph depth larger. Prebuilt jars
have zero intrinsic compiler depth. The limit covers the primary Java packaging
configuration, not all native architecture variants or execution platforms.
