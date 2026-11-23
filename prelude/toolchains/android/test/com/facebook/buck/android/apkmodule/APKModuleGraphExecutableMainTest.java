/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.apkmodule;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class APKModuleGraphExecutableMainTest {

  @Rule public TemporaryFolder tmp = new TemporaryFolder();

  @Test
  public void testTargetsToClassNamesJsonGivesSameMetadataAsTargetsToJars() throws Exception {
    ImmutableList<String> graphArgs =
        graphArgs(
            "{\"//app:app\": [\"//lib:a\", \"//lib:b\", \"//lib:common\"],"
                + " \"//lib:a\": [\"//lib:a_prebuilt\", \"//lib:common\"],"
                + " \"//lib:a_prebuilt\": [],"
                + " \"//lib:b\": [\"//lib:common\"],"
                + " \"//lib:common\": []}");
    String prebuiltJar = writeJar("a_prebuilt.jar", "com/a/prebuilt/P", "com/shared/Dup");

    // `//lib:missing` is not in the target graph, so its classes belong to the root module.
    Path fromJars =
        runMain(
            graphArgs,
            "from_jars.txt",
            "--targets-to-jars",
            writeFile(
                "targets_to_jars.txt",
                "//lib:a " + writeJar("a.jar", "com/a/A", "com/a/A$Inner", "com/shared/Dup"),
                "//lib:a_prebuilt " + prebuiltJar,
                "//lib:b " + writeJar("b.jar", "com/b/B", "com/shared/Dup"),
                "//lib:common " + writeJar("common.jar", "com/common/C"),
                "//lib:missing " + writeJar("missing.jar", "com/missing/M")));
    String batch0 =
        writeFile(
            "batch_0.json",
            "{\"//lib:a\": [\"com/a/A\", \"com/a/A$Inner\", \"com/shared/Dup\"],"
                + " \"//lib:missing\": [\"com/missing/M\"]}");
    String batch1 =
        writeFile(
            "batch_1.json",
            "{\"//lib:b\": [\"com/b/B\", \"com/shared/Dup\"],"
                + " \"//lib:common\": [\"com/common/C\"]}");
    Path fromClassNamesJson =
        runMain(
            graphArgs,
            "from_class_names_json.txt",
            "--targets-to-jars",
            writeFile("prebuilt_targets_to_jars.txt", "//lib:a_prebuilt " + prebuiltJar),
            "--targets-to-class-names-json",
            writeFile("targets_to_class_names_json.txt", batch0, batch1));

    List<String> lines = Files.readAllLines(fromJars);
    assertEquals(
        ImmutableList.of(
            "CLASSES",
            "  dex",
            "    com/common/C",
            "    com/missing/M",
            "  module_a",
            "    com/a/A",
            "    com/a/A$Inner",
            "    com/a/prebuilt/P",
            "    com/shared/Dup",
            "  module_b",
            "    com/b/B",
            "    com/shared/Dup"),
        lines.subList(0, lines.indexOf("TARGETS")));
    assertArrayEquals(Files.readAllBytes(fromJars), Files.readAllBytes(fromClassNamesJson));
  }

  @Test
  public void testTargetsToClassNamesJsonWithoutTargetsToJars() throws Exception {
    ImmutableList<String> graphArgs =
        graphArgs("{\"//app:app\": [\"//lib:a\", \"//lib:b\"], \"//lib:a\": [], \"//lib:b\": []}");

    Path fromJars =
        runMain(
            graphArgs,
            "from_jars.txt",
            "--targets-to-jars",
            writeFile(
                "targets_to_jars.txt",
                "//lib:a " + writeJar("a.jar", "com/a/A"),
                "//lib:b " + writeJar("b.jar", "com/b/B")));
    Path fromClassNamesJson =
        runMain(
            graphArgs,
            "from_class_names_json.txt",
            "--targets-to-class-names-json",
            writeFile(
                "targets_to_class_names_json.txt",
                writeFile(
                    "batch_0.json", "{\"//lib:a\": [\"com/a/A\"], \"//lib:b\": [\"com/b/B\"]}")));

    assertArrayEquals(Files.readAllBytes(fromJars), Files.readAllBytes(fromClassNamesJson));
  }

  @Test
  public void testTargetsToClassNamesJsonFailsOnMalformedOrWrongShapeJson() throws Exception {
    ImmutableList<String> graphArgs =
        graphArgs("{\"//app:app\": [\"//lib:a\", \"//lib:b\"], \"//lib:a\": [], \"//lib:b\": []}");
    ImmutableMap<String, String> badJson =
        ImmutableMap.of(
            "malformed", "{\"//lib:a\": [\"com/a/A\"",
            "top_level_array", "[[\"com/a/A\"]]",
            "string_per_target", "{\"//lib:a\": \"com/a/A\"}",
            "object_in_list", "{\"//lib:a\": [{\"name\": \"com/a/A\"}]}");
    for (Map.Entry<String, String> entry : badJson.entrySet()) {
      Path output = tmp.getRoot().toPath().resolve(entry.getKey() + ".txt");
      Path log = tmp.getRoot().toPath().resolve(entry.getKey() + ".log");
      int exitCode =
          runMainForExitCode(
              graphArgs,
              output,
              log,
              "--targets-to-class-names-json",
              writeFile(
                  entry.getKey() + "_class_names_json.txt",
                  writeFile(entry.getKey() + ".json", entry.getValue())));
      // The JSON reader's exception escapes `main`, so the JVM exits with 1 before it writes any
      // output.
      String message = entry.getKey() + ": " + Files.readString(log);
      assertEquals(message, 1, exitCode);
      assertTrue(message, Files.readString(log).contains("com.fasterxml.jackson"));
      assertFalse(message, Files.exists(output));
    }
  }

  /**
   * Writes a graph whose modules {@code module_a} and {@code module_b} have seeds {@code //lib:a}
   * and {@code //lib:b}.
   */
  private ImmutableList<String> graphArgs(String targetGraph) throws IOException {
    return ImmutableList.of(
        "--root-target",
        "//app:app",
        "--target-graph",
        writeFile("target_graph.json", targetGraph),
        "--seed-config-map",
        writeFile(
            "seed_config_map.json", "{\"module_a\": [\"//lib:a\"], \"module_b\": [\"//lib:b\"]}"),
        "--app-module-dependencies-map",
        writeFile("app_module_dependencies_map.json", "{}"));
  }

  /** Runs the tool, asserts that it succeeds, and returns its output. */
  private Path runMain(List<String> graphArgs, String outputName, String... args) throws Exception {
    Path output = tmp.getRoot().toPath().resolve(outputName);
    Path log = tmp.getRoot().toPath().resolve(outputName + ".log");
    int exitCode = runMainForExitCode(graphArgs, output, log, args);
    assertEquals(Files.readString(log), 0, exitCode);
    return output;
  }

  /** Runs the tool in its own JVM because {@code main} calls {@code System.exit}. */
  private int runMainForExitCode(List<String> graphArgs, Path output, Path log, String... args)
      throws Exception {
    Process process =
        new ProcessBuilder(
                ImmutableList.<String>builder()
                    .add(
                        Path.of(System.getProperty("java.home"), "bin", "java").toString(),
                        "-cp",
                        System.getProperty("java.class.path"),
                        APKModuleGraphExecutableMain.class.getName())
                    .addAll(graphArgs)
                    .add("--output", output.toString())
                    .add(args)
                    .build())
            .redirectErrorStream(true)
            .redirectOutput(log.toFile())
            .start();
    assertTrue(process.waitFor(60, TimeUnit.SECONDS));
    return process.exitValue();
  }

  private String writeFile(String name, String... lines) throws IOException {
    Path path = tmp.getRoot().toPath().resolve(name);
    Files.write(path, ImmutableList.copyOf(lines));
    return path.toString();
  }

  private String writeJar(String name, String... classNames) throws IOException {
    Path path = tmp.getRoot().toPath().resolve(name);
    try (ZipOutputStream jar = new ZipOutputStream(Files.newOutputStream(path))) {
      jar.putNextEntry(new ZipEntry("META-INF/MANIFEST.MF"));
      for (String className : classNames) {
        jar.putNextEntry(new ZipEntry(className + ".class"));
      }
    }
    return path.toString();
  }
}
