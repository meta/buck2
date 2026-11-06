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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.file.MostFiles;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.util.MoreSuppliers;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedMap;
import com.google.common.collect.Maps;
import com.google.common.collect.Ordering;
import com.google.common.io.ByteStreams;
import java.io.ByteArrayInputStream;
import java.io.Closeable;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.SortedMap;
import java.util.function.Function;
import java.util.function.Supplier;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import javax.annotation.Nullable;

/**
 * ExoResourceRewriter is the core of constructing build outputs for exo-for-resources.
 *
 * <p>Some background: Android's resources are packaged into the APK primarily in the resources.arsc
 * file with some references out to other files in the APK's res/ directory (.png/.xml mostly). The
 * resources.arsc file is a binary file in a format that isn't really well documented, but you can
 * get a good idea of its structure by looking at ResourceTable and other classes in this package.
 * At runtime, Android constructs an AssetManager from the APK and resource lookups go through that.
 * While this is primarily done for an app's own resources, the framework may construct one to
 * access an app's resources directly (e.g. for driving animations, for intent pickers, etc) and
 * apps can access resources of other apps (e.g. a launcher app will access names/icons).
 *
 * <p>For exo-for-resources, we determine a minimal set of resources (including referenced files)
 * that need to be in the main APK. This set includes all resources referenced from the
 * AndroidManifest.xml or from any animation. We construct a resources.arsc for these resources and
 * then the primary apk includes those resources. To avoid odd issues, we rewrite the full
 * resources.arsc and all compiled .xml files such that references match between the primary apk and
 * the exo resources (and without this, we have run into issues).
 *
 * <p>For assets, we don't package any into the main apk. For exo resources, assets are put into a
 * separate zip from the exo .arsc (and resource files).
 *
 * <p>TODO(cjhopman): The underlying c++ resource handling supports some things that we should take
 * advantage of. First, we are able to easily have multiple resource apks (including multiple .arsc
 * with the same package id) as long as we don't have the same package-id/type-id pair in different
 * .arsc files. Second, there's no restriction that all resources of the same type actually have the
 * same type id (e.g. we could have strings with type id 0x01, 0x02, and 0x03). I'm pretty sure that
 * aapt's --feature-of/--feature-after work by using different type ids for the same type.
 *
 * <p>Using these, we should be able to split resources across some larger number of .zips in such a
 * way that users will typically only need to install a small number of resources (for example, in a
 * particular large app that I've looked at, a vast majority of resource size is spent on just the
 * 'strings' type). It's probably also possible for us to construct multiple top-level aapt targets
 * that handle smaller subsets of the resources (e.g. construct a separate 'strings' top-level aapt
 * rule). While it would be hard to construct multiple aapt rules for a particular type (adding
 * restrictions on resource overriding would make it easier) we could still easily split a
 * particular type into multiple exo resources zips (using different type ids).
 *
 * <p>It might be possible to get even more ids to work by using a different package id (other than
 * 0x7f). I believe the package id space is partitioned like so:
 *
 * <ul>
 *   <li>0x01- framework
 *   <li>0x02 - 0x7f - potentially any of these are used by OEM overlays. OEM overlays, just like
 *       the framework, will be loaded into the zygote.
 *   <li>0x7f - the app
 *   <li>0x80 and above - (KK+) dynamically loaded resource libraries (like GMS and WebView
 *       resources), some of these get loaded after your process starts by the framework code that
 *       processes your apps AndroidManifest, the WebView resources get loaded at runtime, the first
 *       time you new up a WebView instance.
 * </ul>
 *
 * <p>As the normal Android build system doesn't use anything other than 0x7f, and that's always
 * been the only package id used by applications, and that we have some leeway in the type id space,
 * I didn't think it was worth it now to further investigate the feasibility/difficulty of using
 * different package ids.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ExoResourcesRewriter {
  private ExoResourcesRewriter() {}

  public static void rewrite(
      AbsPath ruleCellRoot,
      RelPath inputPath,
      RelPath inputRDotTxt,
      RelPath primaryResources,
      Path exoResources,
      RelPath outputRDotTxt)
      throws IOException {
    ReferenceMapper resMapping =
        rewriteResources(ruleCellRoot, inputPath, primaryResources, exoResources);
    rewriteRDotTxt(ruleCellRoot, resMapping, inputRDotTxt, outputRDotTxt);
  }

  static ReferenceMapper rewriteResources(
      AbsPath ruleCellRoot, RelPath inputPath, RelPath primaryResources, Path exoResources)
      throws IOException {
    try (ApkZip apkZip =
        new ApkZip(ProjectFilesystemUtils.getAbsPathForRelativePath(ruleCellRoot, inputPath))) {
      UsedResourcesFinder.ResourceClosure closure =
          UsedResourcesFinder.computePrimaryApkClosure(apkZip);
      ReferenceMapper resMapping =
          BringToFrontMapper.construct(ResTablePackage.APP_PACKAGE_ID, closure.idsByType);
      // Rewrite the arsc.
      apkZip.getResourceTable().reassignIds(resMapping);
      // Update the references in xml files.
      for (ResourcesXml xml : apkZip.getResourcesXmls()) {
        xml.transformReferences(resMapping::map);
      }
      // Write the full (rearranged) resources to the exo resources.
      try (ResourcesZipBuilder zipBuilder = new ResourcesZipBuilder(exoResources)) {
        for (ZipEntry entry : apkZip.getEntries()) {
          addEntryWithRawPassThrough(zipBuilder, apkZip, entry, COMPRESSION_LEVEL);
        }
      }
      // Then, slice out the resources needed for the primary apk.
      try (ResourcesZipBuilder zipBuilder =
          new ResourcesZipBuilder(
              ProjectFilesystemUtils.getPathForRelativePath(ruleCellRoot, primaryResources))) {
        ResourceTable primaryResourceTable =
            ResourceTable.slice(
                apkZip.getResourceTable(),
                ImmutableMap.copyOf(Maps.transformValues(closure.idsByType, Set::size)));
        addEntry(
            zipBuilder,
            "resources.arsc",
            primaryResourceTable.serialize(),
            apkZip.getEntry("resources.arsc").getMethod() == ZipEntry.STORED
                ? 0
                : COMPRESSION_LEVEL,
            false);
        for (String path : closure.files.stream().sorted().collect(Collectors.toList())) {
          ZipEntry entry = apkZip.getEntry(path);
          addEntryWithRawPassThrough(zipBuilder, apkZip, entry, COMPRESSION_LEVEL);
        }
      }
      return resMapping;
    }
  }

  static void rewriteRDotTxt(
      AbsPath ruleCellRoot,
      ReferenceMapper refMapping,
      RelPath inputRDotTxt,
      RelPath outputRDotTxt) {
    Map<String, String> cache = new HashMap<>();
    Function<String, String> mapping =
        (s) ->
            cache.computeIfAbsent(
                s, (k) -> String.format("0x%x", refMapping.map(Integer.parseInt(k, 16))));
    try {
      List<String> lines =
          Files.readAllLines(
              ProjectFilesystemUtils.getPathForRelativePath(ruleCellRoot, inputRDotTxt),
              StandardCharsets.UTF_8);
      List<String> mappedLines = new ArrayList<>(lines.size());
      Pattern regular = Pattern.compile("int ([^ ]*) ([^ ]*) 0x(7f[0-9a-f]{6})");
      Pattern styleable = Pattern.compile("int\\[] (styleable) ([^ ]*) \\{(.*) }");
      Pattern index = Pattern.compile("int (styleable) ([^ ]*) ([0-9]*)");
      Pattern number = Pattern.compile("0x([0-9a-f]{8})");

      Iterator<String> iter = lines.iterator();
      while (iter.hasNext()) {
        String line = iter.next();
        Matcher reg = regular.matcher(line);
        if (reg.matches()) {
          String newId = mapping.apply(Objects.requireNonNull(reg.group(3)));
          mappedLines.add(String.format("int %s %s %s", reg.group(1), reg.group(2), newId));
          continue;
        }
        Matcher stMatcher = styleable.matcher(line);
        if (!stMatcher.matches()) {
          throw new RuntimeException("Unmatched: " + line);
        }
        String values = stMatcher.group(3);
        ArrayList<String> ids = new ArrayList<>();
        Matcher m = number.matcher(Objects.requireNonNull(values));
        while (m.find()) {
          String id = mapping.apply(Objects.requireNonNull(m.group(1)));
          ids.add(id);
        }
        Map<String, Integer> newIndex = new HashMap<>();
        List<String> sortedIds = ids.stream().sorted().collect(Collectors.toList());
        for (int i = 0; i < sortedIds.size(); i++) {
          newIndex.put(sortedIds.get(i), i);
        }
        StringBuilder valuesBuilder = new StringBuilder();
        String prefix = "";
        for (String id : sortedIds) {
          valuesBuilder.append(prefix);
          valuesBuilder.append(id);
          prefix = ", ";
        }
        mappedLines.add(
            String.format(
                "int[] styleable %s { %s }", stMatcher.group(2), valuesBuilder.toString()));
        for (int i = 0; i < ids.size(); i++) {
          line = iter.next();
          m = index.matcher(line);
          if (!m.matches()) {
            throw new RuntimeException("Unmatched: " + line);
          }
          int idx = Integer.parseInt(Objects.requireNonNull(m.group(3)));
          int newIdx = Objects.requireNonNull(newIndex.get(ids.get(idx)));
          mappedLines.add(String.format("int styleable %s %d", m.group(2), newIdx));
        }
      }
      MostFiles.writeLinesToFile(
          mappedLines, ProjectFilesystemUtils.getPathForRelativePath(ruleCellRoot, outputRDotTxt));
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  // DEFAULT_COMPRESSION (-1) is not accepted by CustomZipEntry.setCompressionLevel(), so use the
  // JDK's actual default level.
  private static final int COMPRESSION_LEVEL = 6;

  private static void addEntry(
      ResourcesZipBuilder zipBuilder,
      String name,
      byte[] content,
      int compressionLevel,
      boolean isDirectory)
      throws IOException {
    // TODO(cjhopman): for files that we don't already have in memory, we should use the builder's
    // stream api.
    CRC32 crc32 = new CRC32();
    crc32.update(content);
    zipBuilder.addEntry(
        new ByteArrayInputStream(content),
        content.length,
        crc32.getValue(),
        name,
        compressionLevel,
        isDirectory);
  }

  private static void addEntryWithRawPassThrough(
      ResourcesZipBuilder zipBuilder, ApkZip apkZip, ZipEntry entry, int compressionLevel)
      throws IOException {
    if (!apkZip.isModifiedEntry(entry.getName())
        && entry.getMethod() == ZipEntry.DEFLATED
        && entry.getCompressedSize() >= 0) {
      byte[] rawBytes = apkZip.readRawCompressedBytes(entry);
      zipBuilder.addRawEntry(
          new ByteArrayInputStream(rawBytes),
          rawBytes.length,
          entry.getSize(),
          entry.getCrc(),
          entry.getMethod(),
          entry.getName());
    } else {
      addEntry(
          zipBuilder,
          entry.getName(),
          apkZip.getContent(entry.getName()),
          entry.getMethod() == ZipEntry.STORED ? 0 : compressionLevel,
          false);
    }
  }

  private static class ApkZip implements Closeable, UsedResourcesFinder.ApkContentProvider {
    private final ZipFile zipFile;
    private final AbsPath inputPath;
    private final SortedMap<String, ZipEntry> entries;
    private final Map<String, ResourcesXml> xmlEntries;
    private final Supplier<ResourceTable> resourceTable;
    private @Nullable RandomAccessFile randomAccessFile;

    public ApkZip(AbsPath inputPath) throws IOException {
      this.inputPath = inputPath;
      this.zipFile = new ZipFile(inputPath.toFile());
      this.entries =
          Collections.list(zipFile.entries()).stream()
              .collect(
                  ImmutableSortedMap.toImmutableSortedMap(
                      Ordering.natural(), ZipEntry::getName, e -> e));
      this.xmlEntries = new HashMap<>();
      byte[] arscBytes = readEntryBytes("resources.arsc");
      this.resourceTable = MoreSuppliers.memoize(() -> ResourceTable.get(ResChunk.wrap(arscBytes)));
    }

    @Override
    public ResourceTable getResourceTable() {
      return resourceTable.get();
    }

    @Override
    public ResourcesXml getXml(String path) {
      return xmlEntries.computeIfAbsent(path, this::extractXml);
    }

    @Override
    public boolean hasFile(String path) {
      return entries.containsKey(path);
    }

    @Override
    public void close() throws IOException {
      zipFile.close();
      if (randomAccessFile != null) {
        randomAccessFile.close();
      }
    }

    public Iterable<ZipEntry> getEntries() {
      return entries.values();
    }

    public ZipEntry getEntry(String path) {
      return Objects.requireNonNull(entries.get(path));
    }

    Iterable<ResourcesXml> getResourcesXmls() {
      return entries.keySet().stream()
          .filter(
              name ->
                  name.equals("AndroidManifest.xml")
                      || ((name.startsWith("res")
                          && !name.startsWith("res/raw")
                          && name.endsWith(".xml"))))
          .map(this::getXml)
          .collect(ImmutableList.toImmutableList());
    }

    byte[] getContent(String path) {
      if (path.equals("resources.arsc")) {
        return resourceTable.get().serialize();
      }
      ResourcesXml xml = xmlEntries.get(path);
      if (xml != null) {
        return xml.serialize();
      }
      return readEntryBytes(path);
    }

    private byte[] readEntryBytes(String path) {
      try {
        return ByteStreams.toByteArray(
            Objects.requireNonNull(
                zipFile.getInputStream(Objects.requireNonNull(entries.get(path)))));
      } catch (IOException e) {
        throw new RuntimeException(e);
      }
    }

    private ResourcesXml extractXml(String path) {
      try {
        return ResourcesXml.get(ResChunk.wrap(readEntryBytes(path)));
      } catch (Exception e) {
        throw new RuntimeException("When extracting " + path, e);
      }
    }

    /** Returns true if this entry was modified by the rewrite and needs recompression. */
    boolean isModifiedEntry(String name) {
      return name.equals("resources.arsc") || xmlEntries.containsKey(name);
    }

    /**
     * Reads the raw compressed bytes for a zip entry directly from the file, bypassing
     * decompression. This is used for pass-through entries that don't need modification.
     */
    byte[] readRawCompressedBytes(ZipEntry entry) throws IOException {
      long compressedSize = entry.getCompressedSize();
      if (compressedSize < 0 || entry.getMethod() == ZipEntry.STORED) {
        // STORED entries don't benefit from raw copy; just read normally.
        return readEntryBytes(entry.getName());
      }

      Map<String, Long> offsets = getLocalHeaderOffsets();
      Long localHeaderOffset = offsets.get(entry.getName());
      if (localHeaderOffset == null) {
        throw new IOException(
            "No local header offset found for entry: "
                + entry.getName()
                + ". The zip central directory may be corrupt or use ZIP64 extensions.");
      }

      RandomAccessFile raf = getRandomAccessFile();
      // Local file header: 30 bytes fixed + nameLen + extraLen
      raf.seek(localHeaderOffset + 26); // offset to name length field
      int nameLen = readShortLE(raf);
      int extraLen = readShortLE(raf);
      long dataOffset = localHeaderOffset + 30 + nameLen + extraLen;

      byte[] rawBytes = new byte[(int) compressedSize];
      raf.seek(dataOffset);
      raf.readFully(rawBytes);
      return rawBytes;
    }

    private RandomAccessFile getRandomAccessFile() throws IOException {
      if (randomAccessFile == null) {
        randomAccessFile = new RandomAccessFile(inputPath.toFile(), "r");
      }
      return randomAccessFile;
    }

    @Nullable private Map<String, Long> localHeaderOffsets;

    /**
     * Lazily parses the central directory to build a map of entry name → local header offset.
     *
     * <p>NOTE: This parser does not handle ZIP64 extensions. If the APK has more than 65,535
     * entries or is larger than 4 GB, the EOCD64 locator would be at a different position and this
     * parser will fail to find entries. Android APKs are currently well under these limits.
     */
    private Map<String, Long> getLocalHeaderOffsets() throws IOException {
      if (localHeaderOffsets != null) {
        return localHeaderOffsets;
      }
      localHeaderOffsets = new HashMap<>();
      RandomAccessFile raf = getRandomAccessFile();
      // Find End of Central Directory Record (EOCD)
      long fileLen = raf.length();
      long searchStart = Math.max(0, fileLen - 65557);
      byte[] searchBuf = new byte[(int) (fileLen - searchStart)];
      raf.seek(searchStart);
      raf.readFully(searchBuf);

      int eocdOffset = -1;
      for (int i = searchBuf.length - 22; i >= 0; i--) {
        if (searchBuf[i] == 0x50
            && searchBuf[i + 1] == 0x4b
            && searchBuf[i + 2] == 0x05
            && searchBuf[i + 3] == 0x06) {
          eocdOffset = i;
          break;
        }
      }
      if (eocdOffset < 0) {
        return localHeaderOffsets;
      }

      long cdOffset =
          (searchBuf[eocdOffset + 16] & 0xFFL)
              | ((searchBuf[eocdOffset + 17] & 0xFFL) << 8)
              | ((searchBuf[eocdOffset + 18] & 0xFFL) << 16)
              | ((searchBuf[eocdOffset + 19] & 0xFFL) << 24);

      // Parse central directory
      raf.seek(cdOffset);
      byte[] header = new byte[46];
      while (true) {
        if (raf.read(header, 0, 4) < 4) break;
        if (header[0] != 0x50 || header[1] != 0x4b || header[2] != 0x01 || header[3] != 0x02) {
          break;
        }
        raf.readFully(header, 4, 42);
        int nameLen = (header[28] & 0xFF) | ((header[29] & 0xFF) << 8);
        int extraLen = (header[30] & 0xFF) | ((header[31] & 0xFF) << 8);
        int commentLen = (header[32] & 0xFF) | ((header[33] & 0xFF) << 8);
        long localOffset =
            (header[42] & 0xFFL)
                | ((header[43] & 0xFFL) << 8)
                | ((header[44] & 0xFFL) << 16)
                | ((header[45] & 0xFFL) << 24);

        byte[] nameBytes = new byte[nameLen];
        raf.readFully(nameBytes);
        localHeaderOffsets.put(new String(nameBytes, StandardCharsets.UTF_8), localOffset);
        raf.skipBytes(extraLen + commentLen);
      }
      return localHeaderOffsets;
    }

    private static int readShortLE(RandomAccessFile raf) throws IOException {
      int b0 = raf.read();
      int b1 = raf.read();
      return (b0 & 0xFF) | ((b1 & 0xFF) << 8);
    }
  }
}
