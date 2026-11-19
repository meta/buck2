/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.serialization.kotlin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.cd.model.java.ResolvedJavacOptions;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.cd.command.BaseJarCommand;
import com.facebook.buck.jvm.cd.serialization.java.JarParametersSerializer;
import com.facebook.buck.jvm.cd.serialization.java.ResolvedJavacOptionsSerializer;
import com.facebook.buck.jvm.java.Jsr199Javac.ResolvedJsr199Javac;
import java.nio.file.Paths;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.logging.Level;
import org.junit.Test;

public class CompilerCommandDefaultsTest {
  @Test
  public void baseCommandUsesCompilerDefaults() {
    var model =
        com.facebook.buck.cd.model.java.BaseJarCommand.newBuilder()
            .setAbiGenerationMode(AbiGenerationMode.SOURCE_ONLY)
            .setTrackClassUsage(true)
            .setResolvedJavacOptions(javaOptions())
            .addCompileTimeClasspathPaths("dep.jar")
            .build();

    var command = BaseJarCommand.Companion.fromProto(model, Optional.of(RelPath.get("scratch")));

    assertEquals(AbiGenerationMode.CLASS, command.getAbiCompatibilityMode());
    assertEquals(AbiGenerationMode.SOURCE_ONLY, command.getAbiGenerationMode());
    assertTrue(command.getTrackClassUsage());
    assertEquals(RelPath.get("dep.jar"), command.getCompileTimeClasspathPaths().get(0));
    assertTrue(command.getJarToJarDirMap().isEmpty());
    assertEquals(RelPath.get("buck-out/v2"), command.getBuckOut());
    assertTrue(command.getResolvedJavac() instanceof ResolvedJsr199Javac);
    assertTrue(command.getResolvedJavacOptions().getDebug());
  }

  @Test
  public void abiDirectoriesPreserveClasspathOrderAndDuplicateJars() {
    var model =
        com.facebook.buck.cd.model.java.BaseJarCommand.newBuilder()
            .setResolvedJavacOptions(javaOptions())
            .addAllCompileTimeClasspathAbiAndDirPaths(
                List.of("b jar.jar", "b dir", "a.jar", "", "b jar.jar", "b dir"))
            .build();

    var command = BaseJarCommand.Companion.fromProto(model, Optional.of(RelPath.get("scratch")));

    assertEquals(
        List.of(RelPath.get("b jar.jar"), RelPath.get("a.jar"), RelPath.get("b jar.jar")),
        command.getCompileTimeClasspathPaths());
    assertEquals(Map.of(Paths.get("b jar.jar"), Paths.get("b dir")), command.getJarToJarDirMap());
  }

  @Test(expected = IllegalArgumentException.class)
  public void abiDirectoriesRejectIncompletePairs() {
    var model =
        com.facebook.buck.cd.model.java.BaseJarCommand.newBuilder()
            .setResolvedJavacOptions(javaOptions())
            .addCompileTimeClasspathAbiAndDirPaths("dep.jar")
            .build();
    BaseJarCommand.Companion.fromProto(model, Optional.empty());
  }

  @Test(expected = IllegalArgumentException.class)
  public void abiDirectoriesRejectConflictingDirectories() {
    var model =
        com.facebook.buck.cd.model.java.BaseJarCommand.newBuilder()
            .setResolvedJavacOptions(javaOptions())
            .addAllCompileTimeClasspathAbiAndDirPaths(
                List.of("dep.jar", "first-dir", "dep.jar", "second-dir"))
            .build();
    BaseJarCommand.Companion.fromProto(model, Optional.empty());
  }

  @Test(expected = IllegalArgumentException.class)
  public void abiDirectoriesRejectTwoClasspaths() {
    var model =
        com.facebook.buck.cd.model.java.BaseJarCommand.newBuilder()
            .setResolvedJavacOptions(javaOptions())
            .addCompileTimeClasspathPaths("dep.jar")
            .addAllCompileTimeClasspathAbiAndDirPaths(List.of("dep.jar", "dep-dir"))
            .build();
    BaseJarCommand.Companion.fromProto(model, Optional.empty());
  }

  @Test
  public void jarParametersUseFineLogging() {
    var model =
        com.facebook.buck.cd.model.java.JarParameters.newBuilder()
            .setJarPath("out.jar")
            .addEntriesToJar("classes")
            .build();

    var parameters = JarParametersSerializer.deserialize(model);

    assertEquals(Level.FINE, parameters.getDuplicatesLogLevel());
    assertEquals(RelPath.get("out.jar"), parameters.getJarPath());
    assertTrue(parameters.getEntriesToJar().contains(RelPath.get("classes")));
  }

