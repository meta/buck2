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

import static com.facebook.buck.android.aapt.RDotTxtEntryUtil.FakeEntry;
import static org.hamcrest.MatcherAssert.assertThat;
import static org.hamcrest.Matchers.containsString;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.facebook.buck.android.aapt.MiniAapt.ResourceParseException;
import com.facebook.buck.android.aapt.RDotTxtEntry.IdType;
import com.facebook.buck.android.aapt.RDotTxtEntry.RType;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Set;
import java.util.function.Function;
import java.util.stream.Collectors;
import javax.xml.xpath.XPathExpressionException;
import org.hamcrest.core.IsEqual;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.ExpectedException;

public class MiniAaptTest {

  private static final ImmutableList<String> RESOURCES =
      ImmutableList.<String>builder()
          .add(
              "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
              "<LinearLayout>",
              "<Button android:id=\"@+id/button1\" ",
              "android:layout_toLeftOf=\"@id/button2\" ",
              "android:text=\"@string/text\" />",
              "<Button android:id=\"@+id/button3\" ",
              "style:attribute=\"@style/Buck.Theme\" ",
              "android:background=\"@drawable/some_image\" />",
              "<TextView tools:showIn=\"@layout/some_layout\" ",
              "android:id=\"@id/android:empty\" ",
              "android:background=\"?attr/some_attr\"/>",
              "</LinearLayout>")
          .build();

  @Rule public TemporaryPaths tmpFolder = new TemporaryPaths();

  @Rule public ExpectedException thrown = ExpectedException.none();

  private static Set<RDotTxtEntry> createTestingFakes(
      Set<RDotTxtEntry> entries, Function<RDotTxtEntry, RDotTxtEntry> converter) {
    return entries.stream().map(converter).collect(Collectors.toSet());
  }

  private static Set<RDotTxtEntry> createTestingFakes(Set<RDotTxtEntry> entries) {
    return createTestingFakes(entries, RDotTxtEntryUtil::matchDefault);
  }

  private static Set<RDotTxtEntry> createTestingFakesWithParents(Set<RDotTxtEntry> entries) {
    return createTestingFakes(entries, RDotTxtEntryUtil::matchParent);
  }

  @Before
  public void setUp() {}

  @Test
  public void testFindingResourceIdsInXml()
      throws IOException, XPathExpressionException, ResourceParseException {
    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(), RESOURCES, Paths.get("resource.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    ImmutableSet.Builder<RDotTxtEntry> references = ImmutableSet.builder();
    aapt.processXmlFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("resource.xml")),
        references);

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    assertEquals(
        createTestingFakes(definitions),
        ImmutableSet.of(
            FakeEntry.create(IdType.INT, RType.ID, "button1"),
            FakeEntry.create(IdType.INT, RType.ID, "button3")));

