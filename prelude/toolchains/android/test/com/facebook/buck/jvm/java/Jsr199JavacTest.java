/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import static com.facebook.buck.jvm.java.JavacLanguageLevelOptions.TARGETED_JAVA_VERSION;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.cd.model.java.BuildTargetValue.Type;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.java.abi.source.api.SourceOnlyAbiRuleInfoFactory;
import com.facebook.buck.util.ClassLoaderCache;
import com.facebook.buck.util.Verbosity;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.logging.Level;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;
import javax.tools.ToolProvider;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class Jsr199JavacTest {
  @Rule public TemporaryFolder temporaryFolder = new TemporaryFolder();
  private static final RelPath PATH_TO_SRCS_LIST = RelPath.get("srcs_list");
  public static final ImmutableSortedSet<RelPath> SOURCE_FILES =
      ImmutableSortedSet.orderedBy(RelPath.comparator()).add(RelPath.get("foobar.java")).build();

  @Test
  public void testJavacCommand() {
    Jsr199Javac.ResolvedJsr199Javac firstOrder = createTestStep();
    Jsr199Javac.ResolvedJsr199Javac warn = createTestStep();
    Jsr199Javac.ResolvedJsr199Javac transitive = createTestStep();

    assertEquals(
        String.format(
            "javac -source %s -target %s -g -d . -classpath foo.jar @%s",
            TARGETED_JAVA_VERSION, TARGETED_JAVA_VERSION, PATH_TO_SRCS_LIST),
        firstOrder.getDescription(getArgs().add("foo.jar").build(), PATH_TO_SRCS_LIST));
    assertEquals(
        String.format(
            "javac -source %s -target %s -g -d . -classpath foo.jar @%s",
            TARGETED_JAVA_VERSION, TARGETED_JAVA_VERSION, PATH_TO_SRCS_LIST),
        warn.getDescription(getArgs().add("foo.jar").build(), PATH_TO_SRCS_LIST));
    assertEquals(
        String.format(
            "javac -source %s -target %s -g -d . -classpath bar.jar%sfoo.jar @%s",
            TARGETED_JAVA_VERSION, TARGETED_JAVA_VERSION, File.pathSeparator, PATH_TO_SRCS_LIST),
        transitive.getDescription(
            getArgs().add("bar.jar" + File.pathSeparator + "foo.jar").build(), PATH_TO_SRCS_LIST));
  }

  @Test
  public void writesSourceMapAfterDiskAndDirectToJarInvocations() throws Exception {
    for (boolean directToJar : ImmutableList.of(false, true)) {
      Path root = temporaryFolder.newFolder().toPath();
      assertEquals(0, compileWithSourceMap(root, directToJar));
      assertEquals(
          Map.of(
              "p/Container.class",
              Set.of("Container.java"),
              "p/Container$Inner.class",
              Set.of("Container.java"),
              "p/Secondary.class",
              Set.of("Container.java")),
          CompilerClassToSourceMap.read(
                  root.resolve("working").resolve(CompilerClassToSourceMap.JAVA_FILE_NAME))
              .getEntries());
      if (directToJar) {
        try (ZipFile archive = new ZipFile(root.resolve("library.jar").toFile())) {
          assertTrue(archive.getEntry("p/Secondary.class") != null);
        }
      } else {
        assertTrue(Files.isRegularFile(root.resolve("classes/p/Secondary.class")));
      }
    }
  }

  @Test
  public void reportsSourceMapWriteFailure() throws Exception {
    for (boolean directToJar : ImmutableList.of(false, true)) {
      Path root = temporaryFolder.newFolder().toPath();
      Path mapPath = root.resolve("working").resolve(CompilerClassToSourceMap.JAVA_FILE_NAME);
      Files.createDirectories(mapPath);
      RuntimeException failure =
          assertThrows(RuntimeException.class, () -> compileWithSourceMap(root, directToJar));
      assertTrue(failure.getCause() instanceof IOException);
      assertEquals(
          "Failed to write compiler class-to-source map: " + mapPath,
          failure.getCause().getMessage());
      assertTrue(failure.getCause().getCause() instanceof IOException);
    }
  }

  @Test
  public void validatesInheritedMemberTypesDuringLibraryCompilation() throws Exception {
    for (boolean available : ImmutableList.of(false, true)) {
      for (boolean jarClasspath : ImmutableList.of(false, true)) {
        ByteArrayOutputStream diagnostics = new ByteArrayOutputStream();
        int result =
            compileInheritedMember(
                available, jarClasspath, AbiGenerationMode.SOURCE_ONLY, true, diagnostics);
        assertEquals(diagnostics.toString(StandardCharsets.UTF_8), available ? 0 : 1, result);
        if (!available) {
          assertTrue(
              diagnostics
                  .toString(StandardCharsets.UTF_8)
                  .contains("KOSABI_JASABI_MISSING_SUPERTYPES: dep.External"));
        }
      }
    }
  }

  @Test
  public void inheritedMemberValidationRequiresSourceOnlyModeAndReducedClasspath()
      throws Exception {
    assertEquals(
        0,
        compileInheritedMember(
            false, false, AbiGenerationMode.CLASS, true, new ByteArrayOutputStream()));
    assertEquals(
        0,
        compileInheritedMember(
            false, false, AbiGenerationMode.SOURCE_ONLY, false, new ByteArrayOutputStream()));
  }

  @Test
  public void inheritedMemberValidationWarnsDuringMigration() throws Exception {
    ByteArrayOutputStream diagnostics = new ByteArrayOutputStream();
    assertEquals(
        0,
        compileInheritedMember(
            false, false, AbiGenerationMode.MIGRATING_TO_SOURCE_ONLY, true, diagnostics));
    assertTrue(
        diagnostics
            .toString(StandardCharsets.UTF_8)
            .contains("KOSABI_JASABI_MISSING_SUPERTYPES: dep.External"));
  }

  @Test
  public void testAnnotationProcessingStillValidates() throws Exception {
    ByteArrayOutputStream diagnostics = new ByteArrayOutputStream();
    int result =
        compileInheritedMember(
            false,
            false,
            AbiGenerationMode.SOURCE_ONLY,
            true,
            diagnostics,
            "package p; import dep.External; abstract class Foo implements External { public"
                + " Entry<String, String> entry; }",
            true);
    assertEquals(diagnostics.toString(StandardCharsets.UTF_8), 1, result);
    assertTrue(diagnostics.toString(StandardCharsets.UTF_8).contains("N5_REVIEW_PROCESSOR_RAN"));
    assertTrue(
        diagnostics
            .toString(StandardCharsets.UTF_8)
            .contains("KOSABI_JASABI_MISSING_SUPERTYPES: dep.External"));
  }

  @Test
  public void testStaticImportsNeedTheirInheritancePath() throws Exception {
    for (String imported : ImmutableList.of("Entry", "*")) {
      for (boolean available : ImmutableList.of(false, true)) {
        ByteArrayOutputStream diagnostics = new ByteArrayOutputStream();
        int result =
            compileInheritedMember(
                available,
                false,
                AbiGenerationMode.SOURCE_ONLY,
                true,
                diagnostics,
                "package p; import dep.External; import static dep.External."
                    + imported
                    + "; class Foo { public Entry<String, String> entry; }",
                false);
        assertEquals(diagnostics.toString(StandardCharsets.UTF_8), available ? 0 : 1, result);
        if (!available) {
          assertTrue(
              diagnostics
                  .toString(StandardCharsets.UTF_8)
                  .contains("KOSABI_JASABI_MISSING_SUPERTYPES: dep.External"));
        }
      }
    }
  }

  @Test
  public void testRecordsDoNotCrashValidation() throws Exception {
    ByteArrayOutputStream diagnostics = new ByteArrayOutputStream();
    assertEquals(
        0,
        compileInheritedMember(
            false,
            false,
            AbiGenerationMode.MIGRATING_TO_SOURCE_ONLY,
            true,
            diagnostics,
            "package p; public record Foo(int value) {}",
            false));
  }

  private int compileInheritedMember(
      boolean available,
      boolean jarClasspath,
      AbiGenerationMode mode,
      boolean validate,
      ByteArrayOutputStream diagnostics)
      throws Exception {
    return compileInheritedMember(
        available,
        jarClasspath,
        mode,
        validate,
        diagnostics,
        "package p; import dep.External; abstract class Foo implements External { public"
            + " Entry<String, String> entry; }",
        false);
  }

  private int compileInheritedMember(
      boolean available,
      boolean jarClasspath,
      AbiGenerationMode mode,
      boolean validate,
      ByteArrayOutputStream diagnostics,
      String source,
      boolean processor)
      throws Exception {
    Path root = temporaryFolder.newFolder().toPath();
    Path dependencies = Files.createDirectory(root.resolve("dependencies"));
    Path dependencySource = root.resolve("External.java");
    Files.writeString(
        dependencySource,
        "package dep; public interface External extends java.util.Map<String, String> {}");
    assertEquals(
        0,
        ToolProvider.getSystemJavaCompiler()
            .run(null, null, null, "-d", dependencies.toString(), dependencySource.toString()));
    Path provider = dependencies;
    if (jarClasspath) {
      provider = root.resolve("dependency.jar");
      try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(provider))) {
        zip.putNextEntry(new ZipEntry("dep/External.class"));
        zip.write(Files.readAllBytes(dependencies.resolve("dep/External.class")));
        zip.closeEntry();
      }
    }
    Files.writeString(root.resolve("Foo.java"), source);
    Path classes = Files.createDirectory(root.resolve("classes"));
    CompilerOutputPaths paths =
        new CompilerOutputPaths(
            RelPath.get("classes"),
            RelPath.get("."),
            Optional.empty(),
            RelPath.get("annotations"),
            RelPath.get("srcs"),
            RelPath.get("working"),
            Optional.of(RelPath.get("library.jar")),
            RelPath.get("mapping.json"));
    SourceOnlyAbiRuleInfoFactory factory =
        new DefaultSourceOnlyAbiRuleInfoFactory("//:test", false);
    if (validate) {
      factory =
          new ReducedClasspathSourceOnlyAbiRuleInfoFactory(
              factory,
              available ? ImmutableList.of(provider) : ImmutableList.of(),
              ImmutableList.of());
    }
    JavacPluginParams annotationProcessors = JavacPluginParams.EMPTY;
    if (processor) {
      Path processorSource = root.resolve("NoOpProcessor.java");
      Files.writeString(
          processorSource,
          "public class NoOpProcessor extends javax.annotation.processing.AbstractProcessor {"
              + " public java.util.Set<String> getSupportedAnnotationTypes() { return"
              + " java.util.Set.of(\"*\"); } public javax.lang.model.SourceVersion"
              + " getSupportedSourceVersion() { return"
              + " javax.lang.model.SourceVersion.latestSupported(); } public boolean"
              + " process(java.util.Set<? extends javax.lang.model.element.TypeElement>"
              + " annotations, javax.annotation.processing.RoundEnvironment round) {"
              + " processingEnv.getMessager().printMessage(javax.tools.Diagnostic.Kind.NOTE,"
              + " \"N5_REVIEW_PROCESSOR_RAN\"); return false; } }");
      assertEquals(
          0,
          ToolProvider.getSystemJavaCompiler()
              .run(null, null, null, "-d", dependencies.toString(), processorSource.toString()));
      annotationProcessors =
          new JavacPluginParams(
              ImmutableList.of(
                  new ResolvedJavacPluginProperties(
                      false,
                      false,
                      true,
                      true,
                      ImmutableSortedSet.of("NoOpProcessor"),
                      ImmutableList.of(RelPath.get("dependencies")),
                      ImmutableMap.of(),
                      ImmutableList.of())),
              ImmutableSortedSet.of());
    }
    try (ClassLoaderCache cache = new ClassLoaderCache();
        PrintStream stderr = new PrintStream(diagnostics, true, StandardCharsets.UTF_8);
        Jsr199JavacInvocation invocation =
            new Jsr199JavacInvocation(
                ToolProvider::getSystemJavaCompiler,
                new JavacExecutionContext(
                    stderr,
                    cache,
                    Verbosity.STANDARD_INFORMATION,
                    AbsPath.of(root),
                    RelPath.get("buck-out")),
                new BuildTargetValue(Type.LIBRARY, "//:test"),
                new CompilerOutputPathsValue("//:test", paths, paths, paths),
                ImmutableList.of(
                    "-d",
                    classes.toString(),
                    "-classpath",
                    provider.toString(),
                    processor ? "-proc:full" : "-proc:none"),
                annotationProcessors,
                JavacPluginParams.EMPTY,
                ImmutableSortedSet.orderedBy(RelPath.comparator())
                    .add(RelPath.get("Foo.java"))
                    .build(),
                RelPath.get("srcs"),
                false,
                null,
                null,
                mode,
                AbiGenerationMode.CLASS,
                factory)) {
      return invocation.buildClasses();
    }
  }

  private int compileWithSourceMap(Path root, boolean directToJar) throws Exception {
    Path classes = Files.createDirectory(root.resolve("classes"));
    Files.writeString(
        root.resolve("Container.java"),
        "package p; public class Container { class Inner {} } class Secondary {}");
    CompilerOutputPaths paths =
        new CompilerOutputPaths(
            RelPath.get("classes"),
            RelPath.get("."),
            Optional.empty(),
            RelPath.get("annotations"),
            RelPath.get("srcs"),
            RelPath.get("working"),
            Optional.of(RelPath.get("library.jar")),
            RelPath.get("mapping.json"));
    CompilerOutputPathsValue outputs = new CompilerOutputPathsValue("//:test", paths, paths, paths);
    JarParameters jar =
        directToJar
            ? new JarParameters(
                false,
                true,
                RelPath.get("library.jar"),
                ignored -> false,
                ImmutableSortedSet.orderedBy(RelPath.comparator()).build(),
                ImmutableSortedSet.orderedBy(RelPath.comparator()).build(),
                Optional.empty(),
                Optional.empty(),
                Level.OFF)
            : null;
    try (ClassLoaderCache cache = new ClassLoaderCache();
        Jsr199JavacInvocation invocation =
            new Jsr199JavacInvocation(
                ToolProvider::getSystemJavaCompiler,
                new JavacExecutionContext(
                    System.err, cache, Verbosity.SILENT, AbsPath.of(root), RelPath.get("buck-out")),
                new BuildTargetValue(Type.LIBRARY, "//:test"),
                outputs,
                ImmutableList.of("-d", classes.toString(), "-proc:none"),
                JavacPluginParams.EMPTY,
                JavacPluginParams.EMPTY,
                ImmutableSortedSet.orderedBy(RelPath.comparator())
                    .add(RelPath.get("Container.java"))
                    .build(),
                RelPath.get("srcs"),
                false,
                null,
                jar,
                AbiGenerationMode.CLASS,
                AbiGenerationMode.CLASS,
                null)) {
      return invocation.buildClasses();
    }
  }

  private Jsr199Javac.ResolvedJsr199Javac createTestStep() {
    return JdkProvidedInMemoryJavac.createJsr199Javac();
  }

  private ImmutableList.Builder<String> getArgs() {
    return ImmutableList.<String>builder()
        .add(
            "-source",
            TARGETED_JAVA_VERSION,
            "-target",
            TARGETED_JAVA_VERSION,
            "-g",
            "-d",
            ".",
            "-classpath");
  }
}
