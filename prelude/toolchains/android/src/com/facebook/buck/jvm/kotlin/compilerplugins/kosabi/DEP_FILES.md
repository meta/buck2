# Kotlin dep files

How Buck2 knows which classpath classes a Kotlin compile actually used — and why
library compiles and source-only ABI compiles track different things.

## What a dep file is

A dep file is the action's report of which of its declared inputs it actually
used. After a `kotlincd` action runs, the compiler plugin writes one URI per
line:

```
jar:file:///path/to/dep.jar!/com/example/Foo.class
```

On the next build, Buck2 prunes the action's inputs to the previously recorded
set when computing the cache key: inputs outside the set are ignored for
invalidation, so an untouched dependency changing does not recompile you.
A stale or incomplete set causes false HITs (missed rebuilds or missed errors);
an over-broad set causes pure-waste rebuilds. Dep-file work is the trade
between the two.

### Per-jar vs per-class

The classpath inputs are jars, so jar-level tracking falls out naturally. But
if target `A` uses only `B1.class` from jar `B`, a change to `B2.class` should
not recompile `A`. Per-class tracking needs individual classes as action
inputs: the build passes jars on the classpath plus a directory representation
of each jar whose files contain the class hash rather than the class bytes
(memory-cheap). The compiler's report is then mapped onto those per-class
inputs. Per-class dep files were enabled for JavaCD/KotlinCD in D43389937
(plumbing in D43118078) at ~0.5G at-rest memory cost on `fb4a`.

Mode is controlled by buckconfig `kotlin.kotlincd_dep_files` (`per_class` /
`per_jar`); production validation in this area always uses `per_class`.

## Pipeline

```
kotlinc (+ dep-tracker / kosabi plugin) ──► kotlin-used-classes-tmp.json
                                                  │
KotlinClassUsageHelper.getClassUsageData ── merges kapt + ksp reports ──► dep-file.txt (+ jar-level coverage)
```

| Step | Code |
|---|---|
| Temp report path (`kotlin-used-classes-tmp.json`) | `CompilerOutputPaths.getKotlinTempDepFilePath` |
| URI parse + kapt/ksp merge | `KotlinClassUsageHelper` (`fbcode/buck2/prelude/toolchains/android/src/com/facebook/buck/jvm/kotlin/`) |
| URI → jar/class map | `ClassUsageURIParser` (`fbcode/buck2/prelude/toolchains/android/src/com/facebook/buck/jvm/java/`) |
| Final dep-file writer | `DefaultClassUsageFileWriter` via `ClassUsageFileWriterFactory` |
| Toolchain flags → action | `fbcode/buck2/platform/kotlin_toolchains.bzl`, `KotlincStep.java`, `kotlincd.proto` |
| Dep-tracker plugin wiring | `track_class_usage_plugin` in `kotlin_toolchains.bzl` (`fbsource//fbandroid/java/com/facebook/kotlin/compilerplugins/usedclasses:dep-tracker`) |

Entries naming the stubs jars (`stubgen_stubs.jar`, `stubs.jar`) are dropped:
generated stubs carry no dependency signal.

## The two collectors

Library compiles and source-only ABI compiles run different collectors with
deliberately different contracts.

| | Library (`dep-tracker`) | Source-only ABI (Kosabi) |
|---|---|---|
| Collector | `FirClassUsageRecorder` + `ClassUsageRecorder` (`fbandroid/java/com/facebook/kotlin/compilerplugins/usedclasses/`) | `KosabiClassUsageCollector` (`.../compilerplugins/kosabi/k2-jvm-abi-gen/.../KosabiClassUsageCollector.kt`) |
| Mechanism | FIR checkers fired during analysis: `TypeRecordChecker` on every `FirTypeRef` (bodies included), `ImportChecker` on all imports, qualifier/access-expression checkers (`DependencyTrackerCheckerExtension.kt`) | Post-analysis walk of the resolved FIR tree. Kosabi runs its own sandboxed FIR session where dep-tracker checkers are not registered and the checker phase is skipped, so the collector walks declarations directly (`ClassUsageVisitor`; bodies are not visited) |
| Call site | Checker extension registration (`DependencyTrackerFirExtensionRegistrar.kt`) | `K2JvmAbiFirAnalysisHandlerExtension` calls `collectAndDump` right after `runFrontendForKosabi`, before `collectMissingTypesFromFir` / `stripUnresolvableSupertypesFromFir` |
| Output path | `DependencyTrackerCommandLineProcessor.ARG_OUTPUT_PATH` | Same key, reached via reflection (the abi-gen module does not depend on dep-tracker) |
| Contract | Track everything compilation needs, **including unused imports** (D91700150). Removing an unused-import target → MISS → rebuild → real `unresolved reference` error | Track only what shapes ABI bytes. Removing an unused-import target → **HIT**, no rebuild — Kosabi stubs the missing type, so a rebuild would succeed with byte-identical output |