    assertEquals(
        createTestingFakes(references.build()),
        ImmutableSet.of(
            FakeEntry.create(IdType.INT, RType.DRAWABLE, "some_image"),
            FakeEntry.create(IdType.INT, RType.STRING, "text"),
            FakeEntry.create(IdType.INT, RType.STYLE, "Buck_Theme"),
            FakeEntry.create(IdType.INT, RType.ID, "button2"),
            FakeEntry.create(IdType.INT, RType.ATTR, "some_attr")));
  }

  @Test
  public void testParsingFilesUnderValuesDirectory() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<string name=\"hello\">Hello, <xliff:g id=\"name\">%s</xliff:g>!</string>",
                "<plurals name=\"people\">",
                "   <item quantity=\"zero\">ignore1</item>",
                "   <item quantity=\"many\">ignore2</item>",
                "</plurals>",
                "<skip />",
                "<integer name=\"number\">100</integer>",
                "<dimen name=\"dimension\">100sp</dimen>",
                "<declare-styleable name=\"MyNiceView\">",
                "   <attr name=\"titleText\" />",
                "   <attr name=\"subtitleText\" format=\"string\" />",
                "   <attr name=\"complexAttr\">",
                "       <enum name=\"shouldBeIgnored\" value=\"0\" />",
                "       <enum name=\"alsoIgnore\" value=\"1\" />",
                "       <flag name=\"uselessFlag\" value=\"0x00\" />",
                "   </attr>",
                "   <attr name=\"android:layout_gravity\" />",
                "   <item name=\"should_be_ignored\" />",
                "</declare-styleable>",
                "<eat-comment />",
                "<item type=\"id\" name=\"some_id\" />",
                "<style name=\"Widget.Theme\">",
                "  <item name=\"ignoreMe\" />",
                "</style>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    assertEquals(
        createTestingFakes(definitions),
        ImmutableSet.of(
            FakeEntry.create(IdType.INT, RType.STRING, "hello"),
            FakeEntry.create(IdType.INT, RType.PLURALS, "people"),
            FakeEntry.create(IdType.INT, RType.INTEGER, "number"),
            FakeEntry.create(IdType.INT, RType.DIMEN, "dimension"),
            FakeEntry.create(IdType.INT_ARRAY, RType.STYLEABLE, "MyNiceView"),
            FakeEntry.create(IdType.INT, RType.STYLEABLE, "MyNiceView_titleText"),
            FakeEntry.create(IdType.INT, RType.STYLEABLE, "MyNiceView_subtitleText"),
            FakeEntry.create(IdType.INT, RType.STYLEABLE, "MyNiceView_complexAttr"),
            FakeEntry.create(IdType.INT, RType.STYLEABLE, "MyNiceView_android_layout_gravity"),
            FakeEntry.create(IdType.INT, RType.ATTR, "titleText"),
            FakeEntry.create(IdType.INT, RType.ATTR, "subtitleText"),
            FakeEntry.create(IdType.INT, RType.ATTR, "complexAttr"),
            FakeEntry.create(IdType.INT, RType.ID, "some_id"),
            FakeEntry.create(IdType.INT, RType.STYLE, "Widget_Theme")));

    boolean foundElement = false;
    for (RDotTxtEntry definition : definitions) {
      if (definition.name.equals("MyNiceView")) {
        assertEquals("{ 0x7f060001,0x7f060002,0x7f060003,0x7f060004 }", definition.idValue);
        foundElement = true;
      }
    }
    assertTrue(foundElement);
  }

  @Test
  public void testParentIsSet() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n"
                    + "<resources>\n"
                    + "    <attr name=\"justAttr\"/>\n"
                    + "    <declare-styleable name=\"MyLayout\">\n"
                    + "        <attr name=\"myAttr\"/>\n"
                    + "        <attr name=\"myAttr2\"/>\n"
                    + "    </declare-styleable>\n"
                    + "    <declare-styleable name=\"MyLayout_Layout\">\n"
                    + "        <attr name=\"android:text\"/>\n"
                    + "        <attr name=\"android:color\"/>\n"
                    + "    </declare-styleable>\n"
                    + "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    assertEquals(
        createTestingFakesWithParents(definitions),
        ImmutableSet.of(
            FakeEntry.create(IdType.INT, RType.ATTR, "justAttr"),
            FakeEntry.create(IdType.INT_ARRAY, RType.STYLEABLE, "MyLayout"),
            FakeEntry.createWithParent(IdType.INT, RType.STYLEABLE, "MyLayout_myAttr", "MyLayout"),
            FakeEntry.createWithParent(IdType.INT, RType.STYLEABLE, "MyLayout_myAttr2", "MyLayout"),
            FakeEntry.create(IdType.INT_ARRAY, RType.STYLEABLE, "MyLayout_Layout"),
            FakeEntry.createWithParent(
                IdType.INT, RType.STYLEABLE, "MyLayout_Layout_android_text", "MyLayout_Layout"),
            FakeEntry.createWithParent(
                IdType.INT, RType.STYLEABLE, "MyLayout_Layout_android_color", "MyLayout_Layout"),
            FakeEntry.create(IdType.INT, RType.ATTR, "myAttr"),
            FakeEntry.create(IdType.INT, RType.ATTR, "myAttr2")));
  }

  @Test
  public void testParsingValuesExcludedFromResMap() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources exclude-from-buck-resource-map=\"true\">",
                "<string name=\"hello\">Hello, <xliff:g id=\"name\">%s</xliff:g>!</string>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    assertTrue(definitions.isEmpty());
  }

  @Test
  public void testParsingValuesNotExcludedFromResMap() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources exclude-from-buck-resource-map=\"false\">",
                "<string name=\"hello\">Hello, <xliff:g id=\"name\">%s</xliff:g>!</string>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    assertEquals(
        createTestingFakes(definitions),
        ImmutableSet.<RDotTxtEntry>of(FakeEntry.create(IdType.INT, RType.STRING, "hello")));
  }

  @Test
  public void testParsingAndroidDrawables() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<bitmap xmlns:android=\"http://schemas.android.com/apk/res/android\">",
                "  xmlns:fbui=\"http://schemas.android.com/apk/res-auto\"",
                "  android:src=\"@drawable/other_bitmap\"",
                "  >",
                "</bitmap>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(), lines, Paths.get("android_drawable.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processDrawables(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("android_drawable.xml")));

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    assertEquals(
        createTestingFakes(definitions),
        ImmutableSet.<RDotTxtEntry>of(
            FakeEntry.create(IdType.INT, RType.DRAWABLE, "android_drawable")));
  }

  private void testParsingGrayscaleImageImpl(String normalFilename, String grayscaleFilename)
      throws IOException, ResourceParseException {
    ImmutableList<String> lines = ImmutableList.<String>builder().add("").build();
    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get(normalFilename));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processDrawables(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get(grayscaleFilename)));

    Set<RDotTxtEntry> definitions = aapt.getResourceCollector().getResources();

    // Both spellings of the grayscale suffix are trimmed off the resource name.
    assertThat(
        createTestingFakes(definitions),
        IsEqual.equalToObject(
            ImmutableSet.<RDotTxtEntry>of(
                FakeEntry.create(IdType.INT, RType.DRAWABLE, "fbui_tomato"))));
  }

  @Test
  public void testParsingGrayscaleImage() throws IOException, ResourceParseException {
    testParsingGrayscaleImageImpl("fbui_tomato.png", "fbui_tomato.g.png");
    testParsingGrayscaleImageImpl("fbui_tomato.png", "fbui_tomato_g.png");
  }

  @Test(expected = ResourceParseException.class)
  public void testInvalidResourceType() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<resourcetype name=\"number\">100</resourcetype>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));
  }

  @Test(expected = ResourceParseException.class)
  public void testInvalidItemResource() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<item name=\"number\">100</item>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));
  }

  @Test
  public void testInvalidDefinition() throws XPathExpressionException, IOException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<LinearLayout>",
                "<Button android:id=\"@+string/button1\" ",
                "android:layout_toLeftOf=\"@id/button2\" ",
                "android:text=\"@string/text\" />",
                "</LinearLayout>")
            .build();

    Path resource = Paths.get("resource.xml");
    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, resource);

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    try {
      aapt.processXmlFile(
          ProjectFilesystemUtils.getPathForRelativePath(tmpFolder.getRoot(), resource),
          ImmutableSet.builder());
      fail("MiniAapt should throw parsing '@+string/button1'");
    } catch (ResourceParseException e) {
      assertThat(e.getMessage(), containsString("Invalid definition of a resource"));
    }
  }

  @Test
  public void testInvalidReference() throws IOException, XPathExpressionException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<LinearLayout>",
                "<Button android:id=\"@+id/button1\" ",
                "android:layout_toLeftOf=\"@someresource/button2\" ",
                "android:text=\"@string/text\" />",
                "</LinearLayout>")
            .build();

    Path resource = Paths.get("resource.xml");
    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, resource);

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    try {
      aapt.processXmlFile(
          ProjectFilesystemUtils.getPathForRelativePath(tmpFolder.getRoot(), resource),
          ImmutableSet.builder());
      fail("MiniAapt should throw parsing '@someresource/button2'");
    } catch (ResourceParseException e) {
      assertThat(e.getMessage(), containsString("Invalid reference '@someresource/button2'"));
    }
  }

  @Test
  public void testMissingNameAttribute() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage("Error: expected a 'name' attribute in node 'string' with value 'Howdy!'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<string notname=\"hello\">Howdy!</string>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("values.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("values.xml")));
  }

  @Test
  public void testVerifyReferences()
      throws IOException, XPathExpressionException, ResourceParseException {
    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(), RESOURCES, Paths.get("resource.xml"));

    ImmutableList<String> rDotTxt =
        ImmutableList.of(
            "int string text 0x07010001",
            "int style Buck_Theme 0x07020001",
            "int id button2 0x07030001",
            "int attr attribute 0x07040001");

    RelPath depRTxt = RelPath.get("dep/R.txt");
    ProjectFilesystemUtils.createParentDirs(tmpFolder.getRoot(), depRTxt.getPath());
    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), rDotTxt, depRTxt.getPath());

    MiniAapt aapt =
        new MiniAapt(
            ImmutableSet.of(
                ProjectFilesystemUtils.getPathForRelativePath(tmpFolder.getRoot(), depRTxt)));
    ImmutableSet.Builder<RDotTxtEntry> references = ImmutableSet.builder();
    aapt.processXmlFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("resource.xml")),
        references);

    Set<RDotTxtEntry> missing = aapt.verifyReferences(references.build());

    assertEquals(
        ImmutableSet.<RDotTxtEntry>of(
            FakeEntry.create(IdType.INT, RType.DRAWABLE, "some_image"),
            FakeEntry.create(IdType.INT, RType.ATTR, "some_attr")),
        createTestingFakes(missing));
  }

  @Test
  public void testVerifyStyleItemReferences()
      throws IOException, XPathExpressionException, ResourceParseException {
    Path valuesFile = Paths.get("res/values/themes.xml");
    ProjectFilesystemUtils.createParentDirs(tmpFolder.getRoot(), valuesFile);
    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(),
        ImmutableList.of(
            "<resources>",
            "<color name=\"local_color\">#ffffff</color>",
            "<style name=\"ImageCanvas.TextButton\">",
            "<item name=\"android:textColor\">@color/local_color</item>",
            "<item name=\"android:background\">@drawable/background</item>",
            "<item name=\"android:tint\">?attr/accent</item>",
            "<item name=\"android:colorAccent\">?accent</item>",
            "<item name=\"android:shadowColor\">\n @color/shadow_color \n</item>",
            "</style>",
            "</resources>"),
        valuesFile);
    Path nightFile = Paths.get("res/values-night/themes.xml");
    ProjectFilesystemUtils.createParentDirs(tmpFolder.getRoot(), nightFile);
    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(),
        ImmutableList.of(
            "<resources><style name=\"NightTheme\">",
            "<item name=\"android:textSize\">@dimen/night_text_size</item>",
            "</style></resources>"),
        nightFile);
    Path depSymbols = tmpFolder.getRoot().resolve("R.txt").getPath();
    Files.write(
        depSymbols,
        ImmutableList.of("int drawable background 0x7f010001", "int attr accent 0x7f020001"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of(depSymbols));
    ImmutableSet<RDotTxtEntry> references =
        aapt.processAllFiles(MiniAapt.getAllResourceFiles(tmpFolder.getRoot(), RelPath.get("res")));

    assertEquals(
        ImmutableSet.of(
            FakeEntry.create(IdType.INT, RType.COLOR, "shadow_color"),
            FakeEntry.create(IdType.INT, RType.DIMEN, "night_text_size")),
        createTestingFakes(aapt.verifyReferences(references)));

    Files.write(
        depSymbols,
        ImmutableList.of(
            "int drawable background 0x7f010001",
            "int attr accent 0x7f020001",
            "int color shadow_color 0x7f030001",
            "int dimen night_text_size 0x7f040001"));
    assertTrue(aapt.verifyReferences(references).isEmpty());
  }

  @Test
  public void testStyleItemFrameworkReferencesAndLiterals()
      throws IOException, XPathExpressionException, ResourceParseException {
    Path valuesFile = Paths.get("res/values/styles.xml");
    ProjectFilesystemUtils.createParentDirs(tmpFolder.getRoot(), valuesFile);
    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(),
        ImmutableList.of(
            "<resources><style name=\"Theme\">",
            "<item name=\"android:textColor\">@android:color/white</item>",
            "<item name=\"android:textColorHint\">?android:attr/textColorHint</item>",
            "<item name=\"android:background\">@null</item>",
            "<item name=\"android:foreground\">@empty</item>",
            "<item name=\"android:shadowColor\">#ff000000</item>",
            "<item name=\"android:textSize\">16sp</item>",
            "<item name=\"android:text\">\\@color/literal</item>",
            "<item name=\"android:hint\">\"@string/quoted\"</item>",
            "</style></resources>"),
        valuesFile);

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    ImmutableSet<RDotTxtEntry> references =
        aapt.processAllFiles(MiniAapt.getAllResourceFiles(tmpFolder.getRoot(), RelPath.get("res")));

    assertTrue(references.isEmpty());
  }

  @Test
  public void testStyleItemIdDefinition()
      throws IOException, XPathExpressionException, ResourceParseException {
    Path valuesFile = Paths.get("res/values/styles.xml");
    ProjectFilesystemUtils.createParentDirs(tmpFolder.getRoot(), valuesFile);
    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(),
        ImmutableList.of(
            "<resources><style name=\"Theme\">",
            "<item name=\"android:id\">@+id/styled_view</item>",
            "<item name=\"android:nextFocusDown\">@id/styled_view</item>",
            "</style></resources>"),
        valuesFile);

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    ImmutableSet<RDotTxtEntry> references =
        aapt.processAllFiles(MiniAapt.getAllResourceFiles(tmpFolder.getRoot(), RelPath.get("res")));

    assertEquals(
        ImmutableSet.of(FakeEntry.create(IdType.INT, RType.ID, "styled_view")),
        createTestingFakes(references));
    assertTrue(aapt.verifyReferences(references).isEmpty());
  }

  @Test
  public void testStyleItemResourceLikeLiterals() throws IOException, ResourceParseException {
    for (String value :
        ImmutableList.of(
            "@",
            "@color",
            "@0",
            "@0x01040000",
            "@hello/world",
            "@color/",
            "@:color/literal",
            "@color/:literal",
            "@string-array/literal",
            "@integer-array/literal",
            "@declare-styleable/literal",
            "@+string/literal",
            "?",
            "?attr/",
            "?foo/bar",
            "?color/literal")) {
      MiniAapt aapt = new MiniAapt(ImmutableSet.of());
      assertTrue(value, processStyleItemValues(aapt, value).isEmpty());
      assertEquals(
          value,
          ImmutableSet.of(FakeEntry.create(IdType.INT, RType.STYLE, "Theme")),
          createTestingFakes(aapt.getResourceCollector().getResources()));
    }
  }

  @Test
  public void testQualifiedStyleItemReferences() throws IOException, ResourceParseException {
    Path depSymbols = tmpFolder.getRoot().resolve("R.txt").getPath();
    Files.write(depSymbols, ImmutableList.of("int color accent 0x7f010001"));
    for (String value :
        ImmutableList.of(
            "@com.example:color/accent",
            "@*com.example:color/accent",
            "@*color/accent",
            "@color/com.example:accent",
            "@*color/com.example:accent")) {
      MiniAapt aapt = new MiniAapt(ImmutableSet.of(depSymbols));
      ImmutableSet<RDotTxtEntry> references = processStyleItemValues(aapt, value);
      assertEquals(
          value,
          ImmutableSet.of(FakeEntry.create(IdType.INT, RType.COLOR, "accent")),
          createTestingFakes(references));
      assertTrue(value, aapt.verifyReferences(references).isEmpty());
      assertEquals(value, references, new MiniAapt(ImmutableSet.of()).verifyReferences(references));
    }

    Files.write(depSymbols, ImmutableList.of("int attr label_appearance 0x7f010001"));
    for (String value :
        ImmutableList.of(
            "?com.example:attr/label.appearance",
            "?com.example:label.appearance",
            "?*com.example:attr/label.appearance",
            "?*attr/label.appearance",
            "?attr/com.example:label.appearance")) {
      MiniAapt aapt = new MiniAapt(ImmutableSet.of(depSymbols));
      ImmutableSet<RDotTxtEntry> references = processStyleItemValues(aapt, value);
      assertEquals(
          value,
          ImmutableSet.of(FakeEntry.create(IdType.INT, RType.ATTR, "label_appearance")),
          createTestingFakes(references));
      assertTrue(value, aapt.verifyReferences(references).isEmpty());
      assertEquals(value, references, new MiniAapt(ImmutableSet.of()).verifyReferences(references));
    }
  }

  @Test
  public void testPrivateAndLegacyFrameworkStyleItemReferences()
      throws IOException, ResourceParseException {
    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    assertTrue(
        processStyleItemValues(
                aapt,
                "@*android:color/white",
                "@color/android:white",
                "@id/android:empty",
                "?android:textColor",
                "?*android:attr/textColor",
                "?attr/android:textColor")
            .isEmpty());
  }

  @Test
  public void testStyledStringItems() throws IOException, ResourceParseException {
    for (String value :
        ImmutableList.of(
            "<b>@string/missing</b>",
            "<b/>@string/missing",
            "<annotation key=\"value\">?missing</annotation>",
            "<xliff:g xmlns:xliff=\"urn:oasis:names:tc:xliff:document:1.2\" id=\"x\">"
                + "<i>@string/missing</i></xliff:g>",
            "<g xmlns=\"urn:oasis:names:tc:xliff:document:1.2\" id=\"x\">"
                + "<b xmlns=\"\">@string/missing</b></g>")) {
      assertTrue(value, processStyleItemValues(new MiniAapt(ImmutableSet.of()), value).isEmpty());
    }
  }

  @Test
  public void testStyleItemReferencesWithTransparentMarkup()
      throws IOException, ResourceParseException {
    for (String value :
        ImmutableList.of(
            "<![CDATA[@string/missing]]>",
            "@string/<!-- comment -->missing",
            "<xliff:g xmlns:xliff=\"urn:oasis:names:tc:xliff:document:1.2\" id=\"x\">"
                + "@string/missing</xliff:g>",
            "<x:g xmlns:x=\"urn:oasis:names:tc:xliff:document:1.2\" id=\"x\">"
                + "@string/missing</x:g>",
            "<g xmlns=\"urn:oasis:names:tc:xliff:document:1.2\" id=\"x\">" + "@string/missing</g>",
            "<x:g xmlns:x=\"urn:oasis:names:tc:xliff:document:1.2\" id=\"x\">"
                + "<x:g id=\"y\">@string/missing</x:g></x:g>")) {
      MiniAapt aapt = new MiniAapt(ImmutableSet.of());
      ImmutableSet<RDotTxtEntry> references = processStyleItemValues(aapt, value);
      assertEquals(
          value,
          ImmutableSet.of(FakeEntry.create(IdType.INT, RType.STRING, "missing")),
          createTestingFakes(references));
      assertEquals(value, references, aapt.verifyReferences(references));
    }
  }

  @Test
  public void testDottedAndQualifiedStyleItemIdDefinitions()
      throws IOException, ResourceParseException {
    for (String value :
        ImmutableList.of(
            "@+id/styled.view", "@+com.example:id/styled.view", "@+id/com.example:styled.view")) {
      MiniAapt aapt = new MiniAapt(ImmutableSet.of());
      ImmutableSet<RDotTxtEntry> references =
          processStyleItemValues(aapt, value, "@id/styled.view");
      assertEquals(
          value,
          ImmutableSet.of(FakeEntry.create(IdType.INT, RType.ID, "styled_view")),
          createTestingFakes(references));
      assertTrue(value, aapt.verifyReferences(references).isEmpty());
    }
  }

  @Test
  public void testStyleItemReferenceNames() throws IOException, ResourceParseException {
    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    ImmutableSet<RDotTxtEntry> references =
        processStyleItemValues(aapt, "?attr", "@string/missing suffix", "@string/missing/path");
    assertEquals(
        ImmutableSet.of(
            FakeEntry.create(IdType.INT, RType.ATTR, "attr"),
            FakeEntry.create(IdType.INT, RType.STRING, "missing suffix"),
            FakeEntry.create(IdType.INT, RType.STRING, "missing/path")),
        createTestingFakes(aapt.verifyReferences(references)));
  }

  private ImmutableSet<RDotTxtEntry> processStyleItemValues(MiniAapt aapt, String... values)
      throws IOException, ResourceParseException {
    ImmutableList.Builder<String> lines = ImmutableList.builder();
    lines.add("<resources><style name=\"Theme\">");
    for (String value : values) {
      lines.add("<item name=\"android:text\">" + value + "</item>");
    }
    lines.add("</style></resources>");
    Path valuesFile = tmpFolder.getRoot().resolve("styles.xml").getPath();
    Files.write(valuesFile, lines.build());
    return aapt.processValuesFile(valuesFile);
  }

  @Test
  public void testInvalidNodeId()
      throws IOException, XPathExpressionException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage("Invalid definition of a resource: '@button2'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<LinearLayout>",
                "<Button android:id=\"@+id/button1\" ",
                "android:layout_toLeftOf=\"@button2\" />",
                "</LinearLayout>")
            .build();

    Path resource = Paths.get("resource.xml");
    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, resource);

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());
    aapt.processXmlFile(
        ProjectFilesystemUtils.getPathForRelativePath(tmpFolder.getRoot(), resource),
        ImmutableSet.builder());
  }

  @Test
  public void testProcessNonValuesFiles() throws IOException, ResourceParseException {
    ProjectFilesystemUtils.createParentDirs(
        tmpFolder.getRoot(), Paths.get("res/drawable/icon.png"));
    Files.createFile(tmpFolder.getRoot().resolve("res/drawable/icon.png").getPath());
    ProjectFilesystemUtils.createParentDirs(
        tmpFolder.getRoot(), Paths.get("res/drawable-ldpi/nine_patch.9.png"));
    Files.createFile(tmpFolder.getRoot().resolve("res/drawable-ldpi/nine_patch.9.png").getPath());
    ProjectFilesystemUtils.createParentDirs(
        tmpFolder.getRoot(), Paths.get("res/transition-v19/some_transition.xml"));
    Files.createFile(
        tmpFolder.getRoot().resolve("res/transition-v19/some_transition.xml").getPath());

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processNonValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(tmpFolder.getRoot(), "res/drawable/icon.png")
            .getPath(),
        "drawable");
    aapt.processNonValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
                tmpFolder.getRoot(), "res/drawable/drawable-ldpi/nine_patch.9.png")
            .getPath(),
        "drawable-ldpi");
    aapt.processNonValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
                tmpFolder.getRoot(), "res/transition-v19/some_transition.xml")
            .getPath(),
        "transition-v19");

    assertEquals(
        ImmutableSet.<RDotTxtEntry>of(
            FakeEntry.create(IdType.INT, RType.DRAWABLE, "icon"),
            FakeEntry.create(IdType.INT, RType.DRAWABLE, "nine_patch"),
            FakeEntry.create(IdType.INT, RType.TRANSITION, "some_transition")),
        createTestingFakes(aapt.getResourceCollector().getResources()));
  }

  @Test
  public void testIgnoredFiles() throws IOException {
    ProjectFilesystemUtils.createParentDirs(
        tmpFolder.getRoot(), Paths.get("res/drawable/another_icon.png.orig"));
    Files.createFile(tmpFolder.getRoot().resolve("res/drawable/another_icon.png.orig").getPath());
    assertTrue(
        MiniAapt.shouldIgnoreFile(
            ProjectFilesystemUtils.getPathForRelativePath(
                tmpFolder.getRoot(), Paths.get("res/drawable/another_icon.png.orig"))));

    ProjectFilesystemUtils.createParentDirs(
        tmpFolder.getRoot(), Paths.get("res/drawable/drawable-ldpi/.DS_Store"));
    Files.createFile(tmpFolder.getRoot().resolve("res/drawable/drawable-ldpi/.DS_Store").getPath());
    assertTrue(
        MiniAapt.shouldIgnoreFile(
            ProjectFilesystemUtils.getPathForRelativePath(
                tmpFolder.getRoot(), Paths.get("res/drawable/drawable-ldpi/.DS_Store"))));
  }

  @Test
  public void testDotSeparatedResourceNames()
      throws IOException, XPathExpressionException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<LinearLayout>",
                "<Button android:id=\"@+id/button1\" ",
                "android:text=\"@string/com.buckbuild.taskname\" />",
                "</LinearLayout>")
            .build();

    Path resource = Paths.get("resource.xml");
    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, resource);

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    ImmutableSet.Builder<RDotTxtEntry> references = ImmutableSet.builder();
    aapt.processXmlFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("resource.xml")),
        references);

    assertEquals(
        createTestingFakes(references.build()),
        ImmutableSet.<RDotTxtEntry>of(
            FakeEntry.create(IdType.INT, RType.STRING, "com_buckbuild_taskname")));
  }

  @Test
  public void ignoresValidPublicResourceType() throws IOException, ResourceParseException {
    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public name=\"some_resource_name\" type=\"string\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("public.xml")));
  }

  @Test
  public void invalidPublicResourceWithNoName() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage(
        "Error parsing file 'public.xml', expected a 'name' attribute in \n" + "'[public: null]'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public type=\"string\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("public.xml")));
  }

  @Test
  public void invalidPublicResourceWithEmptyName() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage(
        "Error parsing file 'public.xml', expected a 'name' attribute in \n" + "'[public: null]'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public name=\"\" type=\"string\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("public.xml")));
  }

  @Test
  public void invalidPublicResourceWithNoType() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage(
        "Error parsing file 'public.xml', expected a 'type' attribute in: \n" + "'[public: null]'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public name=\"some_resource_name\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("public.xml")));
  }

  @Test
  public void invalidPublicResourceWithEmptyType() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage(
        "Error parsing file 'public.xml', expected a 'type' attribute in: \n" + "'[public: null]'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public name=\"some_resource_name\" type=\"\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("public.xml")));
  }

  @Test
  public void invalidPublicResourceWithUnknownType() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage(
        "Invalid resource type 'unknown_type' in <public> resource 'some_resource_name' in file"
            + " 'public.xml'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public name=\"some_resource_name\" type=\"unknown_type\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(tmpFolder.getRoot(), lines, Paths.get("public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("public.xml")));
  }

  @Test
  public void validPublicResourceTypeInInvalidFile() throws IOException, ResourceParseException {
    thrown.expect(ResourceParseException.class);
    thrown.expectMessage(
        "<public> resource 'some_resource_name' must be declared in res/values/public.xml, but was"
            + " declared in 'non-public.xml'");

    ImmutableList<String> lines =
        ImmutableList.<String>builder()
            .add(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
                "<resources>",
                "<public name=\"some_resource_name\" type=\"string\"/>",
                "</resources>")
            .build();

    ProjectFilesystemUtils.writeLinesToPath(
        tmpFolder.getRoot(), lines, Paths.get("non-public.xml"));

    MiniAapt aapt = new MiniAapt(ImmutableSet.of());

    aapt.processValuesFile(
        ProjectFilesystemUtils.getPathForRelativePath(
            tmpFolder.getRoot(), Paths.get("non-public.xml")));
  }
}