  @Test
  public void annotationProcessorsKeepVariableFlagsWithoutPathParameters() {
    var model =
        javaOptions().toBuilder()
            .setJavaAnnotationProcessorParams(
                ResolvedJavacOptions.JavacPluginParams.newBuilder()
                    .addPluginProperties(
                        ResolvedJavacOptions.ResolvedJavacPluginProperties.newBuilder()
                            .setCanReuseClassLoader(true)
                            .setDoesNotAffectAbi(true)
                            .setSupportsAbiGenerationFromSource(true)
                            .setRunsOnJavaOnly(true)
                            .addProcessorNames("Processor")
                            .addClasspath("processor.jar")))
            .build();

    var processor =
        ResolvedJavacOptionsSerializer.deserialize(model)
            .getJavaAnnotationProcessorParams()
            .getPluginProperties()
            .get(0);

    assertTrue(processor.getCanReuseClassLoader());
    assertTrue(processor.getDoesNotAffectAbi());
    assertTrue(processor.getSupportAbiGenerationFromSource());
    assertTrue(processor.getRunsOnJavaOnly());
    assertTrue(processor.getPathParams().isEmpty());
    assertEquals(RelPath.get("processor.jar"), processor.getClasspath().get(0));
  }

  @Test
  public void kotlinParametersUseExactAbiTerminationMessage() {
    var model =
        com.facebook.buck.cd.model.kotlin.KotlinExtraParams.newBuilder()
            .setStandardLibraryClassPath("stdlib.jar")
            .setAnnotationProcessingClassPath("kapt.jar")
            .setLanguageVersion("2.2")
            .build();

    var parameters =
        KotlinExtraParamsSerializer.deserialize(
            javaOptions(), model, Optional.of(RelPath.get("scratch")));

    assertEquals(
        Optional.of(
            "exception: java.lang.RuntimeException: Terminating compilation. We're done with ABI."),
        parameters.getKosabiJvmAbiGenEarlyTerminationMessagePrefix());
    assertTrue(parameters.getResolvedJavacOptions().getDebug());
  }

  private static ResolvedJavacOptions javaOptions() {
    return ResolvedJavacOptions.newBuilder()
        .setLanguageLevelOptions(
            ResolvedJavacOptions.JavacLanguageLevelOptions.newBuilder()
                .setSourceLevel("8")
                .setTargetLevel("8"))
        .build();
  }

  @Test
  public void classesUseScratchForResourcesAndJarEntries() {
    for (var type :
        List.of(
            com.facebook.buck.cd.model.java.BuildTargetValue.Type.LIBRARY,
            com.facebook.buck.cd.model.java.BuildTargetValue.Type.SOURCE_ABI,
            com.facebook.buck.cd.model.java.BuildTargetValue.Type.SOURCE_ONLY_ABI)) {
      var model =
          com.facebook.buck.cd.model.java.BaseJarCommand.newBuilder()
              .setBuildTargetValue(
                  com.facebook.buck.cd.model.java.BuildTargetValue.newBuilder().setType(type))
              .setResolvedJavacOptions(javaOptions())
              .setJarParameters(
                  com.facebook.buck.cd.model.java.JarParameters.newBuilder().setJarPath("out.jar"))
              .addResourcesMap(
                  com.facebook.buck.cd.model.common.RelPathMapEntry.newBuilder()
                      .setKey("input/resource.txt")
                      .setValue("pkg/resource.txt"))
              .build();
      var command = BaseJarCommand.Companion.fromProto(model, Optional.of(RelPath.get("scratch")));
      var classes = RelPath.get("scratch/__classes__");
      assertEquals(classes, command.getCompilerOutputPathsValue().getByType(type).getClassesDir());
      assertEquals(List.of(classes), List.copyOf(command.getJarParameters().getEntriesToJar()));
      assertEquals(
          Map.of(
              RelPath.get("input/resource.txt"),
              RelPath.get("scratch/__classes__/pkg/resource.txt")),
          command.getResourcesMap());
    }
  }

  @Test
  public void kotlinClassesUseScratchOrPersistentIncrementalState() {
    for (int mode = 0; mode < 2; mode++) {
      var model =
          com.facebook.buck.cd.model.kotlin.KotlinExtraParams.newBuilder()
              .setStandardLibraryClassPath("stdlib.jar")
              .setAnnotationProcessingClassPath("kapt.jar")
              .setLanguageVersion("2.2")
              .setIncrementalStateDir("state")
              .setShouldKotlincRunIncrementally(mode == 1)
              .build();
      var parameters =
          KotlinExtraParamsSerializer.deserialize(
              javaOptions(), model, Optional.of(RelPath.get("scratch")));
      assertEquals(
          RelPath.get((mode == 0 ? "scratch" : "state") + "/__kotlin_classes__").toAbsolutePath(),
          parameters.getKotlinClassesDir());
    }
  }
}