The D92146618 false positive was the *library* leg HITting when it should have
failed; that leg still MISSes and fails via its own dep file. The source-only
HIT cannot mask it: separate actions, separate dep files. Library builds also
produce jar-level coverage (`used-jars.json`); source-only ABI actions do not
emit it.

## Source-only tracking behavior

Reference for what `KosabiClassUsageCollector` records at the head of the
Kosabi dep-file accuracy stack. Rule of thumb: a type is tracked iff a change
to it can change the emitted ABI jar.

### Tracked

| Position | Mechanism |
|---|---|
| Supertypes (+ full closure by default) | `recordClassDeclaration` recurses into `superTypeRefs` (Kotlin) / `supertypes` (Java), plus annotations on each |
| Signatures: return, value-param, property types | `ClassUsageVisitor` (`visitNamedFunctionCompat`, `visitProperty`, `visitConstructor`, `visitEnumEntry`, `visitTypeAlias`) |
| Type-parameter bounds (incl. `where` clauses, which merge into `FirTypeParameter.bounds` at RAW FIR build time) + type-param annotations | `recordTypeParameters`; `FirTypeParameterRef` resolves through the symbol when not already a `FirTypeParameter` (fail-closed) |
| Extension receivers + context parameters | `recordReceiverAndContext` (receivers become JVM parameters, hence ABI) |
| Type arguments, recursively (`List<B>` records `B` even though `List` is skipped) | `recordType` walks `ConeClassLikeType.typeArguments` before the package early-return |
| Annotations and their arguments | `recordAnnotation` + `ExpressionUsageVisitor` (resolved qualifiers, qualified-access resolved + dispatch-receiver types) |
| `const val` initializers (inlined into the ABI) | `ExpressionUsageVisitor` on the initializer |
| Annotation-class constructor defaults (ABI via `AnnotationDefault`) | `visitConstructor` walks defaults only when the owner is an annotation class (`isAnnotationClassConstructor`); all other defaults are regenerated during stripping and stay untracked |
| Missing (unresolvable) types in ABI positions | Missing-type guards — see below |

### Skipped

| Position | Rationale |
|---|---|
| Imports | Kosabi stubs missing types by design, so import resolution never affects ABI bytes |
| `kotlin` / `kotlin.*` and `java` / `java.*` packages | `isStdlibOrJdkPackage` (`common/Types.kt`); the trailing dots are load-bearing — `javax.*` / `kotlinx.*` are dependency namespaces and stay tracked. True stdlib Kotlin classes are also skipped by the `BuiltIns`-origin check; the package check is load-bearing for the JDK leg (`FirJavaClass` with `Java.Library` origin is indistinguishable from dep Java) |
| Local classes, `BuiltIns` origin | Never classpath ABI signal |
| Private-member return/receiver types (`Private` / `PrivateToThis` / `Local`, mirroring `IrNonAbiStripper.removeNonPublicApi`) | Stripped from the ABI (`isStrippedFromAbi`). Value-param types stay recorded: a defaulted private fun's synthetic `$default` stub is kept with its real descriptor |
| Stubs jars | `isStubJarPath` (`stubgen_stubs.jar`, `stubs.jar`) |
| Error-sentinel ids (`<error>`) | Name no class file |

### Missing-type guards

When a type is absent from the source-only classpath, the collector would
record nothing — yet the missing-ness shapes ABI bytes (the unresolvable
supertype is stripped). A later change that makes the type resolvable (a dep
added to `source_only_abi_deps`, or a class moving from an off-classpath jar
to an on-classpath jar) would then wrongly HIT while every tracked entry stays
byte-identical.

The fix collects missing class ids (`missingClassIds`, from `ConeErrorType`
via `recordMissingClassIdFromError` and from unresolvable symbols in
`recordType`) and records a guard entry for each under every jar on the
classpath (`addMissingTypeGuards`, from `configuration.jvmClasspathRoots`
plus any touched jar). A guard trips a MISS exactly when the missing class
appears, changes, or disappears in that jar; unrelated changes still HIT.
Guard construction never fails the build. A jar prefix cannot name a missing
entry inside the zip, so guards watch the whole jar.

