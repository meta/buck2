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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.util.json.ObjectMappers;
import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.type.TypeReference;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableMultimap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSetMultimap;
import com.google.common.collect.Ordering;
import com.google.common.collect.TreeMultimap;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Comparator;
import java.util.LinkedList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.SortedMap;
import java.util.SortedSet;
import java.util.function.Function;
import java.util.stream.Stream;
import org.jetbrains.annotations.Nullable;
import org.kohsuke.args4j.CmdLineException;
import org.kohsuke.args4j.CmdLineParser;
import org.kohsuke.args4j.Option;

/** Main entry point for constructing an {@link APKModuleGraph} from an external graph. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class APKModuleGraphExecutableMain {

  @Option(name = "--root-target", required = true)
  private String rootTarget;

  @Option(name = "--target-graph", required = true)
  private String targetGraphPath;

  @Option(name = "--seed-config-map", required = true)
  private String seedConfigMapPath;

  @Option(name = "--app-module-dependencies-map", required = true)
  private String appModuleDependenciesPath;

  @Option(name = "--output", required = true)
  private String outputPath;

  @Option(name = "--always-in-main-apk-seeds")
  @Nullable
  private String alwaysInMainApkSeedsPath = null;

  @Option(name = "--targets-to-jars")
  @Nullable
  private String targetsToJarsPath = null;

  @Option(name = "--targets-to-so-names")
  @Nullable
  private String targetsToSoNamesPath = null;

  @Option(name = "--targets-to-non-asset-prebuilt-native-library-dirs")
  @Nullable
  private String targetsToNonAssetPrebuiltNativeLibraryDirsPath = null;

  @Option(name = "--output-module-info-and-target-to-module-only")
  private boolean outputModuleInfoAndTargetToModuleOnly;

  public static void main(String[] args) throws IOException {
    APKModuleGraphExecutableMain main = new APKModuleGraphExecutableMain();
    CmdLineParser parser = new CmdLineParser(main);
    try {
      parser.parseArgument(args);
      main.run();
      System.exit(0);
    } catch (CmdLineException e) {
      System.err.println(e.toString());
      parser.printUsage(System.err);
      System.exit(1);
    }
  }

  private void run() throws IOException {
    Map<String, ImmutableList<String>> rawTargetGraphMap =
        Objects.requireNonNull(
            ObjectMappers.READER.readValue(
                ObjectMappers.createParser(Paths.get(targetGraphPath)),
                new TypeReference<Map<String, ImmutableList<String>>>() {}),
            "Expected non-null target graph from " + targetGraphPath);

    ExternalTargetGraph targetGraph = buildTargetGraph(rawTargetGraphMap);

    Map<String, ImmutableList<String>> rawSeedConfigMap =
        Objects.requireNonNull(
            ObjectMappers.READER.readValue(
                ObjectMappers.createParser(Paths.get(seedConfigMapPath)),
                new TypeReference<Map<String, ImmutableList<String>>>() {}),
            "Expected non-null seed config map from " + seedConfigMapPath);
    ImmutableMap<String, ImmutableList<ExternalTargetGraph.ExternalBuildTarget>> seedConfigMap =
        rawSeedConfigMap.entrySet().stream()
            .collect(
                ImmutableMap.toImmutableMap(
                    Map.Entry::getKey,
                    e ->
                        e.getValue().stream()
                            .map(
                                buildTargetName -> {
                                  ExternalTargetGraph.ExternalBuildTarget buildTarget =
                                      targetGraph.getBuildTarget(buildTargetName);
                                  Preconditions.checkState(
                                      buildTarget != null,
                                      "Expected to find a build target for: " + buildTargetName);
                                  return buildTarget;
                                })
                            .collect(ImmutableList.toImmutableList())));

    ImmutableMap<String, ImmutableList<String>> appModuleDependenciesMap =
        Objects.requireNonNull(
            ObjectMappers.READER.readValue(
                ObjectMappers.createParser(Paths.get(appModuleDependenciesPath)),
                new TypeReference<ImmutableMap<String, ImmutableList<String>>>() {}),
            "Expected non-null app module dependencies map from " + appModuleDependenciesPath);

    Optional<List<ExternalTargetGraph.ExternalBuildTarget>> alwaysInMainApkSeeds =
        alwaysInMainApkSeedsPath == null
            ? Optional.empty()
            : Optional.of(
                Files.readAllLines(Paths.get(alwaysInMainApkSeedsPath)).stream()
                    .map(targetGraph::getBuildTarget)
                    .filter(Objects::nonNull)
                    .collect(ImmutableList.toImmutableList()));

    ExternalTargetGraph.ExternalBuildTarget rootBuildTarget =
        targetGraph.getBuildTarget(rootTarget);
    Preconditions.checkState(
        rootBuildTarget != null, "Expected to find a build target for root: " + rootTarget);

    APKModuleGraph<ExternalTargetGraph.ExternalBuildTarget> apkModuleGraph =
        new APKModuleGraph<>(
            targetGraph,
            rootBuildTarget,
            Optional.of(seedConfigMap),
            Optional.of(appModuleDependenciesMap),
            alwaysInMainApkSeeds);

    if (outputModuleInfoAndTargetToModuleOnly) {
      Files.write(
          Paths.get(outputPath), getModuleInfoAndTargetToModuleMappingLines(apkModuleGraph));
      return;
    }

    Optional<ImmutableMultimap<APKModule, String>> apkModuleToClassesMap = Optional.empty();
    if (targetsToJarsPath != null) {
      ImmutableMultimap.Builder<APKModule, Path> builder = ImmutableSetMultimap.builder();
      for (String line : Files.readAllLines(Paths.get(targetsToJarsPath))) {
        String[] parts = line.split(" ");
        Preconditions.checkState(parts.length == 2);
        ExternalTargetGraph.ExternalBuildTarget target = targetGraph.getBuildTarget(parts[0]);
        APKModule module =
            target != null
                ? apkModuleGraph.findModuleForTarget(target)
                : apkModuleGraph.getRootAPKModule();
        builder.put(module, Paths.get(parts[1]));
      }

      apkModuleToClassesMap =
          Optional.of(
              APKModuleGraph.getAPKModuleToClassesMap(
                  builder.build(),
                  Function.identity(),
                  AbsPath.of(Paths.get(".").normalize().toAbsolutePath()),
                  ImmutableSet.of()));
    }

    ImmutableMultimap.Builder<APKModule, String> builder = ImmutableSetMultimap.builder();
    if (targetsToSoNamesPath != null) {
      for (String line : Files.readAllLines(Paths.get(targetsToSoNamesPath))) {
        String[] parts = line.split(" ");
        // First part is the target string, second part is the so name.
        Preconditions.checkState(parts.length == 2);
        ExternalTargetGraph.ExternalBuildTarget target = targetGraph.getBuildTarget(parts[0]);
        APKModule module =
            target != null
                ? apkModuleGraph.findModuleForTarget(target)
                : apkModuleGraph.getRootAPKModule();
        builder.put(module, parts[1]);
      }
    }

    if (targetsToNonAssetPrebuiltNativeLibraryDirsPath != null) {
      for (String line :
          Files.readAllLines(Paths.get(targetsToNonAssetPrebuiltNativeLibraryDirsPath))) {
        String[] parts = line.split(" ");
        // First part is the target string, second part is the prebuilt native library dir.
        Preconditions.checkState(parts.length == 2);
        ExternalTargetGraph.ExternalBuildTarget target = targetGraph.getBuildTarget(parts[0]);
        APKModule module =
            target != null
                ? apkModuleGraph.findModuleForTarget(target)
                : apkModuleGraph.getRootAPKModule();
        Path nonAssetPrebuiltNativeLibraryDir = Paths.get(parts[1]);
        try (Stream<Path> paths = Files.walk(nonAssetPrebuiltNativeLibraryDir)) {
          paths.forEach(
              libPath -> {
                if (!libPath.toFile().isFile()) {
                  return;
                }
                final String soName = libPath.getFileName().toString();
                builder.put(module, soName);
              });
        }
      }
    }

    ImmutableMultimap<APKModule, String> apkModuleToNativeLibraryMap = builder.build();

    List<String> metadataLines =
        APKModuleMetadataUtil.getMetadataLines(
            apkModuleGraph,
            ExternalTargetGraph.ExternalBuildTarget::getName,
            apkModuleToClassesMap,
            apkModuleToNativeLibraryMap.isEmpty()
                ? Optional.empty()
                : Optional.of(apkModuleToNativeLibraryMap));

    Files.write(Paths.get(outputPath), metadataLines);
  }

  private ExternalTargetGraph buildTargetGraph(
      Map<String, ImmutableList<String>> rawTargetGraphMap) {
    ImmutableMap<String, ExternalTargetGraph.ExternalBuildTarget> buildTargetMap =
        rawTargetGraphMap.keySet().stream()
            .collect(
                ImmutableMap.toImmutableMap(
                    Function.identity(), ExternalTargetGraph.ExternalBuildTarget::new));

    ImmutableMap<ExternalTargetGraph.ExternalBuildTarget, ExternalTargetGraph.ExternalTargetNode>
        underlyingMap =
            rawTargetGraphMap.entrySet().stream()
                .collect(
                    ImmutableMap.toImmutableMap(
                        entry -> buildTargetMap.get(entry.getKey()),
                        entry ->
                            new ExternalTargetGraph.ExternalTargetNode(
                                Objects.requireNonNull(buildTargetMap.get(entry.getKey())),
                                entry.getValue().stream()
                                    .map(buildTargetMap::get)
                                    .collect(ImmutableSet.toImmutableSet()))));

    return new ExternalTargetGraph(underlyingMap, buildTargetMap);
  }

  /**
   * API is: first line contains single integer N, the number of modules next N lines contain
   * 'module_name canary_class_name module_dep1 module_dep2 ... module_depN' all other lines contain
   * 'target module_containing_target'
   */
  private List<String> getModuleInfoAndTargetToModuleMappingLines(
      APKModuleGraph<ExternalTargetGraph.ExternalBuildTarget> apkModuleGraph) {
    // Module to module deps map is already sorted
    SortedMap<APKModule, ? extends SortedSet<APKModule>> moduleToDepsMap =
        apkModuleGraph.toOutgoingEdgesMap();
    LinkedList<String> metadataLines = new LinkedList<>();
    metadataLines.add(Integer.toString(moduleToDepsMap.size()));
    for (Map.Entry<APKModule, ? extends SortedSet<APKModule>> entry : moduleToDepsMap.entrySet()) {
      APKModule module = entry.getKey();
      ImmutableList<String> moduleDeps =
          entry.getValue().stream()
              .map(APKModule::getName)
              .collect(ImmutableList.toImmutableList());
      metadataLines.add(
          String.format(
              "%s %s %s",
              module.getName(), module.getCanaryClassName(), String.join(" ", moduleDeps)));
    }

    TreeMultimap<APKModule, String> orderedModuleToTargetsMap =
        TreeMultimap.create(Comparator.comparing(APKModule::getName), Ordering.natural());
    for (APKModule module : apkModuleGraph.getAPKModules()) {
      for (ExternalTargetGraph.ExternalBuildTarget target :
          apkModuleGraph.getBuildTargets(module)) {
        orderedModuleToTargetsMap.put(module, target.getName());
      }
    }
    for (APKModule module : orderedModuleToTargetsMap.keySet()) {
      String moduleName = module.getName();
      for (String target : orderedModuleToTargetsMap.get(module)) {
        metadataLines.add(String.format("%s %s", target, moduleName));
      }
    }

    return metadataLines;
  }
}
