/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aapt;

import com.facebook.buck.android.aapt.RDotTxtEntry.IdType;
import com.facebook.buck.android.aapt.RDotTxtEntry.RType;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.file.MorePaths;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.util.xml.XmlDomParser;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Joiner;
import com.google.common.base.Preconditions;
import com.google.common.base.Strings;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.io.BufferedInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.stream.Collectors;
import javax.xml.xpath.XPathConstants;
import javax.xml.xpath.XPathExpression;
import javax.xml.xpath.XPathExpressionException;
import javax.xml.xpath.XPathFactory;
import org.jetbrains.annotations.Nullable;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;
import org.xml.sax.SAXException;

/**
 * Step which parses resources in an android {@code res} directory and compiles them into a {@code
 * R.txt} file, following the exact same format as the Android build tool {@code aapt}.
 *
 * <p>
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class MiniAapt {

  private static final String GRAYSCALE_SUFFIX = "_g.png";

  // Ignore .orig files that Mercurial creates
  public static final ImmutableList<String> IGNORED_FILE_EXTENSIONS =
      ImmutableList.of("orig", "scc");

  private static final String ID_DEFINITION_PREFIX = "@+id/";
  private static final String ITEM_TAG = "item";
  private static final String OVERLAYABLE_TAG = "overlayable";
  private static final String PUBLIC_TAG = "public";
  private static final String PUBLIC_FILENAME = "public.xml";

  private static final XPathExpression ANDROID_ID_AND_ATTR_USAGE =
      createExpression(
          "//@*[(starts-with(., '@') and "
              + "not(starts-with(., '@+')) and "
              + "not(starts-with(., '@android:')) and "
              + "not(starts-with(., '@null'))) or "
              + "starts-with(., '?attr')]");

  private static final XPathExpression ANDROID_ATTR_USAGE_FOR_STYLES =
      createExpression(
          "//item/@name[not(starts-with(., 'android'))] | //item[@name]/text()[starts-with(.,"
              + " '?attr')]");

  private static final XPathExpression ANDROID_ID_DEFINITION =
      createExpression("//@*[starts-with(., '@+') and " + "not(starts-with(., '@+android:id'))]");

  private static final ImmutableMap<String, RType> RESOURCE_TYPES = getResourceTypes();

  /**
   * {@code <overlayable>} is for configuring the policies around what resources are overlayable.
   * Since these policies are not included into R.txt we can just skip handling them.
   *
   * <p>{@code <public>} is a special type of resource that is not be handled by aapt, but can be
   * analyzed by Android Lint.
   *
   * @see <a href="https://source.android.com/docs/core/runtime/rros">RRO Documentation</a>
   * @see <a
   *     href="https://developer.android.com/studio/projects/android-library#PrivateResources">Private
   *     resources</a>
   */
  private static final ImmutableSet<String> IGNORED_TAGS =
      ImmutableSet.of("eat-comment", "skip", OVERLAYABLE_TAG, PUBLIC_TAG);

  // aapt, unless specified a pattern, ignores certain files and directories. We follow the same
  // logic as the default pattern found at http://goo.gl/OTTK88 and line 61.
  public static boolean isSilentlyIgnored(Path path) {
    String fileName = Objects.requireNonNull(path.getFileName()).toString();
    return ".gitkeep".equalsIgnoreCase(fileName)
        || ".svn".equalsIgnoreCase(fileName)
        || ".git".equalsIgnoreCase(fileName)
        || ".ds_store".equalsIgnoreCase(fileName)
        || "cvs".equalsIgnoreCase(fileName)
        || "thumbs.db".equalsIgnoreCase(fileName)
        || "picasa.ini".equalsIgnoreCase(fileName)
        || fileName.endsWith("~");
  }

  private final ImmutableSet<Path> pathsToSymbolsOfDeps;
  private final RDotTxtResourceCollector resourceCollector;

  public MiniAapt(ImmutableSet<Path> pathsToSymbolsOfDeps) {
    this.pathsToSymbolsOfDeps = pathsToSymbolsOfDeps;
    this.resourceCollector = new RDotTxtResourceCollector();
  }

  private static XPathExpression createExpression(String expressionStr) {
    try {
      return XPathFactory.newInstance().newXPath().compile(expressionStr);
    } catch (XPathExpressionException e) {
      throw new RuntimeException(e);
    }
  }

  private static ImmutableMap<String, RType> getResourceTypes() {
    ImmutableMap.Builder<String, RType> types = ImmutableMap.builder();
    for (RType rType : RType.values()) {
      types.put(rType.toString(), rType);
    }
    types.put("string-array", RType.ARRAY);
    types.put("integer-array", RType.ARRAY);
    types.put("declare-styleable", RType.STYLEABLE);
    return types.build();
  }

  public RDotTxtResourceCollector getResourceCollector() {
    return resourceCollector;
  }

  /** Recursively gets all resources files under a given directory */
  public static ImmutableMap<Path, Path> getAllResourceFiles(
      AbsPath root, RelPath resourceDirectory) throws IOException {
    return ProjectFilesystemUtils.getFilesUnderPath(
            root,
            resourceDirectory.getPath(),
            ProjectFilesystemUtils.getDefaultVisitOptions(),
            ProjectFilesystemUtils.getEmptyIgnoreFilter())
        .stream()
        .collect(
            ImmutableMap.toImmutableMap(
                path -> MorePaths.relativize(resourceDirectory.getPath(), path),
                path -> ProjectFilesystemUtils.getPathForRelativePath(root, path)));
  }

  public ImmutableSet<RDotTxtEntry> processAllFiles(ImmutableMap<Path, Path> files)
      throws IOException, ResourceParseException, XPathExpressionException {
    return processAllFiles(files, false);
  }

  ImmutableSet<RDotTxtEntry> processAllFilesIncludingExcludedFromBuckResourceMap(
      ImmutableMap<Path, Path> files)
      throws IOException, ResourceParseException, XPathExpressionException {
    return processAllFiles(files, true);
  }

  private ImmutableSet<RDotTxtEntry> processAllFiles(
      ImmutableMap<Path, Path> files, boolean includeExcludedFromBuckResourceMap)
      throws IOException, ResourceParseException, XPathExpressionException {
    ImmutableSet.Builder<RDotTxtEntry> references = ImmutableSet.builder();
    for (Map.Entry<Path, Path> entry : files.entrySet()) {
      Path relativePath = entry.getKey();
      Path fullPath = entry.getValue();
      if (shouldIgnoreFile(fullPath)) {
        continue;
      }

      String dirName = relativePath.getName(0).toString();
      if (dirName.equals(relativePath.toString())) {
        // File in root directory, ignore
        continue;
      }
      if (dirName.startsWith("values")) {
        if (!isAValuesDir(dirName)) {
          throw new ResourceParseException("'%s' is not a valid values directory.", dirName);
        }
        references.addAll(processValuesFile(fullPath, includeExcludedFromBuckResourceMap));
      } else {
        processNonValuesFile(fullPath, dirName);
        if (fullPath.endsWith("styles.xml")) {
          processStyleFile(fullPath, references);
        } else if (fullPath.toString().endsWith(".xml")) {
          processXmlFile(fullPath, references);
        }
      }
    }

    return references.build();
  }

  /**
   * Collects file names under the {@code res} directory, except those under directories starting
   * with {@code values}, as resources based on their parent directory.
   *
   * <p>So for instance, if the directory structure is something like:
   *
   * <pre>
   *   res/
   *       values/ ...
   *       values-es/ ...
   *       drawable/
   *                image.png
   *                nine_patch.9.png
   *       layout/
   *              my_view.xml
   *              another_view.xml
   * </pre>
   *
   * the resulting resources would contain:
   *
   * <ul>
   *   <li>R.drawable.image
   *   <li>R.drawable.nine_patch
   *   <li>R.layout.my_view
   *   <li>R.layout.another_view
   * </ul>
   */
  void processNonValuesFile(Path fullPath, String dirName)
      throws IOException, ResourceParseException {
    String filename = Objects.requireNonNull(fullPath.getFileName()).toString();

    int dotIndex = filename.indexOf('.');
    String resourceName = dotIndex != -1 ? filename.substring(0, dotIndex) : filename;

    int dashIndex = dirName.indexOf('-');
    String standardizedDirName = dashIndex == -1 ? dirName : dirName.substring(0, dashIndex);
    if (!RESOURCE_TYPES.containsKey(standardizedDirName)) {
      throw new ResourceParseException(
          "'%s' is not a valid resource sub-directory.", standardizedDirName);
    }

    RType rType = Objects.requireNonNull(RESOURCE_TYPES.get(standardizedDirName));
    if (rType == RType.DRAWABLE) {
      processDrawables(fullPath);
    } else {
      resourceCollector.addIntResourceIfNotPresent(rType, resourceName);
    }
  }

  void processDrawables(Path resourceFile) throws IOException, ResourceParseException {
    String filename = Objects.requireNonNull(resourceFile.getFileName()).toString();
    int dotIndex = filename.indexOf('.');
    String resourceName = dotIndex != -1 ? filename.substring(0, dotIndex) : filename;

    // .g.png is no longer an allowed filename in newer versions of aapt2, hence the two spellings.
    if (!filename.endsWith(".xml")
        && (filename.endsWith(".g.png") || filename.endsWith(GRAYSCALE_SUFFIX))) {
      // Trim _g or .g from the resource name
      resourceName = filename.substring(0, filename.length() - GRAYSCALE_SUFFIX.length());
    }

    resourceCollector.addIntResourceIfNotPresent(RType.DRAWABLE, resourceName);
  }

  /**
   * Processes an {@code xml} file immediately under a {@code values} directory. See <a
   * href="http://developer.android.com/guide/topics/resources/more-resources.html>More Resource
   * Types</a> to find out more about how resources are defined.
   *
   * <p>For an input file with contents like:
   *
   * <pre>
   *   <?xml version="1.0" encoding="utf-8"?>
   *   <resources>
   *     <integer name="number">42</integer>
   *     <dimen name="dimension">10px</dimen>
   *     <string name="hello">World</string>
   *     <item name="my_fraction" type="fraction">1.5</item>
   *   </resources>
   * </pre>
   *
   * the resulting resources would be:
   *
   * <ul>
   *   <li>R.integer.number
   *   <li>R.dimen.dimension
   *   <li>R.string.hello
   *   <li>R.fraction.my_fraction
   * </ul>
   */
  @VisibleForTesting
  ImmutableSet<RDotTxtEntry> processValuesFile(Path valuesFile)
      throws IOException, ResourceParseException {
    return processValuesFile(valuesFile, false);
  }

  private ImmutableSet<RDotTxtEntry> processValuesFile(
      Path valuesFile, boolean includeExcludedFromBuckResourceMap)
      throws IOException, ResourceParseException {
    ImmutableSet.Builder<RDotTxtEntry> references = ImmutableSet.builder();
    try (InputStream stream = new BufferedInputStream(Files.newInputStream(valuesFile))) {
      Document dom = parseXml(valuesFile, stream);
      Element root = Objects.requireNonNull(dom.getDocumentElement());

      // Exclude resources annotated with the attribute {@code exclude-from-resource-map}.
      // This is useful to exclude using generated strings to build the
      // resource map, which ensures a build break will show up at build time
      // rather than being hidden until generated resources are updated.
      if (!includeExcludedFromBuckResourceMap
          && root.getAttribute("exclude-from-buck-resource-map").equals("true")) {
        return ImmutableSet.of();
      }

      for (Node node = root.getFirstChild(); node != null; node = node.getNextSibling()) {
        if (node.getNodeType() != Node.ELEMENT_NODE) {
          continue;
        }

        String resourceType = node.getNodeName();
        if (resourceType.equals(ITEM_TAG)) {
          Node typeNode = verifyNodeHasTypeAttribute(valuesFile, node);
          resourceType = Objects.requireNonNull(typeNode.getNodeValue());
        } else if (resourceType.equals(PUBLIC_TAG)) {
          Node nameAttribute = Objects.requireNonNull(node.getAttributes()).getNamedItem("name");
          if (nameAttribute == null
              || Objects.requireNonNull(nameAttribute.getNodeValue()).isEmpty()) {
            throw new ResourceParseException(
                "Error parsing file '%s', expected a 'name' attribute in \n'%s'\n",
                Objects.requireNonNull(valuesFile.getFileName()), node.toString());
          }
          String type =
              Objects.requireNonNull(verifyNodeHasTypeAttribute(valuesFile, node).getNodeValue());

          if (!RESOURCE_TYPES.containsKey(type)) {
            throw new ResourceParseException(
                "Invalid resource type '%s' in <public> resource '%s' in file '%s'.",
                type,
                nameAttribute.getNodeValue(),
                Objects.requireNonNull(valuesFile.getFileName()));
          }

          if (!PUBLIC_FILENAME.equals(
              Objects.requireNonNull(valuesFile.getFileName()).toString())) {
            throw new ResourceParseException(
                "<public> resource '%s' must be declared in res/values/public.xml, but was declared"
                    + " in '%s'",
                nameAttribute.getNodeValue(), Objects.requireNonNull(valuesFile.getFileName()));
          }
        }

        if (IGNORED_TAGS.contains(resourceType)) {
          continue;
        }

        if (!RESOURCE_TYPES.containsKey(resourceType)) {
          throw new ResourceParseException(
              "Invalid resource type '<%s>' in '%s'.", resourceType, valuesFile);
        }

        RType rType = Objects.requireNonNull(RESOURCE_TYPES.get(resourceType));
        addToResourceCollector(node, rType);
        if (rType == RType.STYLE) {
          processStyleItemReferences(node, references);
        }
      }
    }
    return references.build();
  }

  private void processStyleItemReferences(
      Node style, ImmutableSet.Builder<RDotTxtEntry> references) {
    for (Node item = style.getFirstChild(); item != null; item = item.getNextSibling()) {
      if (item.getNodeType() != Node.ELEMENT_NODE
          || !ITEM_TAG.equals(item.getNodeName())
          || hasStyleSpans(item)) {
        continue;
      }
      addStyleItemReference(item.getTextContent().trim(), references);
    }
  }

  private static boolean hasStyleSpans(Node node) {
    for (Node child = node.getFirstChild(); child != null; child = child.getNextSibling()) {
      if (child.getNodeType() == Node.ELEMENT_NODE
          && (!isXliffElement(child) || hasStyleSpans(child))) {
        return true;
      }
    }
    return false;
  }

  private static boolean isXliffElement(Node node) {
    String name = node.getNodeName();
    int colon = name.indexOf(':');
    String namespaceAttribute = colon < 0 ? "xmlns" : "xmlns:" + name.substring(0, colon);
    for (Node ancestor = node; ancestor instanceof Element; ancestor = ancestor.getParentNode()) {
      Element element = (Element) ancestor;
      if (element.hasAttribute(namespaceAttribute)) {
        return "urn:oasis:names:tc:xliff:document:1.2"
            .equals(element.getAttribute(namespaceAttribute));
      }
    }
    return false;
  }

  private void addStyleItemReference(String value, ImmutableSet.Builder<RDotTxtEntry> references) {
    if (value.length() < 2 || (value.charAt(0) != '@' && value.charAt(0) != '?')) {
      return;
    }
    boolean themeReference = value.charAt(0) == '?';
    boolean createId = value.startsWith("@+");
    String resourceName = value.substring(createId || value.charAt(1) == '*' ? 2 : 1);
    int colon = resourceName.indexOf(':');
    int slash = resourceName.indexOf('/');
    if (colon >= 0) {
      int packageStart = slash >= 0 && slash < colon ? slash + 1 : 0;
      String resourcePackage = resourceName.substring(packageStart, colon);
      if (resourcePackage.isEmpty() || resourcePackage.equals("android")) {
        return;
      }
      resourceName = resourceName.substring(0, packageStart) + resourceName.substring(colon + 1);
    }

    slash = resourceName.indexOf('/');
    RType rType;
    String name;
    if (slash < 0) {
      if (!themeReference) {
        return;
      }
      rType = RType.ATTR;
      name = resourceName;
    } else {
      String type = resourceName.substring(0, slash);
      rType = RESOURCE_TYPES.get(type);
      if (rType == null || !type.equals(rType.toString())) {
        return;
      }
      name = resourceName.substring(slash + 1);
    }
    if (name.isEmpty()
        || (themeReference && rType != RType.ATTR)
        || (createId && rType != RType.ID)) {
      return;
    }
    if (createId) {
      resourceCollector.addIntResourceIfNotPresent(RType.ID, sanitizeName(name));
    } else {
      references.add(new FakeRDotTxtEntry(IdType.INT, rType, sanitizeName(name)));
    }
  }

  private Node verifyNodeHasTypeAttribute(Path valuesFile, Node node)
      throws ResourceParseException {
    Node typeNode = Objects.requireNonNull(node.getAttributes()).getNamedItem("type");
    if (typeNode == null || Objects.requireNonNull(typeNode.getNodeValue()).isEmpty()) {
      throw new ResourceParseException(
          "Error parsing file '%s', expected a 'type' attribute in: \n'%s'\n",
          Objects.requireNonNull(valuesFile.getFileName()), node.toString());
    }
    return typeNode;
  }

  private void addToResourceCollector(Node node, RType rType) throws ResourceParseException {
    String resourceName = sanitizeName(extractNameAttribute(node));

    if (rType.equals(RType.STYLEABLE)) {
      int count = 0;
      for (Node attrNode = node.getFirstChild();
          attrNode != null;
          attrNode = attrNode.getNextSibling()) {
        if (attrNode.getNodeType() != Node.ELEMENT_NODE || !attrNode.getNodeName().equals("attr")) {
          continue;
        }

        String rawAttrName = extractNameAttribute(attrNode);
        String attrName = sanitizeName(rawAttrName);
        resourceCollector.addResource(
            RType.STYLEABLE,
            IdType.INT,
            String.format("%s_%s", resourceName, attrName),
            Integer.toString(count++),
            resourceName);

        if (!rawAttrName.startsWith("android:")) {
          resourceCollector.addIntResourceIfNotPresent(RType.ATTR, attrName);
        }
      }

      resourceCollector.addIntArrayResourceIfNotPresent(rType, resourceName, count);
    } else {
      resourceCollector.addIntResourceIfNotPresent(rType, resourceName);
    }
  }

  @VisibleForTesting
  void processStyleFile(Path xmlFile, ImmutableSet.Builder<RDotTxtEntry> references)
      throws IOException, XPathExpressionException, ResourceParseException {
    try (InputStream stream = new BufferedInputStream(Files.newInputStream(xmlFile))) {
      Document dom = parseXml(xmlFile, stream);

      XPathExpression expression = ANDROID_ATTR_USAGE_FOR_STYLES;
      NodeList nodesUsingIds =
          (NodeList)
              Objects.requireNonNull(
                  expression.evaluate(dom, Objects.requireNonNull(XPathConstants.NODESET)));
      for (int i = 0; i < nodesUsingIds.getLength(); i++) {
        String resourceName =
            Objects.requireNonNull(Objects.requireNonNull(nodesUsingIds.item(i)).getNodeValue());
        if (resourceName.startsWith("?attr")) {
          resourceName = resourceName.substring("?attr/".length());
        } else {
          int colonPosition = resourceName.indexOf(":");
          if (colonPosition > 0) {
            resourceName = resourceName.substring(colonPosition + 1);
          }
        }
        references.add(new FakeRDotTxtEntry(IdType.INT, RType.ATTR, sanitizeName(resourceName)));
      }
    }
  }

  @VisibleForTesting
  void processXmlFile(Path xmlFile, ImmutableSet.Builder<RDotTxtEntry> references)
      throws IOException, XPathExpressionException, ResourceParseException {
    try (InputStream stream = new BufferedInputStream(Files.newInputStream(xmlFile))) {
      Document dom = parseXml(xmlFile, stream);
      NodeList nodesWithIds =
          (NodeList)
              Objects.requireNonNull(
                  ANDROID_ID_DEFINITION.evaluate(
                      dom, Objects.requireNonNull(XPathConstants.NODESET)));
      for (int i = 0; i < nodesWithIds.getLength(); i++) {
        String resourceName =
            Objects.requireNonNull(Objects.requireNonNull(nodesWithIds.item(i)).getNodeValue());
        if (!resourceName.startsWith(ID_DEFINITION_PREFIX)) {
          throw new ResourceParseException("Invalid definition of a resource: '%s'", resourceName);
        }
        Preconditions.checkState(resourceName.startsWith(ID_DEFINITION_PREFIX));
        resourceCollector.addIntResourceIfNotPresent(
            RType.ID, resourceName.substring(ID_DEFINITION_PREFIX.length()));
      }

      NodeList nodesUsingIds =
          (NodeList)
              Objects.requireNonNull(
                  ANDROID_ID_AND_ATTR_USAGE.evaluate(
                      dom, Objects.requireNonNull(XPathConstants.NODESET)));
      for (int i = 0; i < nodesUsingIds.getLength(); i++) {
        String resourceName =
            Objects.requireNonNull(Objects.requireNonNull(nodesUsingIds.item(i)).getNodeValue());
        String nodeName = Objects.requireNonNull(nodesUsingIds.item(i)).getNodeName();
        if (nodeName.startsWith("tools:")) {
          continue;
        }
        addReference(resourceName, xmlFile, references);
      }
    }
  }

  private static void addReference(
      String resourceName, Path xmlFile, ImmutableSet.Builder<RDotTxtEntry> references)
      throws ResourceParseException {
    int slashPosition = resourceName.indexOf('/');
    if ((resourceName.charAt(0) != '@' && resourceName.charAt(0) != '?') || slashPosition == -1) {
      throw new ResourceParseException("Invalid definition of a resource: '%s'", resourceName);
    }

    String rawRType = resourceName.substring(1, slashPosition);
    String name = resourceName.substring(slashPosition + 1);
    if (name.startsWith("android:")) {
      return;
    }
    if (!RESOURCE_TYPES.containsKey(rawRType)) {
      throw new ResourceParseException("Invalid reference '%s' in '%s'", resourceName, xmlFile);
    }
    RType rType = Objects.requireNonNull(RESOURCE_TYPES.get(rawRType));
    references.add(new FakeRDotTxtEntry(IdType.INT, rType, sanitizeName(name)));
  }

  private static Document parseXml(Path filepath, InputStream inputStream)
      throws IOException, ResourceParseException {
    try {
      return XmlDomParser.parse(inputStream);
    } catch (SAXException e) {
      throw new ResourceParseException("Error parsing xml file '%s': %s.", filepath, e.toString());
    }
  }

  private static String extractNameAttribute(Node node) throws ResourceParseException {
    Node attribute = Objects.requireNonNull(node.getAttributes()).getNamedItem("name");
    if (attribute == null) {
      throw new ResourceParseException(
          "Error: expected a 'name' attribute in node '%s' with value '%s'",
          node.getNodeName(), node.getTextContent());
    }
    return Objects.requireNonNull(attribute.getNodeValue());
  }

  private static String sanitizeName(String rawName) {
    return rawName.replaceAll("[.:]", "_");
  }

  private static boolean isAValuesDir(String dirname) {
    return dirname.equals("values") || dirname.startsWith("values-");
  }

  @VisibleForTesting
  static boolean shouldIgnoreFile(Path path) throws IOException {
    return Files.isHidden(path)
        || IGNORED_FILE_EXTENSIONS.contains(
            com.google.common.io.Files.getFileExtension(
                Objects.requireNonNull(path.getFileName()).toString()))
        || isSilentlyIgnored(path);
  }

  public ImmutableSet<RDotTxtEntry> verifyReferences(ImmutableSet<RDotTxtEntry> references)
      throws IOException {
    ImmutableSet.Builder<RDotTxtEntry> unresolved = ImmutableSet.builder();
    ImmutableSet.Builder<RDotTxtEntry> definitionsBuilder = ImmutableSet.builder();
    definitionsBuilder.addAll(resourceCollector.getResources());
    for (Path depRTxt : pathsToSymbolsOfDeps) {
      Iterable<String> lines =
          Files.readAllLines(depRTxt).stream()
              .filter(input -> !Strings.isNullOrEmpty(input))
              .collect(Collectors.toList());
      for (String line : lines) {
        Optional<RDotTxtEntry> entry = RDotTxtEntry.parse(line);
        Preconditions.checkState(entry.isPresent());
        definitionsBuilder.add(entry.get());
      }
    }

    Set<RDotTxtEntry> definitions = definitionsBuilder.build();
    for (RDotTxtEntry reference : references) {
      if (!definitions.contains(reference)) {
        unresolved.add(reference);
      }
    }
    return unresolved.build();
  }

  public static class ResourceParseException extends Exception {

    ResourceParseException(String messageFormat, Object... args) {
      super(String.format(messageFormat, args));
    }
  }

  /**
   * Responsible for collecting resources parsed by {@link MiniAapt} and assigning unique integer
   * ids to those resources. Resource ids are of the type {@code 0x7fxxyyyy}, where {@code xx}
   * represents the resource type, and {@code yyyy} represents the id within that resource type.
   */
  public static class RDotTxtResourceCollector {

    private int currentTypeId;
    private final Map<RType, ResourceIdEnumerator> enumerators;
    private final Set<RDotTxtEntry> resources;

    public RDotTxtResourceCollector() {
      this.enumerators = new HashMap<>();
      this.resources = new HashSet<>();
      this.currentTypeId = 1;
    }

    public void addIntResourceIfNotPresent(RType rType, String name) {
      RDotTxtEntry entry = new FakeRDotTxtEntry(IdType.INT, rType, name);
      if (!resources.contains(entry)) {
        addResource(rType, IdType.INT, name, getNextIdValue(rType), null);
      }
    }

    public void addIntArrayResourceIfNotPresent(RType rType, String name, int numValues) {
      addResource(rType, IdType.INT_ARRAY, name, getNextArrayIdValue(rType, numValues), null);
    }

    public void addResource(
        RType rType, IdType idType, String name, String idValue, @Nullable String parent) {
      resources.add(new RDotTxtEntry(idType, rType, name, idValue, parent));
    }

    public Set<RDotTxtEntry> getResources() {
      return Collections.unmodifiableSet(resources);
    }

    ResourceIdEnumerator getEnumerator(RType rType) {
      if (!enumerators.containsKey(rType)) {
        enumerators.put(rType, new ResourceIdEnumerator(currentTypeId++));
      }
      return Objects.requireNonNull(enumerators.get(rType));
    }

    String getNextIdValue(RType rType) {
      return String.format("0x%08x", getEnumerator(rType).next());
    }

    String getNextArrayIdValue(RType rType, int numValues) {
      // Robolectric expects the array to be populated with the right number of values, irrespective
      // of what the values are.
      ImmutableList.Builder<String> values = ImmutableList.builder();
      for (int id = 0; id < numValues; id++) {
        values.add(String.format("0x%x", getEnumerator(rType).next()));
      }

      return String.format(
          "{ %s }", Joiner.on(RDotTxtEntry.INT_ARRAY_SEPARATOR).join(values.build()));
    }

    private static class ResourceIdEnumerator {

      private int currentId;

      ResourceIdEnumerator(int typeId) {
        this.currentId = 0x7f000000 + 0x10000 * typeId + 1;
      }

      int next() {
        return currentId++;
      }
    }
  }
}