### Shallow mode (`kotlin.kosabi_shallow_class_usage`)

Default-off plugin option `shallowClassUsage`: the target's own supertype
chain is recorded with supertype recursion (HIERARCHY, no annotations) while
everything else is recorded without recursion (SHALLOW). Two visited sets with
hierarchy-upgrade on re-visit keep the result independent of visit order.
Flag-off output is byte-identical to before; flag-on strictly shrinks tracked
sets (it also nets most guard lines, since guards hang off
transitive-only refs).

## Flags and plugin options

| Buckconfig | Proto (`kotlincd.proto`) | `kotlinc -P` (source-only only, via `KotlincStep.configureSourceOnlyOptions`) | Config key |
|---|---|---|---|
| `kotlin.kosabi_shallow_class_usage` (bool, default off) | `shouldKosabiUseShallowClassUsage` (32) | `plugin:com.facebook.k2.jvm.abi.gen:shallowClassUsage=true` | `K2JvmAbiConfigurationKeys.SHALLOW_CLASS_USAGE` |
| `kotlin.kosabi_abi_validation_mode` (`off`/`warn`/`error`, default `off`) | `kosabiAbiValidationMode` (33) | `plugin:com.facebook.k2.jvm.abi.gen:abiValidationMode=<mode>` | `K2JvmAbiConfigurationKeys.ABI_VALIDATION_MODE` |
| `kotlin.kosabi_kept_body_stub_check` (bool, default on) | `disableKosabiKeptBodyStubCheck` (36) | `plugin:com.facebook.k2.jvm.abi.gen:keptBodyStubCheck=false` | `K2JvmAbiConfigurationKeys.KEPT_BODY_STUB_CHECK` |

`abiValidationMode` reports ABI content repaired because a dep was missing
from the source-only classpath: `warn` reports each repair, `error` fails the
compile instead of emitting a knowingly-wrong ABI. Parsed in
`K2JvmAbiCommandLineProcessor.kt` alongside `outputDir`, `earlyTermination`,
`enableMixedCompilation`, `debugOutput`, `enableComposeAbiEmulation`.

## Tests

- Behavior tests: `test_dynamic_source_only_kotlincd_*_dep_files` in
  `fbcode/buck2/tests/meta_only/e2e/rules/jvm/test_kosabi_dep_files.py`. Each
  mutates a dep (or adds a class) and asserts HIT vs MISS plus ABI-jar
  byte-equality via `get_source_only_abi_jar_bytes`. Convention is
  assert-buggy test-first: the test lands asserting the buggy behavior with a
  `# BUG` marker, the fix flips the assertion.
- Fixtures: `fbandroid/buck2/tests/good/kotlincd_dep_files/` (`BUCK` +
  `SourceOnly*.kt`).
- Run: `buck2 test fbcode//buck2/tests/meta_only/e2e/rules/jvm:test_kosabi_dep_files --
  <name filter>`; the `source_only` / `unused_import` filters run the family.

### Production validation recipe

Per-fix evidence in this area uses production `[source-only-abi]` targets,
not just fixtures:

1. Build with `-c kotlin.kotlincd_dep_files=per_class --prefer-local
   --no-remote-cache -c build.use_persistent_workers=true -c
   build.remote_dep_file_cache_enabled=false`, per-mode `--isolation-dir`,
   snapshot `output_artifacts/source_only_abi_jar/`.
2. Set-diff dep files after normalizing `buck-out/<iso>/`, `art/<cell>/<hash>/`,
   and config-hash dirs; compare ABI jars by sha256.
3. For removals, prove the skipped rebuild is safe: mutate the dep so its jar
   changes, rebuild the consumer in place (expect dep-file HIT, kind=7), then
   force a rebuild in a fresh isolation dir and confirm byte-identical ABI.
   Additive changes need no ABI column — identical output is trivial.
4. For fixes, also run the flip: the same mutation at the parent commit must
   MISS (kind=8) with ABI-identical output (the waste) or stale-HIT (the bug),
   depending on direction.

## Further reading

- Kosabi K2 technical design doc (plugin core, `KosabiClassUsageCollector`
  section, key code pointers)
- D91700150 (track all imports in library dep files) / D92146618 (unused-import
  false positive) and the originating R-class Workplace thread
- Per-class dep files for JavaCD/KotlinCD (Workplace post, Feb 2026)
- Buck2 `dep_files` rule-author docs
