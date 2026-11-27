# maven_to_buck

Generates BUCK files for Maven dependencies, so a Java, Kotlin or Android project can use
libraries from Google's Maven and Maven Central without writing a `remote_file` and a
`prebuilt_jar` or `android_prebuilt_aar` for each one by hand. It needs Python 3.9 or newer and
nothing else.

List the libraries you use, one `group:artifact:version` per line:

```
# app/libs/external_deps.txt
androidx.activity:activity-ktx:1.10.1
androidx.compose.material3:material3:1.3.2
junit:junit:4.13.2
```

Then run, from the project root:

```sh
python3 path/to/buck2/tools/maven_to_buck/maven_to_buck.py \
    app/libs/external_deps.txt --out app/libs --package //app/libs
```

It resolves the compile and runtime dependencies of those libraries and writes one BUCK file per
artifact, such as `app/libs/androidx/activity/activity-ktx/BUCK`. Depend on
`//app/libs/androidx/activity/activity-ktx:activity-ktx`.

How it resolves:

- When two libraries ask for different versions of a third, the highest version wins, as in
  Gradle. The constraints AndroidX publishes (in each POM's `dependencyManagement`) count too, so
  the AndroidX libraries of one family stay on one version.
- Test, provided and optional dependencies are left out, as are exclusions.
- Parent POMs, properties, imported BOMs and version ranges are followed.

Options:

- `--repo URL`: the repositories to search, in order. The default is Google's Maven, then Maven
  Central.
- `--mirror URL`: download everything from a mirror of those repositories. The BUCK files still
  name the public repositories; with a mirror, AndroidX and other Google groups get Google's Maven
  and the rest Maven Central.
- `--cache DIR`: where downloaded POMs and checksums are kept between runs
  (`~/.cache/buck2-maven-to-buck`).
- `--prune`: delete the BUCK files of artifacts that are no longer needed. Only files this tool
  wrote are ever deleted.

To change a version, edit the list and run the tool again.
