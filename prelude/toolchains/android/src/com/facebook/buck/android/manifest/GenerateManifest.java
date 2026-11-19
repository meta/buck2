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

import com.android.ide.common.xml.XmlFormatPreferences;
import com.android.ide.common.xml.XmlFormatStyle;
import com.android.ide.common.xml.XmlPrettyPrinter;
import com.android.manifmerger.ManifestMerger2;
import com.android.manifmerger.MergingReport;
import com.android.manifmerger.PlaceholderHandler;
import com.android.utils.ILogger;
import com.facebook.buck.android.DefaultAndroidManifestReader;
import com.facebook.buck.android.apkmodule.APKModule;
import com.facebook.buck.util.environment.Platform;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Joiner;
import com.google.common.base.Strings;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.Iterables;
import com.google.common.io.Files;
import java.io.File;
import java.io.IOException;
import java.io.StringReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import org.jetbrains.annotations.Nullable;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;
import org.xml.sax.InputSource;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class GenerateManifest {

  public static final String ANDROID_NAMESPACE = "http://schemas.android.com/apk/res/android";

  /** Holds SDK version information extracted from a manifest file. */
  @VisibleForTesting
  static class SdkVersions {
    @Nullable private final String minSdkVersion;
    @Nullable private final String targetSdkVersion;

    SdkVersions(@Nullable String minSdkVersion, @Nullable String targetSdkVersion) {
      this.minSdkVersion = minSdkVersion;
      this.targetSdkVersion = targetSdkVersion;
    }

    @Nullable
    String getMinSdkVersion() {
      return minSdkVersion;
    }

    @Nullable
    String getTargetSdkVersion() {
      return targetSdkVersion;
    }
  }

  public static String generateXml(
      Path skeletonManifestPath,
      String moduleName,
      ImmutableList<Path> libraryManifestPaths,
      ImmutableMap<String, String> placeholders,
      Path outManifestPath,
      Path mergeReportPath,
      @Nullable Path preprocessLogPath,
      ILogger logger)
      throws IOException {
    if (skeletonManifestPath.getNameCount() == 0) {
      throw new RuntimeException("Skeleton manifest filepath is missing");
    }

    if (outManifestPath.getNameCount() == 0) {
      throw new RuntimeException("Output Manifest filepath is missing");
    }

    Files.createParentDirs(outManifestPath.toFile());

    // Create preprocess logger (always creates a log file, in /tmp if no path specified)
    PreprocessLogger preprocessLogger = PreprocessLogger.create(preprocessLogPath, logger);
    preprocessLogger.log("=== Manifest Preprocessing Log ===\n");
    preprocessLogger.log("Skeleton manifest: %s\n", skeletonManifestPath.toAbsolutePath());
    preprocessLogger.log("Module name: %s\n", moduleName);
    preprocessLogger.log("Library manifests count: %d\n\n", libraryManifestPaths.size());

    // Extract SDK versions from the skeleton manifest
    SdkVersions sdkVersions = extractSdkVersions(skeletonManifestPath.toFile(), logger);

    // Preprocess library manifests to inject targetSdkVersion and minSdkVersion if missing
    String tmpDirEnv = System.getenv("BUCK_SCRATCH_PATH");
    if (tmpDirEnv == null || tmpDirEnv.isEmpty()) {
      throw new RuntimeException(
          "BUCK_SCRATCH_PATH environment variable must be set and non-empty");
    }
    Path tmpDirPath = java.nio.file.Paths.get(tmpDirEnv);
    Path tempDir = tmpDirPath.resolve("preprocessed_manifests");
    java.nio.file.Files.createDirectories(tempDir);

    List<File> libraryManifestFiles = new ArrayList<>();
    for (Path libraryManifestPath : libraryManifestPaths) {
      File processedFile =
          preprocessLibraryManifest(
              libraryManifestPath.toFile(), tempDir, sdkVersions, preprocessLogger, logger);
      libraryManifestFiles.add(processedFile);
    }

    MergingReport mergingReport =
        mergeManifests(
            moduleName,
            skeletonManifestPath.toFile(),
            libraryManifestFiles,
            mergeReportPath,
            logger);

    // Post-process merge report to make all paths relative to current directory
    makePathsRelativeInMergeReport(mergeReportPath, logger);

    // Absent only when the merge failed, which mergeManifests already turns into an exception.
    String xmlText =
        Objects.requireNonNull(
            mergingReport.getMergedDocument(MergingReport.MergedManifestKind.MERGED));
    xmlText = replacePlaceholders(xmlText, placeholders);
    xmlText = moveActivityAliasesToEnd(xmlText);

    if (Platform.detect() == Platform.WINDOWS) {
      // Convert line endings to Lf on Windows.
      xmlText = xmlText.replace("\r\n", "\n");
    }

    // Close preprocess logger
    preprocessLogger.log("\n=== Preprocessing Complete ===\n");
    preprocessLogger.close();

    return xmlText;
  }

  public static String replaceApplicationIdPlaceholders(
      String currentManifest, boolean verifyNoPlaceholdersLeft) throws IOException {
    // Library manifests can set the applicationId placeholder in the expectation that it will
    // be replaced by the actual applicationId in the final manifest. The applicationId is
    // the "package" attribute in the final manifest, so we extract that here and then use it.
    // See https://developer.android.com/studio/build/configure-app-module#set-application-id
    final String applicationPackage =
        DefaultAndroidManifestReader.forString(currentManifest).getPackage();

    String manifest =
        replacePlaceholders(
            currentManifest,
            ImmutableMap.of(PlaceholderHandler.APPLICATION_ID, applicationPackage));

    if (verifyNoPlaceholdersLeft) {
      verifyAllPlaceholdersAreReplacedInManifestOrThrow(manifest);
    }

    return manifest;
  }

  private static MergingReport mergeManifests(
      String moduleName,
      File mainManifestFile,
      List<File> libraryManifestFiles,
      Path mergeReportPath,
      ILogger logger) {
    try {
      ManifestMerger2.Invoker manifestInvoker =
          ManifestMerger2.newMerger(
              mainManifestFile, logger, ManifestMerger2.MergeType.APPLICATION);
      if (!APKModule.isRootModule(moduleName)) {
        manifestInvoker.setPlaceHolderValue("split", moduleName);
      } else {
        manifestInvoker.withFeatures(ManifestMerger2.Invoker.Feature.NO_PLACEHOLDER_REPLACEMENT);
      }

      MergingReport mergingReport =
          manifestInvoker
              .withFeatures(
                  ManifestMerger2.Invoker.Feature.REMOVE_TOOLS_DECLARATIONS,
                  ManifestMerger2.Invoker.Feature.DISABLE_PACKAGE_NAME_UNIQUENESS_CHECK,
                  ManifestMerger2.Invoker.Feature.USES_SDK_IN_MANIFEST_LENIENT_HANDLING)
              .addLibraryManifests(Iterables.toArray(libraryManifestFiles, File.class))
              .setMergeReportFile(mergeReportPath.toFile())
              .merge();
      if (mergingReport.getResult().isError()) {
        for (MergingReport.Record record : mergingReport.getLoggingRecords()) {
          logger.error(null, record.toString());
        }
        throw new RuntimeException("Error generating manifest file");
      }

      return mergingReport;
    } catch (ManifestMerger2.MergeFailureException e) {
      throw new RuntimeException(
          String.format("Error generating manifest file: %s", String.valueOf(e.getMessage())),
          e.getCause());
    }
  }

  @VisibleForTesting
  static String replacePlaceholders(String content, ImmutableMap<String, String> placeholders) {
    Iterable<String> escaped = Iterables.transform(placeholders.keySet(), Pattern::quote);

    Joiner joiner = Joiner.on("|");
    String patternString =
        Pattern.quote("${") + "(" + joiner.join(escaped) + ")" + Pattern.quote("}");
    Pattern pattern = Pattern.compile(patternString);
    Matcher matcher = pattern.matcher(content);

    StringBuffer sb = new StringBuffer();
    while (matcher.find()) {
      final String value = Objects.requireNonNull(placeholders.get(matcher.group(1)));
      // There are instances where the value of a placeholder points to another placeholder, having
      // the form of `${key}`.
      // This clashes with the implementation details of `Matcher.appendReplacement`, which uses the
      // same form to identify what to replace. Escaping `$` prevents this conflict from happening.
      final String escapedValue = value.replace("$", "\\$");
      matcher.appendReplacement(sb, escapedValue);
    }
    matcher.appendTail(sb);
    return sb.toString();
  }

  private static void verifyAllPlaceholdersAreReplacedInManifestOrThrow(String content) {
    String contentWithoutComments = content.replaceAll("<!--[\\s\\S\\n]*?-->", "");
    Pattern pattern = Pattern.compile(Pattern.quote("${") + "(.*?)" + Pattern.quote("}"));
    Matcher matcher = pattern.matcher(contentWithoutComments);

    Set<String> nonHandledPlaceholders = new HashSet<>();
    while (matcher.find()) {
      nonHandledPlaceholders.add(matcher.group(1));
    }

    if (!nonHandledPlaceholders.isEmpty()) {
      throw new RuntimeException(
          String.format(
              "Not handled placeholders (%s) in manifest: %s", nonHandledPlaceholders, content));
    }
  }

  /**
   * At the moment all apps using manifest.py in fbsource already handle how activity-alias are
   * sorted to guarantee they are in the manifest after the activity they target thanks to the post
   * processing happening in @link{<a href="https://fburl.com/code/fsiow1yu">the manifest.py</a>}.
   *
   * <p>In WA this is not the case and we want to guarantee the order is maintained until we are
   * able to update the ManifestMerging plugin to more recent version. This is a link to a @link{<a
   * href="https://fburl.com/workplace/byshefjr">discussion</a>} regarding this specific manifest
   * merging issue.
   *
   * <p>Once the latest version of the ManifestMerger will be updated we can clean up this step and
   * the work is tracked in T239793438
   */
  @VisibleForTesting
  static String moveActivityAliasesToEnd(String xmlContent) {
    try {
      // Parse the XML
      DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
      factory.setNamespaceAware(true);
      DocumentBuilder builder = factory.newDocumentBuilder();
      Document doc =
          Objects.requireNonNull(builder.parse(new InputSource(new StringReader(xmlContent))));

      // Find the application element
      NodeList applicationNodes = doc.getElementsByTagName("application");
      if (applicationNodes.getLength() == 0) {
        return xmlContent; // No application element, nothing to do
      }

      Element application = (Element) Objects.requireNonNull(applicationNodes.item(0));
      List<Element> earlyAliases = getEarlyAliases(application);

      if (earlyAliases.isEmpty()) {
        // No early aliases found, so let's just return the original xmlContent
        return xmlContent;
      }

      // Move early aliases to the end
      for (Element alias : earlyAliases) {
        application.removeChild(alias);
        application.appendChild(alias);
      }

      // Use XmlPrettyPrinter to format the output, matching the manifest merger's formatting
      XmlFormatPreferences prefs = XmlFormatPreferences.defaults();
      prefs.removeEmptyLines = true;

      return XmlPrettyPrinter.prettyPrint(
          doc,
          prefs,
          XmlFormatStyle.get(doc),
          null, /* lineSeparator */
          false /* endWithNewline */);

    } catch (Exception e) {
      // If XML processing fails, return original content
      // This ensures the build doesn't break if there's an issue with the reordering
      return xmlContent;
    }
  }

  private static List<Element> getEarlyAliases(Element application) {
    Set<String> activityNames = new HashSet<>();
    List<Element> earlyAliases = new ArrayList<>();

    // Iterate through children to find activities and early aliases
    NodeList children = application.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      Node node = Objects.requireNonNull(children.item(i));
      if (node.getNodeType() != Node.ELEMENT_NODE) {
        continue;
      }

      Element element = (Element) node;
      String tagName = element.getTagName();

      if ("activity".equals(tagName)) {
        String name = element.getAttributeNS(ANDROID_NAMESPACE, "name");
        if (name != null && !name.isEmpty()) {
          activityNames.add(name);
        }
      } else if ("activity-alias".equals(tagName)) {
        String target = element.getAttributeNS(ANDROID_NAMESPACE, "targetActivity");
        if (target != null && !target.isEmpty() && !activityNames.contains(target)) {
          earlyAliases.add(element);
        }
      }
    }
    return earlyAliases;
  }

  /**
   * Extracts both minSdkVersion and targetSdkVersion from the main/skeleton manifest in a single
   * pass.
   *
   * @param skeletonManifest The main application manifest file
   * @param logger Logger for debugging information
   * @return SdkVersions object containing both SDK versions (may be null if not found)
   */
  @VisibleForTesting
  static SdkVersions extractSdkVersions(File skeletonManifest, ILogger logger) {
    String minSdkVersion = null;
    String targetSdkVersion = null;

    try {
      DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
      factory.setNamespaceAware(true);
      DocumentBuilder builder = factory.newDocumentBuilder();
      Document doc = Objects.requireNonNull(builder.parse(skeletonManifest));

      NodeList usesSdkNodes = doc.getElementsByTagName("uses-sdk");
      if (usesSdkNodes.getLength() > 0) {
        Element usesSdk = (Element) Objects.requireNonNull(usesSdkNodes.item(0));

        String docMinSdkVersion = usesSdk.getAttributeNS(ANDROID_NAMESPACE, "minSdkVersion");
        String docTargetSdkVersion = usesSdk.getAttributeNS(ANDROID_NAMESPACE, "targetSdkVersion");

        if (!Strings.isNullOrEmpty(docMinSdkVersion)) {
          minSdkVersion = docMinSdkVersion;
          logger.info(
              "Extracted minSdkVersion=%s from skeleton manifest %s",
              minSdkVersion, skeletonManifest.getPath());
        } else {
          logger.warning(
              "No minSdkVersion found in skeleton manifest %s", skeletonManifest.getPath());
        }

        if (!Strings.isNullOrEmpty(docTargetSdkVersion)) {
          targetSdkVersion = docTargetSdkVersion;
          logger.info(
              "Extracted targetSdkVersion=%s from skeleton manifest %s",
              targetSdkVersion, skeletonManifest.getPath());
        } else {
          logger.warning(
              "No targetSdkVersion found in skeleton manifest %s", skeletonManifest.getPath());
        }
      } else {
        logger.warning(
            "No <uses-sdk> element found in skeleton manifest %s", skeletonManifest.getPath());
      }

    } catch (Exception e) {
      logger.warning(
          "Failed to extract SDK versions from skeleton manifest %s: %s",
          skeletonManifest.getPath(), String.valueOf(e.getMessage()));
    }

    return new SdkVersions(minSdkVersion, targetSdkVersion);
  }

  /**
   * Preprocesses library manifests to inject targetSdkVersion and minSdkVersion when missing.
   *
   * <p>This prevents the manifest merger from assuming targetSdkVersion < 4 for library manifests
   * that don't declare a <uses-sdk> element, which would cause Android to add implied permissions
   * like READ_PHONE_STATE.
   *
   * <p>The targetSdkVersion and minSdkVersion are extracted from the main application manifest to
   * ensure consistency across all merged manifests.
   *
   * @param originalFile Original library manifest file
   * @param outputDir Directory to write preprocessed manifests
   * @param sdkVersions SDK versions from the main manifest
   * @param preprocessLogger Logger for logging preprocessing activities
   * @param logger Logger for debugging information
   * @return Preprocessed manifest file (or original if no processing needed)
   * @throws IOException if logging fails
   */
  @VisibleForTesting
  static File preprocessLibraryManifest(
      File originalFile,
      Path outputDir,
      SdkVersions sdkVersions,
      PreprocessLogger preprocessLogger,
      ILogger logger)
      throws IOException {
    String targetSdkVersion = sdkVersions.getTargetSdkVersion();
    String minSdkVersion = sdkVersions.getMinSdkVersion();

    // Log the file being processed
    preprocessLogger.log("Processing library manifest: %s\n", originalFile.getPath());

    if (targetSdkVersion == null && minSdkVersion == null) {
      preprocessLogger.log("  -> Skipped: No SDK versions to inject\n");
      return originalFile;
    }

    try {
      DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
      factory.setNamespaceAware(true);
      DocumentBuilder builder = factory.newDocumentBuilder();
      Document doc = Objects.requireNonNull(builder.parse(originalFile));

      Element usesSdk = getOrCreateUsesSdk(doc);

      String existingTargetSdk = usesSdk.getAttributeNS(ANDROID_NAMESPACE, "targetSdkVersion");
      String existingMinSdk = usesSdk.getAttributeNS(ANDROID_NAMESPACE, "minSdkVersion");
      boolean patched = false;

      if (Strings.isNullOrEmpty(existingTargetSdk) && targetSdkVersion != null) {
        usesSdk.setAttributeNS(ANDROID_NAMESPACE, "android:targetSdkVersion", targetSdkVersion);
        patched = true;
        preprocessLogger.log("  -> Injected targetSdkVersion=%s (was missing)\n", targetSdkVersion);
      }
      if (Strings.isNullOrEmpty(existingMinSdk) && minSdkVersion != null) {
        usesSdk.setAttributeNS(ANDROID_NAMESPACE, "android:minSdkVersion", minSdkVersion);
        patched = true;
        preprocessLogger.log("  -> Injected minSdkVersion=%s (was missing)\n", minSdkVersion);
      }

      if (patched) {
        ensureAndroidNamespace(doc);
        File processedFile = writeProcessedManifest(originalFile, outputDir, doc);
        logger.info("%s -- preprocessed --> %s", originalFile.getPath(), processedFile.getPath());
        preprocessLogger.log("  -> Written to: %s\n", processedFile.getPath());
        return processedFile;
      } else {
        preprocessLogger.log("  -> No preprocessing needed (SDK versions already present)\n");
      }
    } catch (IOException e) {
      // Rethrow IOException (from logging) to fail the build
      throw e;
    } catch (Exception e) {
      // Catch XML parsing exceptions, log them, and use original manifest
      logger.warning(
          "Failed to preprocess library manifest %s: %s. Using original.",
          originalFile.getPath(), String.valueOf(e.getMessage()));
      preprocessLogger.log(
          "  -> ERROR: Failed to preprocess: %s\n", String.valueOf(e.getMessage()));
      preprocessLogger.log("  -> Using original manifest\n");
    }
    return originalFile;
  }

  /**
   * Ensures the android namespace is declared on the manifest element.
   *
   * @param doc The manifest document
   */
  @VisibleForTesting
  static void ensureAndroidNamespace(Document doc) {
    Element manifestElement = Objects.requireNonNull(doc.getDocumentElement());
    String androidNsAttr = manifestElement.getAttribute("xmlns:android");
    if (Strings.isNullOrEmpty(androidNsAttr)) {
      manifestElement.setAttribute("xmlns:android", ANDROID_NAMESPACE);
    }
  }

  /**
   * Gets the existing uses-sdk element from the document, or creates a new one if it doesn't exist.
   *
   * @param doc The manifest document
   * @return The uses-sdk element (either existing or newly created)
   */
  @VisibleForTesting
  static Element getOrCreateUsesSdk(Document doc) {
    NodeList usesSdkNodes = doc.getElementsByTagName("uses-sdk");
    if (usesSdkNodes.getLength() > 0) {
      return (Element) Objects.requireNonNull(usesSdkNodes.item(0));
    }

    Element usesSdk = doc.createElement("uses-sdk");
    Element manifestElement = Objects.requireNonNull(doc.getDocumentElement());

    Node firstChild = manifestElement.getFirstChild();
    if (firstChild != null) {
      manifestElement.insertBefore(usesSdk, firstChild);
    } else {
      manifestElement.appendChild(usesSdk);
    }

    return usesSdk;
  }

  private static File writeProcessedManifest(File originalFile, Path outputDir, Document doc)
      throws IOException {
    // Write the modified manifest to a temporary file
    File processedFile = outputDir.resolve(originalFile.getPath()).toFile();
    Objects.requireNonNull(processedFile.getParentFile()).mkdirs();

    XmlFormatPreferences prefs = XmlFormatPreferences.defaults();
    prefs.removeEmptyLines = true;
    String formattedXml =
        XmlPrettyPrinter.prettyPrint(
            doc,
            prefs,
            XmlFormatStyle.get(doc),
            null, /* lineSeparator */
            false /* endWithNewline */);

    java.nio.file.Files.write(processedFile.toPath(), formattedXml.getBytes());
    return processedFile;
  }

  /**
   * Post-processes the merge report file to convert all absolute paths to relative paths.
   *
   * @param mergeReportPath Path to the merge report file
   * @param logger Logger for debugging information
   */
  private static void makePathsRelativeInMergeReport(Path mergeReportPath, ILogger logger) {
    try {
      if (!java.nio.file.Files.exists(mergeReportPath)) {
        logger.warning("Merge report file does not exist: " + mergeReportPath);
        return;
      }

      String content =
          new String(java.nio.file.Files.readAllBytes(mergeReportPath), StandardCharsets.UTF_8);
      Path currentDir = java.nio.file.Paths.get("").toAbsolutePath();
      String modifiedContent = makePathsRelative(content, currentDir);
      java.nio.file.Files.write(mergeReportPath, modifiedContent.getBytes(StandardCharsets.UTF_8));
      logger.info("Made paths relative in merge report: " + mergeReportPath);
    } catch (IOException e) {
      logger.warning(
          "Failed to make paths relative in merge report: "
              + String.valueOf(e.getMessage())
              + ". Report will contain absolute paths.");
    }
  }

  /**
   * Replaces absolute paths in the content with relative paths from the current directory.
   *
   * <p>Only replaces paths that start with the current directory to avoid false matches with
   * substring occurrences.
   *
   * @param content The content to process
   * @param currentDir The current working directory
   * @return Content with paths made relative
   */
  private static String makePathsRelative(String content, Path currentDir) {
    String currentDirStr = currentDir.toString();
    String result = content;

    // Build patterns with path separators to match only actual path prefixes
    // Handle both forward slashes and backslashes for cross-platform compatibility
    String withForwardSlash = currentDirStr + "/";
    String withBackslash = currentDirStr + "\\";

    // Replace paths that start with currentDir followed by a separator
    result = result.replace(withForwardSlash, "");
    result = result.replace(withBackslash, "");

    return result;
  }
}
