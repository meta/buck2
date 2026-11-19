/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.manifest;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.TimeUnit;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/** Tests for {@link ReplaceApplicationIdPlaceholdersExecutableMain}. */
public class ReplaceApplicationIdPlaceholdersExecutableMainTest {

  @Rule public TemporaryFolder tempFolder = new TemporaryFolder();

  private int callMain(String... args) throws Exception {
    return ReplaceApplicationIdPlaceholdersExecutableMain.runMain(args);
  }

  /**
   * Resolves the {@code java} launcher of the currently running JVM. {@code
   * ProcessHandle.current().info().command()} is unreliable under the test runner (it can point at
   * a wrapper script rather than a usable java binary), so derive it from {@code java.home}
   * instead.
   */
  private static String javaExecutable() {
    return Path.of(System.getProperty("java.home"), "bin", "java").toString();
  }

  @Test
  public void shouldReplaceApplicationIdPlaceholderInManifest() throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"${applicationId}.permission.C2D_MESSAGE\" />\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("output.xml");

    int exitCode =
        callMain(
            "--manifest", manifestFile.toString(),
            "--output", outputFile.toString());

    assertEquals(0, exitCode);
    String result = Files.readString(outputFile);
    String expected =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"com.example.app.permission.C2D_MESSAGE\" />\n"
            + "</manifest>";
    assertEquals(expected, result);
  }

  @Test
  public void shouldReplaceMultipleApplicationIdPlaceholders() throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"${applicationId}.permission.C2D_MESSAGE\" />\n"
            + "   <provider android:authorities=\"${applicationId}.provider\" />\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("output.xml");

    int exitCode =
        callMain(
            "--manifest", manifestFile.toString(),
            "--output", outputFile.toString());

    assertEquals(0, exitCode);
    String result = Files.readString(outputFile);
    assertTrue(result.contains("com.example.app.permission.C2D_MESSAGE"));
    assertTrue(result.contains("com.example.app.provider"));
  }

  @Test
  public void shouldWriteManifestWithNoPlaceholdersUnchanged() throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"com.example.app.permission.C2D_MESSAGE\" />\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("output.xml");

    int exitCode =
        callMain(
            "--manifest", manifestFile.toString(),
            "--output", outputFile.toString());

    assertEquals(0, exitCode);
    String result = Files.readString(outputFile);
    assertEquals(manifestContent, result);
  }

  @Test
  public void shouldPassSanityCheckWhenAllPlaceholdersAreReplaced() throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"${applicationId}.permission.C2D_MESSAGE\" />\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("output.xml");

    int exitCode =
        callMain(
            "--manifest",
            manifestFile.toString(),
            "--output",
            outputFile.toString(),
            "--sanity-check-placeholders");

    assertEquals(0, exitCode);
    String result = Files.readString(outputFile);
    assertEquals(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"com.example.app.permission.C2D_MESSAGE\" />\n"
            + "</manifest>",
        result);
  }

  @Test
  public void shouldExitWithErrorWhenRequiredArgsAreMissing() throws Exception {
    int exitCode = callMain();

    assertEquals(1, exitCode);
  }

  @Test
  public void shouldNotThrowWhenSanityCheckDisabledAndUnreplacedPlaceholdersExist()
      throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission\n"
            + "       android:name=\"${applicationId}.permission.C2D_MESSAGE\"\n"
            + "       android:protectionLevel=\"${unreplacedPlaceholder}\" />\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("output.xml");

    int exitCode =
        callMain(
            "--manifest", manifestFile.toString(),
            "--output", outputFile.toString());

    assertEquals(0, exitCode);
    String result = Files.readString(outputFile);
    assertTrue(result.contains("com.example.app.permission.C2D_MESSAGE"));
    assertTrue(result.contains("${unreplacedPlaceholder}"));
  }

  @Test
  public void shouldCreateOutputFileAtSpecifiedPath() throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("subdir").resolve("output.xml");
    Files.createDirectories(outputFile.getParent());

    int exitCode =
        callMain(
            "--manifest", manifestFile.toString(),
            "--output", outputFile.toString());

    assertEquals(0, exitCode);
    assertTrue(Files.exists(outputFile));
    String result = Files.readString(outputFile);
    assertEquals(manifestContent, result);
  }

  @Test
  public void mainShouldExitWithZeroOnSuccess() throws Exception {
    String manifestContent =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"
            + "<manifest\n"
            + "   xmlns:android=\"http://schemas.android.com/apk/res/android\"\n"
            + "   package=\"com.example.app\">\n"
            + "   <permission android:name=\"${applicationId}.permission.C2D_MESSAGE\" />\n"
            + "</manifest>";

    Path manifestFile = tempFolder.newFile("AndroidManifest.xml").toPath();
    Files.writeString(manifestFile, manifestContent);

    Path outputFile = tempFolder.getRoot().toPath().resolve("main_output.xml");

    ProcessBuilder pb =
        new ProcessBuilder(
            javaExecutable(),
            "-cp",
            System.getProperty("java.class.path"),
            "com.facebook.buck.android.manifest.ReplaceApplicationIdPlaceholdersExecutableMain",
            "--manifest",
            manifestFile.toString(),
            "--output",
            outputFile.toString());
    pb.redirectErrorStream(true);
    Process process = pb.start();
    assertTrue(process.waitFor(30, TimeUnit.SECONDS));
    assertEquals(0, process.exitValue());
    assertTrue(Files.exists(outputFile));
  }

  @Test
  public void mainShouldExitWithOneOnMissingArgs() throws Exception {
    ProcessBuilder pb =
        new ProcessBuilder(
            javaExecutable(),
            "-cp",
            System.getProperty("java.class.path"),
            "com.facebook.buck.android.manifest.ReplaceApplicationIdPlaceholdersExecutableMain");
    pb.redirectErrorStream(true);
    Process process = pb.start();
    assertTrue(process.waitFor(30, TimeUnit.SECONDS));
    assertEquals(1, process.exitValue());
  }
}
