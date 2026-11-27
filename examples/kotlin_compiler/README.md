# Build the Kotlin compiler with Buck2

This example builds the Kotlin compiler from the sources of
[JetBrains/kotlin](https://github.com/JetBrains/kotlin) at tag `v2.4.21`: the
`:compiler:cli-jvm` module and the 101 modules it depends on, 5253 Kotlin and
815 Java files. `gen_buck.py` writes the Buck2 project: a `BUCK` file next to
each module's `build.gradle.kts`, and `.buckconfig`, `BUCK`, `third-party/BUCK`
and `toolchains/BUCK` in the project root.

The project uses the prelude bundled with Buck2 and its demo toolchains.
`[kotlin] kotlin_version = 2.4.21` selects the Kotlin compiler that builds the
modules. Two parts are specific to this project:

- `third-party/BUCK` downloads the compiler's dependencies: jars from Maven
  Central, the IntelliJ platform jars that the compiler is built against, and
  two JetBrains jars that only JetBrains' repositories carry (`asm-all`,
  `protobuf-lite`).
- `third-party/BUCK` also downloads a JDK 8 (Amazon Corretto) for its
  `lib/tools.jar`. The modules that wrap `javac` compile against JDK 8's `javac`
  internals, as JetBrains' build does.

`model.json` holds the modules, their dependencies and their compiler options.
`pins.json` holds the hashes of the jars.

## Requirements

- JDK 21 or later, with `java` and `javac` on `PATH`
- Python 3.9 or later
- git

## Build

Install Buck2:

```sh
curl -fsSL https://raw.githubusercontent.com/facebook/buck2/main/install.sh | sh
```

Get this example, put the Kotlin sources into an empty directory, and generate
the project there:

```sh
git clone --depth 1 https://github.com/facebook/buck2.git
mkdir kotlin-compiler && cd kotlin-compiler
git clone --depth 1 --branch v2.4.21 https://github.com/JetBrains/kotlin.git
python3 ../buck2/examples/kotlin_compiler/gen_buck.py
```

Build the compiler, a single runnable jar of all 102 modules and their
dependencies:

```sh
buck2 build //:kotlinc --show-output
```

`buck2 build //kotlin/...` builds the modules alone.

## Run the compiler

```sh
cat > hello.kt <<'EOF'
fun main() = println("Hello, world!")
EOF
KOTLINC=$(buck2 build //:kotlinc --show-full-output | cut -d' ' -f2)
STDLIB=$(buck2 build //third-party:kotlin-stdlib --show-full-output | cut -d' ' -f2)
java -jar "$KOTLINC" -version -no-stdlib -no-reflect
java -jar "$KOTLINC" hello.kt -d out -no-stdlib -no-reflect -cp "$STDLIB"
java -cp "out:$STDLIB" HelloKt
```

## Source-only ABIs

`python3 ../buck2/examples/kotlin_compiler/gen_buck.py --abi kosabi` gives 79 of the modules source-only ABIs:
Buck2 builds their ABI jars from the sources with the Kosabi compiler plugins,
so the modules that depend on them can compile before they finish. The classes
are identical to those of the default class ABIs. A clean build takes about as
long: on a 174-core Linux machine it takes 146 seconds with class ABIs and 145
with source-only ABIs (medians of three). About half of that builds the
prelude's own Kotlin tools, and six modules on the longest dependency chain keep
class ABIs.

## Known gaps

- Compared with JetBrains' Gradle build of the same modules, 8 of 16282 class
  files differ: `CoreProgressManager`, `FirObjCInteropKt`,
  `Fir2IrDeclarationStorage`, `CallAndReferenceGenerator`, and four classes of
  `:compiler:fir:resolve` named `FirInvokeResolveTowerExtension$…$$inlined$…`,
  whose inlined suspend lambdas have one more `nop` because the module compiles
  against the `jvm-abi-gen` class ABIs of its dependencies.
- `asm-all` and `protobuf-lite` come from JetBrains' repositories (through
  `cache-redirector.jetbrains.com`), not from Maven Central. Where those
  repositories cannot be reached, `-c kotlin_compiler.asm-all_url=<url>
  -c kotlin_compiler.asm-all_sha256=<sha256>` (and the same for
  `protobuf-lite`) point at another copy.
- The project covers the compiler's JVM command line only: no tests, no
  standard library, no other platforms.
