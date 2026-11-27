/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/// The sample project `buck2 init` writes next to the configuration it generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Template {
    /// A `genrule` that writes a file.
    #[default]
    Genrule,
    /// A Kotlin library and a `java_binary` that runs it. Needs a JDK.
    Kotlin,
    /// An Android app written in Kotlin. Needs a JDK and the Android SDK.
    Android,
}

impl Template {
    /// The sample's files: paths relative to the project root, and their contents.
    pub(crate) fn files(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Template::Genrule => &[("BUCK", GENRULE_BUCK)],
            Template::Kotlin => &[
                ("BUCK", KOTLIN_BUCK),
                ("src/com/example/Main.kt", KOTLIN_MAIN),
            ],
            Template::Android => &[
                ("BUCK", ANDROID_BUCK),
                ("AndroidManifest.xml", ANDROID_MANIFEST),
                ("debug.keystore.properties", ANDROID_KEYSTORE_PROPERTIES),
                ("res/values/strings.xml", ANDROID_STRINGS),
                ("src/com/example/app/MainActivity.kt", ANDROID_MAIN_ACTIVITY),
            ],
        }
    }
}

#[cfg(test)]
const RULES_LINK: &str = "# A list of available rules and their signatures can be found here: https://buck2.build/docs/prelude/globals/";

const GENRULE_BUCK: &str = r#"# A list of available rules and their signatures can be found here: https://buck2.build/docs/prelude/globals/

genrule(
    name = "hello_world",
    out = "out.txt",
    cmd = "echo BUILT BY BUCK2> $OUT",
)
"#;

const KOTLIN_BUCK: &str = r#"# A list of available rules and their signatures can be found here: https://buck2.build/docs/prelude/globals/
#
# `buck2 run //:main` builds and runs the program. The Kotlin compiler comes with the toolchains in
# `toolchains//`; a JDK must be on `PATH`.

kotlin_library(
    name = "lib",
    srcs = ["src/com/example/Main.kt"],
)

java_binary(
    name = "main",
    main_class = "com.example.MainKt",
    deps = [
        ":lib",
        # `kotlin_library` compiles against the Kotlin standard library but does not package it.
        "prelude//toolchains/android/third-party:kotlin-stdlib",
    ],
)
"#;

const KOTLIN_MAIN: &str = r#"package com.example

fun main() {
  println("Hello from Kotlin, built by Buck2")
}
"#;

const ANDROID_BUCK: &str = r#"# A list of available rules and their signatures can be found here: https://buck2.build/docs/prelude/globals/
#
# `buck2 build //:app` builds the APK and `buck2 install //:app` installs it on a device or an
# emulator. The Android SDK comes from `[android] sdk_path` in `.buckconfig`; a JDK must be on
# `PATH`.

android_binary(
    name = "app",
    keystore = ":debug_keystore",
    # Manifest Merger merges this with the manifests of the dependencies.
    manifest_skeleton = "AndroidManifest.xml",
    deps = [":main"],
)

android_library(
    name = "main",
    srcs = ["src/com/example/app/MainActivity.kt"],
    language = "KOTLIN",
    deps = [
        ":kotlin-stdlib",
        ":res",
    ],
)

# The Kotlin standard library the app packages, at the version of the toolchains' Kotlin compiler.
remote_file(
    name = "kotlin-stdlib.jar",
    sha1 = "c343e135bd1e7dd94b7ed76b4159068d888d09db",
    url = "mvn:org.jetbrains.kotlin:kotlin-stdlib:jar:2.4.21",
)

prebuilt_jar(
    name = "kotlin-stdlib",
    binary_jar = ":kotlin-stdlib.jar",
)

android_resource(
    name = "res",
    package = "com.example.app",
    res = "res",
)

# `buck2 init` created `debug.keystore` with the JDK's `keytool`. Use your own keystore for
# release builds.
keystore(
    name = "debug_keystore",
    properties = "debug.keystore.properties",
    store = "debug.keystore",
)
"#;

const ANDROID_MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
          package="com.example.app">

  <uses-sdk android:minSdkVersion="24"
            android:targetSdkVersion="35" />

  <application android:label="@string/app_name">
    <activity android:name=".MainActivity"
              android:exported="true">
      <intent-filter>
        <action android:name="android.intent.action.MAIN" />
        <category android:name="android.intent.category.LAUNCHER" />
      </intent-filter>
    </activity>
  </application>
</manifest>
"#;

const ANDROID_KEYSTORE_PROPERTIES: &str = "key.store=debug.keystore
key.alias=androiddebugkey
key.store.password=android
key.alias.password=android
";

const ANDROID_STRINGS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
  <string name="app_name">Buck2 App</string>
  <string name="hello">Hello from Kotlin, built by Buck2</string>
</resources>
"#;

const ANDROID_MAIN_ACTIVITY: &str = r#"package com.example.app

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

class MainActivity : Activity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val greeting = listOf(getString(R.string.hello), "Kotlin ${KotlinVersion.CURRENT}")
    setContentView(TextView(this).apply { text = greeting.joinToString("\n") })
  }
}
"#;

#[cfg(test)]
mod tests {
    use crate::commands::init_template::RULES_LINK;
    use crate::commands::init_template::Template;

    #[test]
    fn test_every_template_has_a_buck_file_with_the_rules_link() {
        for template in [Template::Genrule, Template::Kotlin, Template::Android] {
            let buck = template
                .files()
                .iter()
                .find_map(|(path, contents)| (*path == "BUCK").then_some(*contents))
                .unwrap_or_else(|| panic!("{template:?} should have a `BUCK` file"));
            assert!(
                buck.starts_with(RULES_LINK),
                "{template:?}'s `BUCK` should start with the rules link"
            );
        }
    }

    #[test]
    fn test_template_sources_are_referenced_from_buck() {
        for template in [Template::Kotlin, Template::Android] {
            let files = template.files();
            let buck = files[0].1;
            for (path, _) in files.iter().filter(|(path, _)| path.ends_with(".kt")) {
                assert!(
                    buck.contains(&format!("\"{path}\"")),
                    "{template:?}'s `BUCK` should list `{path}`"
                );
            }
        }
    }
}
