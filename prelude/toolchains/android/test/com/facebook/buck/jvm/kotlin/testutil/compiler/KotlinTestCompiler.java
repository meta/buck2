/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.testutil.compiler;

import com.facebook.buck.jvm.java.testutil.compiler.Classes;
import com.facebook.buck.jvm.java.testutil.compiler.ClassesImpl;
import com.facebook.buck.util.environment.EnvVariablesProvider;
import com.google.common.collect.ImmutableList;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.jetbrains.kotlin.cli.common.arguments.K2JVMCompilerArguments;
import org.jetbrains.kotlin.cli.common.messages.MessageRenderer;
import org.jetbrains.kotlin.cli.common.messages.PrintingMessageCollector;
import org.jetbrains.kotlin.cli.jvm.K2JVMCompiler;
import org.jetbrains.kotlin.config.Services;
import org.junit.Assert;
import org.junit.rules.ExternalResource;
import org.junit.rules.TemporaryFolder;

/**
 * A [org.junit.Rule] for working with kotlinc in tests.
 *
 * <p>Add it as a public field like this:
 *
 * <pre>
 * &#64;Rule
 * public KotlinTestCompiler testCompiler = new KotlinTestCompiler();
 * </pre>
 *
 * *
 */
public class KotlinTestCompiler extends ExternalResource implements AutoCloseable {

  private final K2JVMCompiler kotlinCompiler = new K2JVMCompiler();
  private final TemporaryFolder inputFolder = new TemporaryFolder();
  private final TemporaryFolder outputFolder = new TemporaryFolder();
  private final TemporaryFolder abiOutputFolder = new TemporaryFolder();

  private final List<File> sourceFiles = new ArrayList<>();
  private final Set<String> classpath = new HashSet<>();
  private final Classes classes = new ClassesImpl(outputFolder);
  private final Classes abiClasses = new ClassesImpl(abiOutputFolder);

  public void addSourceFileContents(String fileName, String... lines) throws IOException {
    Path sourceFilePath = inputFolder.getRoot().toPath().resolve(fileName);

    sourceFilePath.toFile().getParentFile().mkdirs();
    Files.write(sourceFilePath, Arrays.asList(lines), StandardCharsets.UTF_8);

    sourceFiles.add(sourceFilePath.toFile());
  }

  public void addClasspath(Collection<Path> paths) {
    classpath.addAll(paths.stream().map(Path::toString).collect(ImmutableList.toImmutableList()));
  }

  public void compile() {
    PrintingMessageCollector collector =
        new PrintingMessageCollector(System.err, MessageRenderer.PLAIN_RELATIVE_PATHS, true);
    final String kotlinVersion = EnvVariablesProvider.getRequiredEnvVar("KOTLIN_VERSION");

    K2JVMCompilerArguments k2JVMCompilerArguments = new K2JVMCompilerArguments();
    k2JVMCompilerArguments.setNoStdlib(true);
    k2JVMCompilerArguments.setNoReflect(true);

    // Added in Kotlin 1.8 to fix SMAP StubJarTests
    k2JVMCompilerArguments.setNoSourceDebugExtension(true);

    k2JVMCompilerArguments.setFreeArgs(
        sourceFiles.stream()
            .map(File::getAbsolutePath)
            .distinct()
            .collect(ImmutableList.toImmutableList()));
    k2JVMCompilerArguments.setDestination(outputFolder.getRoot().toString());

    k2JVMCompilerArguments.setClasspath(
        String.join(File.pathSeparator, classpath)
            + File.pathSeparator
            + System.getProperty("java.class.path"));

    k2JVMCompilerArguments.setPluginOptions(
        new String[] {
          "plugin:org.jetbrains.kotlin.jvm.abi:outputDir=" + abiOutputFolder.getRoot()
        });

    kotlinCompiler.exec(collector, Services.EMPTY, k2JVMCompilerArguments);

    if (collector.hasErrors()) {
      Assert.fail("Kotlin compilation failed with errors, see stderr for details");
    }
  }

  public Classes getClasses() {
    return classes;
  }

  public Classes getAbiClasses() {
    return abiClasses;
  }

  public void init() {
    try {
      before();
    } catch (IOException ioe) {
      throw new AssertionError(ioe);
    }
  }

  @Override
  public void before() throws IOException {
    inputFolder.create();
    outputFolder.create();
    abiOutputFolder.create();
  }

  @Override
  public void after() {
    abiOutputFolder.delete();
    outputFolder.delete();
    inputFolder.delete();
  }

  @Override
  public void close() {
    after();
  }
}
