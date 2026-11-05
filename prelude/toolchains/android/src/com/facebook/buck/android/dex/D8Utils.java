/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex;

import com.android.tools.r8.ByteDataView;
import com.android.tools.r8.CompilationFailedException;
import com.android.tools.r8.CompilationMode;
import com.android.tools.r8.D8Command;
import com.android.tools.r8.DexFilePerClassFileConsumer;
import com.android.tools.r8.DexIndexedConsumer;
import com.android.tools.r8.Diagnostic;
import com.android.tools.r8.DiagnosticsHandler;
import com.android.tools.r8.SyntheticInfoConsumer;
import com.android.tools.r8.SyntheticInfoConsumerData;
import com.android.tools.r8.graph.DexItemFactory;
import com.android.tools.r8.utils.FileUtils;
import com.android.tools.r8.utils.InternalOptions;
import com.facebook.buck.android.apkmodule.APKModule;
import com.facebook.buck.util.zip.CustomZipOutputStream;
import com.facebook.buck.util.zip.ZipOutputStreams;
import com.facebook.buck.util.zip.ZipScrubber;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.hash.Hashing;
import com.google.common.io.ByteStreams;
import java.io.BufferedInputStream;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.OptionalInt;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.Stream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/** Runs d8. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class D8Utils {

  public static D8Output runD8Command(
      D8DiagnosticsHandler diagnosticsHandler,
      Path outputDexFile,
      Iterable<Path> filesToDex,
      Set<D8Options> options,
      Optional<Path> primaryDexClassNamesPath,
      Path androidJarPath,
      Collection<Path> classpathFiles,
      Optional<Integer> minSdkVersion)
      throws CompilationFailedException, IOException {
    return runD8Command(
        diagnosticsHandler,
        outputDexFile,
        filesToDex,
        options,
        primaryDexClassNamesPath,
        androidJarPath,
        classpathFiles,
        minSdkVersion,
        OptionalInt.empty());
  }

  public static D8Output runD8Command(
      D8DiagnosticsHandler diagnosticsHandler,
      Path outputDexFile,
      Iterable<Path> filesToDex,
      Set<D8Options> options,
      Optional<Path> primaryDexClassNamesPath,
      Path androidJarPath,
      Collection<Path> classpathFiles,
      Optional<Integer> minSdkVersion,
      OptionalInt threadCount)
      throws CompilationFailedException, IOException {
    return runD8Command(
        diagnosticsHandler,
        outputDexFile,
        filesToDex,
        options,
        primaryDexClassNamesPath,
        androidJarPath,
        classpathFiles,
        minSdkVersion,
        threadCount,
        ImmutableSet.of());
  }

  public static D8Output runD8CommandWithOutputClassDescriptors(
      D8DiagnosticsHandler diagnosticsHandler,
      Path outputDexFile,
      Iterable<Path> filesToDex,
      Set<D8Options> options,
      Optional<Path> primaryDexClassNamesPath,
      Path androidJarPath,
      Collection<Path> classpathFiles,
      Optional<Integer> minSdkVersion,
      OptionalInt threadCount)
      throws CompilationFailedException, IOException {
    return runD8Command(
        diagnosticsHandler,
        outputDexFile,
        filesToDex,
        options,
        primaryDexClassNamesPath,
        androidJarPath,
        classpathFiles,
        minSdkVersion,
        threadCount,
        ImmutableSet.of(D8OutputOption.CLASS_DESCRIPTORS));
  }

  private static D8Output runD8Command(
      D8DiagnosticsHandler diagnosticsHandler,
      Path outputDexFile,
      Iterable<Path> filesToDex,
      Set<D8Options> options,
      Optional<Path> primaryDexClassNamesPath,
      Path androidJarPath,
      Collection<Path> classpathFiles,
      Optional<Integer> minSdkVersion,
      OptionalInt threadCount,
      Set<D8OutputOption> outputOptions)
      throws CompilationFailedException, IOException {
    Set<Path> inputs = new HashSet<>();
    for (Path toDex : filesToDex) {
      if (Files.isRegularFile(toDex)) {
        inputs.add(toDex);
      } else {
        try (Stream<Path> paths = Files.walk(toDex)) {
          paths.filter(path -> path.toFile().isFile()).forEach(inputs::add);
        }
      }
    }

    // D8 only outputs to dex if the output path is a directory. So we output to a temporary dir
    // and move it over to the final location
    boolean outputToDex = outputDexFile.getFileName().toString().endsWith(".dex");
    Path output = outputDexFile;
    if (outputToDex) {
      String scratchPath = System.getenv("BUCK_SCRATCH_PATH");
      if (scratchPath == null || scratchPath.isEmpty()) {
        throw new IllegalStateException(
            "BUCK_SCRATCH_PATH environment variable must be set and non-empty");
      }
      output = Files.createTempDirectory(Path.of(scratchPath), "buck-d8");
    }

    // Wrap the consumer that setOutput() would have installed so we can record which classes D8
    // actually wrote.
    Set<String> writtenDescriptors = Collections.synchronizedSet(new HashSet<>());
    // Picks the consumer the same way setOutput() would, using the same predicate it uses.
    DexIndexedConsumer outputConsumer =
        FileUtils.isArchive(output)
            ? new DexIndexedConsumer.ArchiveConsumer(output)
            : new DexIndexedConsumer.DirectoryConsumer(output);
    DexIndexedConsumer recordingConsumer =
        new DexIndexedConsumer.ForwardingConsumer(outputConsumer) {
          @Override
          public void accept(
              int fileIndex,
              ByteDataView data,
              Set<String> descriptors,
              DiagnosticsHandler handler) {
            if (descriptors != null) {
              writtenDescriptors.addAll(descriptors);
            }
            super.accept(fileIndex, data, descriptors, handler);
          }
        };

    // Ask D8 which class each synthetic was synthesized from. Synthetics have to be placed in the
    // same dex as their context class, and their names are mangled in a format D8 does not treat
    // as stable (see setEnableVerboseSyntheticNames), so the names cannot be parsed for this.
    // D8 invokes acceptSyntheticInfo from its worker threads, so this has to be thread safe.
    Map<String, String> syntheticToSynthesizingContext = new ConcurrentHashMap<>();
    SyntheticInfoConsumer syntheticInfoConsumer =
        new SyntheticInfoConsumer() {
          @Override
          public void acceptSyntheticInfo(SyntheticInfoConsumerData data) {
            syntheticToSynthesizingContext.put(
                data.getSyntheticClass().getBinaryName(),
                data.getSynthesizingContextClass().getBinaryName());
          }

          @Override
          public void finished() {}
        };

    D8Command.Builder builder =
        D8Command.builder(diagnosticsHandler)
            .setSyntheticInfoConsumer(syntheticInfoConsumer)
            .addProgramFiles(inputs)
            .setIntermediate(options.contains(D8Options.INTERMEDIATE))
            .addLibraryFiles(androidJarPath)
            .setMode(
                options.contains(D8Options.NO_OPTIMIZE)
                    ? CompilationMode.DEBUG
                    : CompilationMode.RELEASE)
            .setDisableDesugaring(options.contains(D8Options.NO_DESUGAR))
            .setInternalOptionsModifier(
                (InternalOptions opt) -> {
                  opt.testing.forceJumboStringProcessing = options.contains(D8Options.FORCE_JUMBO);
                  if (options.contains(D8Options.MINIMIZE_PRIMARY_DEX)) {
                    opt.minimalMainDex = true;
                  } else if (options.contains(D8Options.MAXIMIZE_PRIMARY_DEX)) {
                    opt.minimalMainDex = false;
                  }
                  if (threadCount.isPresent()) {
                    opt.threadCount = threadCount.getAsInt();
                  }
                  // Verbose synthetic names: keep backport-with-forwarding helpers in a distinct
                  // namespace so they do not collide with lambda $N ordinals across separate dex
                  // invocations (fb4a ARFU NoSuchMethodError).
                  try {
                    Object desugarSpecificOptions = opt.desugarSpecificOptions();
                    desugarSpecificOptions
                        .getClass()
                        .getField("enableVerboseSyntheticNames")
                        .setBoolean(desugarSpecificOptions, true);
                  } catch (NoSuchFieldException | IllegalAccessException ignored) {
                  }
                });

    OutputClassDescriptorConsumer outputClassDescriptorConsumer = null;
    if (options.contains(D8Options.FILE_PER_CLASS)) {
      builder.setProgramConsumer(new DexFilePerClassFileConsumer.DirectoryConsumer(output));
    } else if (outputOptions.contains(D8OutputOption.CLASS_DESCRIPTORS)) {
      outputClassDescriptorConsumer = new OutputClassDescriptorConsumer(recordingConsumer);
      builder.setProgramConsumer(outputClassDescriptorConsumer);
    } else {
      builder.setProgramConsumer(recordingConsumer);
    }

    minSdkVersion.ifPresent(builder::setMinApiLevel);
    if (minSdkVersion.orElse(0) <= 21) {
      // addMainDexListFiles is not supported for minSdkVersion > 21
      primaryDexClassNamesPath.ifPresent(builder::addMainDexListFiles);
    }

    if (classpathFiles != null) {
      // classpathFiles is needed only for D8 Java 8 desugar
      builder.addClasspathFiles(classpathFiles);
    }

    D8Command d8Command = builder.build();
    com.android.tools.r8.D8.run(d8Command);

    if (outputToDex) {
      moveSingleDexOutput(output, outputDexFile, options);
    }

    // Only null for the help/version commands produced by D8Command.parse, never for a built one.
    DexItemFactory dexItemFactory = Objects.requireNonNull(d8Command.getDexItemFactory());
    return new D8Output(
        dexItemFactory.computeReferencedResources(),
        writtenDescriptors,
        outputClassDescriptorConsumer == null
            ? ImmutableMap.of()
            : outputClassDescriptorConsumer.getOutputClassDescriptors(),
        syntheticToSynthesizingContext);
  }

  private static class OutputClassDescriptorConsumer extends DexIndexedConsumer.ForwardingConsumer {
    private final Map<Integer, ImmutableSet<String>> outputClassDescriptors =
        new ConcurrentHashMap<>();

    private OutputClassDescriptorConsumer(DexIndexedConsumer consumer) {
      super(consumer);
    }

    @Override
    public void accept(
        int fileIndex, ByteDataView data, Set<String> descriptors, DiagnosticsHandler handler) {
      Preconditions.checkState(
          outputClassDescriptors.putIfAbsent(fileIndex, ImmutableSet.copyOf(descriptors)) == null,
          "D8 reported output index %s more than once",
          fileIndex);
      super.accept(fileIndex, data, descriptors, handler);
    }

    private ImmutableMap<Integer, ImmutableSet<String>> getOutputClassDescriptors() {
      return ImmutableMap.copyOf(outputClassDescriptors);
    }
  }

  static void writeSecondaryDexJarAndMetadataFile(
      Path secondaryDexOutputJarPath,
      Path secondaryDexOutputJarMetadataPath,
      Path rawSecondaryDexPath,
      String compression)
      throws IOException {

    try (CustomZipOutputStream jarOutputStream =
            ZipOutputStreams.newOutputStream(secondaryDexOutputJarPath);
        InputStream secondaryDexInputStream =
            new BufferedInputStream(new FileInputStream(rawSecondaryDexPath.toFile()))) {

      ZipEntry customEntry = new ZipEntry("classes.dex");
      if (compression.equals("xzs")) {
        try (ByteArrayOutputStream bos = new ByteArrayOutputStream()) {
          ByteStreams.copy(secondaryDexInputStream, bos);
          byte[] bytes = bos.toByteArray();
          customEntry.setCrc(Hashing.crc32().hashBytes(bytes).padToLong());
          customEntry.setSize(bytes.length);
          customEntry.setCompressedSize(bytes.length);
          customEntry.setMethod(ZipEntry.STORED);
          jarOutputStream.putNextEntry(customEntry);
          ByteStreams.copy(new ByteArrayInputStream(bytes), jarOutputStream);
          jarOutputStream.closeEntry();
        }
      } else {
        jarOutputStream.putNextEntry(customEntry);
        ByteStreams.copy(secondaryDexInputStream, jarOutputStream);
        jarOutputStream.closeEntry();
      }
    }

    ZipScrubber.scrubZip(secondaryDexOutputJarPath);

    writeSecondaryDexMetadata(
        secondaryDexOutputJarPath, secondaryDexOutputJarMetadataPath, compression);
  }

  /**
   * Write a secondary dex jar metadata file. This is a .meta file with a single line containing:
   *
   * <p>jar:<size of secondary dex jar (in bytes)> dex:<size of uncompressed dex file (in bytes)>
   */
  static void writeSecondaryDexMetadata(
      Path secondaryDexOutputJarPath, Path secondaryDexOutputJarMetadataPath, String compression)
      throws IOException {
    try (ZipFile zf = new ZipFile(secondaryDexOutputJarPath.toFile())) {
      ZipEntry classesDexEntry = zf.getEntry("classes.dex");
      if (classesDexEntry == null) {
        throw new RuntimeException("could not find classes.dex in jar");
      }

      long uncompressedSize = classesDexEntry.getSize();
      if (uncompressedSize == -1) {
        throw new RuntimeException("classes.dex size should be known");
      }

      long jarSize = Files.size(secondaryDexOutputJarPath);
      if (compression.equals("xzs")) {
        Preconditions.checkState(
            uncompressedSize + 120 == jarSize,
            "For xzs compression, we expect the .dex to be stored uncompressed and the "
                + "overhead of the .jar itself to be 120 bytes!");
      }

      Files.write(
          secondaryDexOutputJarMetadataPath,
          Collections.singletonList(
              String.format(
                  "jar:%s dex:%s", Files.size(secondaryDexOutputJarPath), uncompressedSize)));
    }
  }

  /**
   * The secondary dex directory contains a single metadata.txt file which has one line per
   * secondary dex, consisting of:
   *
   * <p><secondary dex file name> <sha1 hash of secondary dex> <canary class>
   */
  static String getSecondaryDexMetadataString(Path secondaryDexPath, String canaryClassName)
      throws IOException {
    return String.format(
        "%s %s %s",
        secondaryDexPath.getFileName(),
        com.google.common.io.Files.hash(secondaryDexPath.toFile(), Hashing.sha1()).toString(),
        canaryClassName);
  }

  static String getRawSecondaryDexSubDir(String module) {
    if (APKModule.isRootModule(module)) {
      return "";
    } else {
      return String.format("assets/%s", module);
    }
  }

  /**
   * Moves a single DEX file from a D8 output directory to the final destination. When {@link
   * D8Options#FAIL_ON_MULTIPLE_DEX} is present in the options set, throws {@link
   * IllegalStateException} if D8 produced multiple DEX files. Otherwise, silently moves the first
   * file (sorted lexicographically).
   *
   * <p>This is package-visible for testing.
   */
  static void moveSingleDexOutput(Path d8OutputDir, Path outputDexFile, Set<D8Options> options)
      throws IOException {
    File[] outputs = d8OutputDir.toFile().listFiles();
    if (outputs == null || outputs.length == 0) {
      return;
    }
    if (outputs.length > 1 && options.contains(D8Options.FAIL_ON_MULTIPLE_DEX)) {
      StringBuilder fileList = new StringBuilder();
      for (File f : outputs) {
        fileList.append("\n  ").append(f.getName());
      }
      throw new IllegalStateException(
          "D8 produced "
              + outputs.length
              + " DEX files but only a single output was expected:"
              + fileList
              + "\nThe combined input likely exceeds DEX reference limits."
              + " Enable split_dex (use_split_dex = True) for this target.");
    }
    // File.listFiles() returns undefined order — sort for determinism.
    Arrays.sort(outputs);
    Files.move(outputs[0].toPath(), outputDexFile, StandardCopyOption.REPLACE_EXISTING);
  }

  public static class D8DiagnosticsHandler implements DiagnosticsHandler {

    public final List<Diagnostic> diagnostics = new ArrayList<>();

    @Override
    public void warning(Diagnostic warning) {
      diagnostics.add(warning);
    }

    @Override
    public void info(Diagnostic info) {}
  }
}
