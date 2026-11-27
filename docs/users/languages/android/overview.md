---
id: overview
title: Overview
---

# Android

Buck2 builds Android apps with `android_binary`, `android_bundle`,
`android_library`, `android_resource` and the other Android rules of the
prelude. Tests run with `robolectric_test` on the JVM, or with
`android_instrumentation_test` on a device. This page covers the Android
toolchains of the prelude's `system_demo_toolchains`.

## Just need an example?

See
**[examples/android/demoapp](https://github.com/facebook/buck2/tree/main/examples/android/demoapp)**,
a Kotlin app with Jetpack Compose, resources, Robolectric and instrumentation
tests, an app bundle and a JNI library:

| Command                                    | What it does                                                                           |
| ------------------------------------------ | -------------------------------------------------------------------------------------- |
| `buck2 build //app:demoapp_debug`          | Builds the debug APK                                                                   |
| `buck2 build //app:demoapp_release`        | Builds the release APK, shrunk with ProGuard                                           |
| `buck2 build //app:demoapp_bundle`         | Builds an app bundle (AAB) for Google Play                                             |
| `buck2 install --run //app:demoapp_debug`  | Installs the APK on the device or emulator that `adb` reaches and starts it            |
| `buck2 test //app:tests`                   | Runs the Robolectric tests on the JVM                                                  |
| `buck2 test //app:android_tests`           | Installs the app and its test APK on a device or emulator and runs the tests there     |
| `buck2 test //app:native_tests`            | The same, for the app with a JNI library; needs the [NDK](#native-code)                |

To start a new project from a smaller app, run `buck2 init --template=android`.
It takes `[android] sdk_path` from `$ANDROID_HOME` when that is set.

## Requirements

- **The Android SDK**, with build tools 35.0.0 and the `android-35` platform. The
  toolchain uses the SDK in `[android] sdk_path`, `/opt/android_sdk` by default:

  ```ini
  [android]
    sdk_path = /home/me/Android/Sdk
  ```

  `[android] build_tools_version` and `[android] compile_sdk_version` choose
  other versions. If the SDK lacks a file the build needs, the build stops at
  the first step that needs it, lists the missing files, and prints the
  `sdkmanager` command that installs them.

- **A JDK**, as for [Kotlin](../kotlin/overview.md#requirements): the one in
  `[java] java_home`, or else the one whose `java` is on `PATH`. The prelude's
  tools are built for Java 21. With an older JDK, pass
  `-c java.buck2_java_version=17`.

- **Network access on the first build.** The toolchain downloads the Kotlin
  compiler and the libraries of its tools from Maven Central, and a few of its
  own tools from the `androidToolchain` release on GitHub. The demo app
  downloads its libraries from Google's Maven repository. To use a mirror of
  Maven repositories, pass `-c http.maven_repo_override=<url>`.

- **For `buck2 install` and instrumentation tests**, a device or emulator that
  `adb devices` lists. Instrumentation tests use the `adb` in the SDK's
  `platform-tools`. So does `buck2 install` when `[android] sdk_path` is an
  absolute path; otherwise it uses the `adb` on `PATH`. `[android] adb` names
  another one for `buck2 install`.

## App bundles

`android_bundle` builds an AAB. It needs a version code, which bundletool
reads from the manifest, and split dex:

```python
android_bundle(
    name = "demoapp_bundle",
    keystore = ":debug",
    manifest_entries = {
        "version_code": 1,
        "version_name": "1.0",
    },
    manifest_skeleton = "src/main/AndroidManifest.xml",
    primary_dex_patterns = ["^com/facebook/demoapp/"],
    use_split_dex = True,
    deps = [...],
)
```

Without a version code, bundletool fails with
`Version code not found in manifest`.

## Instrumentation tests

An `android_instrumentation_apk` packages the tests with
`androidx.test.runner.AndroidJUnitRunner`. Its manifest names the runner and
the app under test:

```xml
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="com.facebook.demoapp.test">
  <uses-sdk android:minSdkVersion="21" android:targetSdkVersion="35"/>
  <instrumentation
      android:name="androidx.test.runner.AndroidJUnitRunner"
      android:targetPackage="com.facebook.demoapp"/>
</manifest>
```

`buck2 test` on the `android_instrumentation_test` installs both APKs on the
first device that `adb devices` lists and runs the tests. A failed test fails
the target, and the output shows the assertion:

```
✗ Fail: root//app:android_tests (5.5s)
1 failure(s):
com.facebook.demoapp.AppContextTest#targetContextIsTheApp failed:
org.junit.ComparisonFailure: expected:<com.[example.wrong]> but was:<com.[facebook.demoapp]>
```

To choose the device, pass its serial to the test:
`buck2 test //app:android_tests -- --env ANDROID_SERIAL=<serial>`. Setting
`ANDROID_SERIAL` in the shell does not reach the test.

`buck2 run` on the same target also runs the tests, and writes
`test_result.xml` to the current directory.

## Native code

`android_binary` builds the `cxx_library` targets it depends on once for each
CPU in its `cpu_filters`, and packages each shared library under
`lib/<ABI>/`. To build them with the Android NDK, set its path:

```ini
[android]
  ndk_path = /home/me/Android/Sdk/ndk/27.1.12297006
```

Without `[android] ndk_path`, the demo toolchains compile that code with the
host's compiler.

```python
cxx_library(
    name = "greeting",
    srcs = ["greeting.cpp"],
    soname = "libgreeting.so",
)

android_binary(
    name = "demoapp_native",
    cpu_filters = ["arm64", "x86_64"],
    deps = [":greeting", ...],
    ...
)
```

The `cpu_filters` values are `arm64`, `armv7`, `x86` and `x86_64`, for the ABIs
`arm64-v8a`, `armeabi-v7a`, `x86` and `x86_64`. The toolchain:

- compiles for API level 21; projects that configure their toolchains
  themselves set another one with `android_ndk_cxx_toolchain_attrs(ndk_path,
  api_level)` from `@prelude//toolchains:android.bzl`.
- links the C++ standard library statically (`-static-libstdc++`), so the app
  needs no `libc++_shared.so`.
- aligns shared libraries to 16 KB pages, which Google Play requires for apps
  that target Android 15 and later.
- strips them with the NDK's `llvm-strip` and gives them a build ID.

## Dependencies from Maven

The demo app declares each library from Maven as a `remote_file` and an
`android_prebuilt_aar` or `prebuilt_jar`, in `app/libs/<group>/<artifact>/BUCK`.
For example, `app/libs/androidx/test/runner/BUCK`:

```python
remote_file(
    name = "runner-remote",
    out = "runner-1.6.2.aar",
    sha1 = "b65768fd399679596090ca2a9f6bcb84d89b5a50",
    url = "mvn:https://maven.google.com:androidx.test:runner:aar:1.6.2",
)

android_prebuilt_aar(
    name = "runner",
    aar = ":runner-remote",
    visibility = ["PUBLIC"],
    deps = [...],
)
```

To generate these files, list the libraries the app uses, one
`group:artifact:version` per line, and run
[`maven_to_buck`](https://github.com/facebook/buck2/tree/main/tools/maven_to_buck),
a Python script with no dependencies:

```sh
python3 maven_to_buck.py app/libs/external_deps.txt --out app/libs --package //app/libs
```

It adds the libraries' dependencies, picks one version of each library as Gradle
does, and writes one BUCK file per library. To change a version, edit the list
and run it again.

## IntelliJ and Android Studio

```sh
buck2 bxl prelude//ide_integrations/intellij/intellij.bxl:generate -- --targets //app/...
```

writes an IntelliJ project for the Java, Kotlin and Android targets in
`//app/...` and prints its directory, which IntelliJ or Android Studio opens as
it is:
- each library or test with sources is a module, with its source folders;
- its dependencies are the other modules, or else the built jars;
- Android libraries also get their manifest, resources, `android.jar` and
  generated `R` classes.

Run it again after changing the BUCK files.

## Persistent workers

The toolchain compiles Java and Kotlin in
[persistent workers](../../../rule_authors/persistent_workers.md), on by
default. `[build] use_persistent_workers = false` turns them off.
