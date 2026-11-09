/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.stepsbuilder.javacd.main;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.cd.AbiDirWriter;
import com.facebook.buck.jvm.cd.BuildCommandStepsBuilder;
import com.facebook.buck.jvm.cd.DepFileUtils;
import com.facebook.buck.jvm.cd.JvmCDCommand;
import com.facebook.buck.jvm.cd.command.PostBuildParams;
import com.facebook.buck.jvm.cd.command.java.BuildJavaCommand;
import com.facebook.buck.jvm.java.CompilerClassToSourceMap;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.JavaStepsBuilder;
import com.facebook.buck.jvm.java.abi.StubJar;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableMap;
import com.google.protobuf.util.JsonFormat;
import java.io.FileReader;
import java.io.IOException;
import java.io.Reader;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.stream.Collectors;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Represents a javacd command invocation, on the command line or sent to a worker */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavaCDCommand implements JvmCDCommand {
  @Option(name = "--action-id", required = true)
  public String actionId;

  @Option(name = "--command-file", required = true)
  public Path commandFile;

  @Option(name = "--logging-level")
  private int loggingLevel = 0;

  private final JavaStepsBuilder stepsBuilder;
  private final ImmutableMap<Path, Path> jarToJarDirMap;
  private final PostBuildParams postBuildParams;
  private final CompilerOutputPaths compilerOutputPaths;
  private final List<Path> sourcePaths;

  public JavaCDCommand(String[] args, ImmutableMap<String, String> env)
      throws CmdLineException, IOException {
    CmdLineParser parser = new CmdLineParser(this);
    try {
      parser.parseArgument(args);
    } catch (CmdLineException e) {
      System.err.println(e.getMessage());
      parser.printUsage(System.err);
      throw e;
    }
    com.facebook.buck.cd.model.java.BuildJavaCommand.Builder builder =
        com.facebook.buck.cd.model.java.BuildJavaCommand.newBuilder();
    try (Reader reader = new FileReader(commandFile.toFile())) {
      JsonFormat.parser().ignoringUnknownFields().merge(reader, builder);
    }
    RelPath buckScratchPath =
        RelPath.get(Objects.requireNonNull(env.get(WORKING_DIRECTORY_ENV_VAR)));

    com.facebook.buck.cd.model.java.BuildJavaCommand proto = builder.build();
    BuildJavaCommand buildJavaCommand =
        BuildJavaCommand.Companion.fromProto(proto.getBuildCommand(), Optional.of(buckScratchPath));
    this.postBuildParams = PostBuildParams.Companion.fromProto(proto.getPostBuildParams());
    this.stepsBuilder = new JavaStepsBuilder(buildJavaCommand);
    this.jarToJarDirMap = buildJavaCommand.getBaseJarCommand().getJarToJarDirMap();
    this.compilerOutputPaths =
        buildJavaCommand
            .getBaseJarCommand()
            .getCompilerOutputPathsValue()
            .getLibraryCompilerOutputPath();
    CompilerClassToSourceMap.prepare(compilerOutputPaths, stepsBuilder.getRuleCellRoot().getPath());
    this.sourcePaths =
        buildJavaCommand.getBaseJarCommand().getJavaSrcs().stream()
            .map(RelPath::getPath)
            .collect(Collectors.toList());
  }

  public void maybeWriteClassAbi() throws IOException {
    if (!postBuildParams.getShouldCreateClassAbi()) {
      return;
    }

    AbsPath root = AbsPath.of(Paths.get(".").toAbsolutePath().normalize());
    Preconditions.checkState(postBuildParams.getAbiJar() != null);
    StubJar stubJar =
        new StubJar(root.resolve(Objects.requireNonNull(postBuildParams.getLibraryJar())), false);
    stubJar.writeTo(root.resolve(Objects.requireNonNull(postBuildParams.getAbiJar())));
  }

  public void maybeWriteAbiDir() throws IOException {
    if (postBuildParams.getAbiOutputDir() == null) {
      Preconditions.checkState(postBuildParams.getAbiJar() == null);
      return;
    }

    if (postBuildParams.getAbiJar() != null) {
      AbiDirWriter.writeAbiOutputDir(
          postBuildParams.getAbiJar(), postBuildParams.getAbiOutputDir());
    } else {
      throw new RuntimeException("Asked for an ABI dir but there is no ABI!");
    }
  }

  public void maybeWriteDepFile() throws IOException {
    Preconditions.checkState(
        (postBuildParams.getDepFile() == null)
            == (postBuildParams.getUsedClassesPaths().isEmpty()));
    if (postBuildParams.getDepFile() != null) {
      DepFileUtils.usedClassesToDepFile(
          postBuildParams.getUsedClassesPaths(),
          postBuildParams.getDepFile(),
          jarToJarDirMap,
          false);
    }
  }

  public void maybeWriteUsedJarsFile() throws IOException {
    if (postBuildParams.getUsedJarsPath() == null) {
      return;
    }

    DepFileUtils.usedClassesToUsedJars(
        postBuildParams.getUsedClassesPaths(), postBuildParams.getUsedJarsPath(), false);
  }

  @Override
  public BuildCommandStepsBuilder getBuildCommand() {
    return stepsBuilder;
  }

  @Override
  public String getActionId() {
    return actionId;
  }

  @Override
  public int getLoggingLevel() {
    return loggingLevel;
  }

  @Override
  public void postExecute() throws IOException {
    CompilerClassToSourceMap.writeCombined(
        compilerOutputPaths, stepsBuilder.getRuleCellRoot().getPath(), sourcePaths);
    maybeWriteClassAbi();
    maybeWriteAbiDir();
    maybeWriteDepFile();
    maybeWriteUsedJarsFile();
  }
}
