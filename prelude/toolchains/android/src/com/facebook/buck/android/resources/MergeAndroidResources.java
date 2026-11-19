/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources;

import com.facebook.buck.android.aapt.RDotTxtEntry;
import com.facebook.buck.android.aapt.RDotTxtEntry.RType;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.util.ThrowingPrintWriter;
import com.facebook.buck.util.json.ObjectMappers;
import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.databind.JsonNode;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Joiner;
import com.google.common.base.Preconditions;
import com.google.common.base.Strings;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSetMultimap;
import com.google.common.collect.SetMultimap;
import com.google.common.collect.Sets;
import com.google.common.collect.SortedSetMultimap;
import com.google.common.collect.TreeMultimap;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collection;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Map.Entry;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.stream.Collectors;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class MergeAndroidResources {
  private static final Logger LOG = Logger.get(MergeAndroidResources.class);

  /**
   * Merges text symbols files from {@code aapt} for each of the input {@code android_resource} into
   * a set of resources per R.java package and writes an {@code R.java} file per package under the
   * output directory. Also, if {@code uberRDotTxt} is present, the IDs in the output {@code R.java}
   * file will be taken from the {@code R.txt} file.
   */
  public static void mergeAndroidResources(
      ImmutableList<Path> uberRDotTxt,
      ImmutableMap<Path, String> symbolsFileToRDotJavaPackage,
      ImmutableMap<Path, String> symbolsFileToResourceDeps,
      boolean forceFinalResourceIds,
      EnumSet<RType> bannedDuplicateResourceTypes,
      Optional<Path> duplicateResourceWhitelistPath,
      ImmutableList<Path> overrideSymbolsPath,
      Path outputDir,
      Optional<Path> stringsOutputDir,
      Optional<Path> idsOutputDir,
      ImmutableSet<String> referencedResources)
      throws DuplicateResourceException, IOException {
    // In order to convert a symbols file to R.java, all resources of the same type are grouped
    // into a static class of that name. The static class contains static values that correspond
    // to the resource (type, name, value) tuples. See RDotTxtEntry.
    //
    // The first step is to merge symbol files of the same package type and resource type/name.
    // That is, within a package type, each resource type/name pair must be unique. If there are
    // multiple pairs, only one will be written to the R.java file.
    //
    // Because the resulting files do not match their respective resources.arsc, the values are
    // meaningless and do not represent the usable final result.  This is why the R.java file is
    // written without using final so that javac will not inline the values.
    Optional<ImmutableMap<RDotTxtEntry, String>> uberRDotTxtIds;
    if (uberRDotTxt.isEmpty()) {
      uberRDotTxtIds = Optional.empty();
    } else {
      // re-assign Ids
      ImmutableSet.Builder<RDotTxtEntry> uberRdotTxtEntries = ImmutableSet.builder();
      uberRDotTxt.forEach(
          rDot -> {
            try {
              readResources(rDot).forEach(uberRdotTxtEntries::add);
            } catch (IOException e) {
              throw new RuntimeException(e);
            }
          });
      uberRDotTxtIds =
          Optional.of(
              uberRdotTxtEntries.build().stream()
                  .collect(ImmutableMap.toImmutableMap(input -> input, input -> input.idValue)));
    }

    Optional<SetMultimap<String, RDotTxtEntry>> overrideSymbols =
        loadOverrideSymbols(overrideSymbolsPath);

    ImmutableSet<String> duplicateResourceWhitelist =
        (duplicateResourceWhitelistPath.isPresent())
            ? ImmutableSet.copyOf(Files.readAllLines(duplicateResourceWhitelistPath.get()))
            : ImmutableSet.of();

    SortedSetMultimap<String, RDotTxtEntry> rDotJavaPackageToResources =
        sortSymbols(
            symbolsFileToRDotJavaPackage,
            uberRDotTxtIds,
            symbolsFileToResourceDeps,
            overrideSymbols,
            bannedDuplicateResourceTypes,
            duplicateResourceWhitelist);

    ImmutableSet.Builder<String> requiredPackages = ImmutableSet.builder();

    requiredPackages.addAll(symbolsFileToRDotJavaPackage.values());

    Preconditions.checkState(
        stringsOutputDir.isPresent() == idsOutputDir.isPresent(),
        "Expect either both stringOutputDir and idsOutputDir, or neither");
    boolean hasStringsAndIdsOutputDir = stringsOutputDir.isPresent();

    Set<String> emptyPackages;
    if (hasStringsAndIdsOutputDir) {
      SortedSetMultimap<String, RDotTxtEntry> otherRDotJavaPackageToResources =
          TreeMultimap.create();
      SortedSetMultimap<String, RDotTxtEntry> stringsRDotJavaPackageToResources =
          TreeMultimap.create();
      SortedSetMultimap<String, RDotTxtEntry> idsRDotJavaPackageToResources = TreeMultimap.create();
      for (Entry<String, RDotTxtEntry> entry : rDotJavaPackageToResources.entries()) {
        RType resourceType = entry.getValue().type;
        if (resourceType.equals(RType.STRING)) {
          stringsRDotJavaPackageToResources.put(entry.getKey(), entry.getValue());
        } else if (resourceType.equals(RType.ID)) {
          idsRDotJavaPackageToResources.put(entry.getKey(), entry.getValue());
        } else {
          otherRDotJavaPackageToResources.put(entry.getKey(), entry.getValue());
        }
      }

      writePerPackageRDotJava(
          outputDir, otherRDotJavaPackageToResources, forceFinalResourceIds, referencedResources);
      writePerPackageRDotJava(
          stringsOutputDir.get(),
          stringsRDotJavaPackageToResources,
          forceFinalResourceIds,
          referencedResources);
      writePerPackageRDotJava(
          idsOutputDir.get(),
          idsRDotJavaPackageToResources,
          forceFinalResourceIds,
          referencedResources);
      emptyPackages =
          Sets.difference(requiredPackages.build(), otherRDotJavaPackageToResources.keySet());
    } else {
      Preconditions.checkState(referencedResources.isEmpty());
      writePerPackageRDotJava(
          outputDir, rDotJavaPackageToResources, forceFinalResourceIds, referencedResources);
      emptyPackages =
          Sets.difference(requiredPackages.build(), rDotJavaPackageToResources.keySet());
    }

    if (!emptyPackages.isEmpty()) {
      writeEmptyRDotJavaForPackages(outputDir, emptyPackages);
    }
  }

  /**
   * Read resource IDs from a R.txt file and add them to a list of entries
   *
   * @param rDotTxt the path to the R.txt file to read
   * @return a list of RDotTxtEntry objects read from the file
   * @throws IOException
   */
  private static List<RDotTxtEntry> readResources(Path rDotTxt) throws IOException {
    return Files.readAllLines(rDotTxt).stream()
        .filter(input -> !Strings.isNullOrEmpty(input))
        .map(RDotTxtEntry.TO_ENTRY)
        .collect(Collectors.toList());
  }

  private static Optional<SetMultimap<String, RDotTxtEntry>> loadOverrideSymbols(
      Iterable<Path> paths) throws IOException {
    ImmutableSetMultimap.Builder<String, RDotTxtEntry> symbolsBuilder =
        ImmutableSetMultimap.builder();
    for (Path path : paths) {
      if (!Files.isRegularFile(path)) {
        LOG.info("Override-symbols file %s is not present or not regular.  Skipping.", path);
        continue;
      }
      JsonNode jsonData =
          Preconditions.checkNotNull(
              ObjectMappers.READER.readTree(ObjectMappers.createParser(path)));
      for (String packageName : (Iterable<String>) jsonData::fieldNames) {
        Iterator<JsonNode> rDotTxtLines =
            Objects.requireNonNull(jsonData.get(packageName)).elements();
        while (rDotTxtLines.hasNext()) {
          String rDotTxtLine = rDotTxtLines.next().asText();
          symbolsBuilder.put(packageName, parseEntryOrThrow(rDotTxtLine));
        }
      }
    }
    return Optional.of(symbolsBuilder.build());
  }

  private static void writeEmptyRDotJavaForPackages(Path outputDir, Set<String> rDotJavaPackages)
      throws IOException {
    for (String rDotJavaPackage : rDotJavaPackages) {
      Path outputFile = getPathToRDotJava(outputDir, rDotJavaPackage);
      Files.createDirectories(Objects.requireNonNull(outputFile.getParent()));
      Files.write(
          outputFile,
          String.format("package %s;\n\npublic class R {}\n", rDotJavaPackage)
              .getBytes(StandardCharsets.UTF_8));
    }
  }

  private static void writePerPackageRDotJava(
      Path outputDir,
      SortedSetMultimap<String, RDotTxtEntry> packageToResources,
      boolean forceFinalResourceIds,
      ImmutableSet<String> referencedResources)
      throws IOException {
    Files.createDirectories(outputDir);
    for (String rDotJavaPackage : packageToResources.keySet()) {
      Path outputFile = getPathToRDotJava(outputDir, rDotJavaPackage);
      Files.createDirectories(Objects.requireNonNull(outputFile.getParent()));
      try (ThrowingPrintWriter writer =
          new ThrowingPrintWriter(new FileOutputStream(outputFile.toFile()))) {
        writer.format("package %s;\n\n", rDotJavaPackage);
        writer.write("public class R {\n");

        RType lastType = null;

        for (RDotTxtEntry res : packageToResources.get(rDotJavaPackage)) {
          boolean isUsed =
              referencedResources.isEmpty()
                  || referencedResources.contains(rDotJavaPackage + "." + res.name);
          if (isUsed) {
            RType type = res.type;
            if (!Objects.equals(type, lastType)) {
              // If the previous type needs to be closed, close it.
              if (lastType != null) {
                writer.println("  }\n");
              }

              // Now start the block for the new type.
              writer.format("  public static class %s {\n", type);
              lastType = type;
            }

            // Write out the resource.
            // Write as an int.
            writer.format(
                "    public static%s%s %s=%s;\n",
                forceFinalResourceIds ? " final " : " ", res.idType, res.name, res.idValue);
          }
        }

        // If some type was written (e.g., the for loop was entered), then the last type needs to be
        // closed.
        if (lastType != null) {
          writer.println("  }\n");
        }

        // Close the class definition.
        writer.println("}");
      }
    }
  }

  @VisibleForTesting
  public static SortedSetMultimap<String, RDotTxtEntry> sortSymbols(
      Map<Path, String> symbolsFileToRDotJavaPackage,
      Optional<ImmutableMap<RDotTxtEntry, String>> uberRDotTxtIds,
      ImmutableMap<Path, String> symbolsFileToResourceDeps,
      Optional<SetMultimap<String, RDotTxtEntry>> overrides,
      EnumSet<RType> bannedDuplicateResourceTypes,
      Set<String> duplicateResourceWhitelist)
      throws DuplicateResourceException {
    Map<RDotTxtEntry, String> finalIds = null;
    if (uberRDotTxtIds.isPresent()) {
      finalIds = uberRDotTxtIds.get();
    }

    SortedSetMultimap<String, RDotTxtEntry> rDotJavaPackageToSymbolsFiles = TreeMultimap.create();
    SortedSetMultimap<RDotTxtEntry, Path> bannedDuplicateResourceToSymbolsFiles =
        TreeMultimap.create();

    // Expand the package overrides into per-package self-maps.
    // The self-maps are basically sets, but we need to be able to look up
    // the actual RDotTxtEntry objects to get their ids (which are not included in .equals()).
    Map<String, Map<RDotTxtEntry, RDotTxtEntry>> expandedPackageOverrides = ImmutableMap.of();
    if (overrides.isPresent()) {
      expandedPackageOverrides = new HashMap<>();
      Map<String, Map<RDotTxtEntry, RDotTxtEntry>> ovr = expandedPackageOverrides;
      overrides
          .get()
          .asMap()
          .forEach(
              (pkg, entries) ->
                  ovr.put(pkg, entries.stream().collect(Collectors.toMap(k -> k, v -> v))));
    }

    for (Entry<Path, String> entry : symbolsFileToRDotJavaPackage.entrySet()) {
      Path symbolsFile = entry.getKey();
      // Read the symbols file and parse each line as a Resource.
      List<RDotTxtEntry> linesInSymbolsFile;
      try {
        linesInSymbolsFile =
            Files.readAllLines(symbolsFile).stream()
                .filter(input -> !Strings.isNullOrEmpty(input))
                .map(MergeAndroidResources::parseEntryOrThrow)
                .collect(Collectors.toList());
      } catch (IOException e) {
        throw new RuntimeException(e);
      }

      String packageName = entry.getValue();
      Map<RDotTxtEntry, RDotTxtEntry> packageOverrides =
          expandedPackageOverrides.getOrDefault(packageName, ImmutableMap.of());

      if (!packageOverrides.isEmpty()) {
        // RDotTxtEntry computes hash codes and checks equality only based on type and name,
        // so we can use simple map lookup to find the overridden resource entry.
        for (int i = 0; i < linesInSymbolsFile.size(); i++) {
          RDotTxtEntry mappedEntry = packageOverrides.get(linesInSymbolsFile.get(i));
          if (mappedEntry != null) {
            linesInSymbolsFile.set(i, mappedEntry);
          }
        }
      }

      for (int index = 0; index < linesInSymbolsFile.size(); index++) {
        RDotTxtEntry resource = linesInSymbolsFile.get(index);

        if (uberRDotTxtIds.isPresent()) {
          // TODO(natthu): Make this a hard error once we fix fbandroid and remove all unreferenced
          // non-english strings.
          Objects.requireNonNull(finalIds);
          if (!finalIds.containsKey(resource)) {
            LOG.debug("Cannot find resource '%s' in the uber R.txt.", resource);
            continue;
          }
          resource = resource.copyWithNewIdValue(finalIds.get(resource));
        }

        if (bannedDuplicateResourceTypes.contains(resource.type)) {
          bannedDuplicateResourceToSymbolsFiles.put(resource, symbolsFile);
        }
        rDotJavaPackageToSymbolsFiles.put(packageName, resource);
      }
    }

    // Find any "overridden" resources that were actually new resources and add them.
    Map<RDotTxtEntry, String> finalFinalIds = finalIds;
    overrides.ifPresent(
        ovr ->
            ovr.forEach(
                (pkg, resource) -> {
                  Objects.requireNonNull(finalFinalIds);
                  if (!rDotJavaPackageToSymbolsFiles.containsEntry(pkg, resource)) {
                    String realId = finalFinalIds.get(resource);
                    Preconditions.checkState(
                        realId != null,
                        "ID for resource created by filtering is not present in R.txt: %s",
                        resource);
                    rDotJavaPackageToSymbolsFiles.put(pkg, resource.copyWithNewIdValue(realId));
                  }
                }));

    StringBuilder duplicateResourcesMessage = new StringBuilder();
    for (Entry<RDotTxtEntry, Collection<Path>> resourceAndSymbolsFiles :
        bannedDuplicateResourceToSymbolsFiles.asMap().entrySet()) {
      Collection<Path> paths = resourceAndSymbolsFiles.getValue();
      RDotTxtEntry resource = resourceAndSymbolsFiles.getKey();
      if (paths.size() > 1 && !duplicateIsWhitelisted(resource, duplicateResourceWhitelist)) {
        duplicateResourcesMessage.append(
            String.format(
                "Resource '%s' (%s) is duplicated across: ", resource.name, resource.type));
        duplicateResourcesMessage.append(
            Joiner.on(", ")
                .join(
                    paths.stream()
                        .map(symbolsFileToResourceDeps::get)
                        .filter(Objects::nonNull)
                        .collect(Collectors.toList())));
        duplicateResourcesMessage.append("\n");
      }
    }

    if (duplicateResourcesMessage.length() > 0) {
      throw new DuplicateResourceException(duplicateResourcesMessage.toString());
    }

    return rDotJavaPackageToSymbolsFiles;
  }

  private static boolean duplicateIsWhitelisted(RDotTxtEntry resource, Set<String> whitelist) {
    return whitelist.contains(resource.type.toString().toLowerCase() + " " + resource.name);
  }

  private static RDotTxtEntry parseEntryOrThrow(String line) {
    Optional<RDotTxtEntry> parsedEntry = RDotTxtEntry.parse(line);
    Preconditions.checkState(parsedEntry.isPresent(), "Should be able to match '%s'.", line);

    return parsedEntry.get();
  }

  /** Returns {@link Path} to R. java file */
  public static Path getPathToRDotJava(Path outputDir, String rDotJavaPackage) {
    return getPathToJavaFile(outputDir, rDotJavaPackage, "R.java");
  }

  static Path getPathToJavaFile(Path outputDir, String javaPackage, String fileName) {
    return outputDir.resolve(javaPackage.replace('.', '/')).resolve(fileName);
  }

  public static class DuplicateResourceException extends Exception {
    DuplicateResourceException(String messageFormat, Object... args) {
      super(String.format(messageFormat, args));
    }
  }
}
