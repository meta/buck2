/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.jar.Attributes;
import java.util.jar.Manifest;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;
import org.junit.runners.Parameterized.Parameter;
import org.junit.runners.Parameterized.Parameters;

/** Parameterized tests for {@link JarManifestMerger} and {@link ManifestAttributeMerger}. */
@RunWith(Parameterized.class)
public class JarManifestMergerTest {

  private static final String TRUE = Boolean.TRUE.toString();
  private static final String FALSE = Boolean.FALSE.toString();

  private static final String MULTI_RELEASE = "Multi-Release";
  private static final String MANIFEST_VERSION = "Manifest-Version";
  private static final String CREATED_BY = "Created-By";
  private static final String SEALED = "Sealed";
  private static final String IMPL_VERSION = "Implementation-Version";
  private static final String IMPL_TITLE = "Implementation-Title";

  private static final String PKG_EXAMPLE = "com/example/";
  private static final String PKG_PACKAGE1 = "com/package1/";
  private static final String PKG_PACKAGE2 = "com/package2/";
  private static final String PKG_OTHER = "org/other/";
  private static final String PKG_INTO = "com/into/";
  private static final String PKG_FROM = "com/from/";
  private static final String PKG_LIB1 = "com/lib1/";
  private static final String PKG_LIB2 = "com/lib2/";

  private static final String VERSION_1_5 = "1.5";
  private static final String VERSION_2_0 = "2.0";
  private static final String VERSION_3_0 = "3.0";

  private static final String TOOL_A = "Tool A";
  private static final String TOOL_B = "Tool B";
  private static final String TOOL_ORIGINAL = "Original Tool";
  private static final String TOOL_BUILD = "Build Tool";

  private static final String LIB_MY = "MyLib";
  private static final String LIB_BASE = "BaseLib";

  @Parameters(name = "{0}")
  public static List<TestData> data() {
    return List.of(
        // Empty manifests
        new TestData("empty manifests")
            .initial(ManifestBuilder.empty())
            .merge(ManifestBuilder.empty())
            .expect(ManifestBuilder.empty()),

        // Multi-Release: true wins when true comes first
        new TestData("Multi-Release true first, false second")
            .initial(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .expect(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest()),

        // Multi-Release: true wins when false comes first
        new TestData("Multi-Release false first, true second")
            .initial(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .expect(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest()),

        // Multi-Release: both true stays true
        new TestData("Multi-Release both true")
            .initial(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .expect(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest()),

        // Multi-Release: both false stays false
        new TestData("Multi-Release both false")
            .initial(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .expect(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest()),

        // Multi-Release: case insensitive TRUE
        new TestData("Multi-Release case insensitive TRUE")
            .initial(
                ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE.toUpperCase()).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .expect(
                ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE.toUpperCase()).manifest()),

        // Multi-Release: target empty, source has true
        new TestData("Multi-Release target empty, source true")
            .initial(ManifestBuilder.empty())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .expect(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest()),

        // Multi-Release: multiple merges, true preserved
        new TestData("Multi-Release multiple merges true preserved")
            .initial(ManifestBuilder.empty())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, FALSE).manifest())
            .expect(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest()),

        // Default merger: last value wins
        new TestData("default merger last value wins")
            .initial(ManifestBuilder.builder().mainAttr(CREATED_BY, TOOL_A).manifest())
            .merge(ManifestBuilder.builder().mainAttr(CREATED_BY, TOOL_B).manifest())
            .expect(ManifestBuilder.builder().mainAttr(CREATED_BY, TOOL_B).manifest()),

        // Preserves existing attributes not in source
        new TestData("preserves existing attributes")
            .initial(ManifestBuilder.builder().mainAttr(CREATED_BY, TOOL_ORIGINAL).manifest())
            .merge(ManifestBuilder.builder().mainAttr(MULTI_RELEASE, TRUE).manifest())
            .expect(
                ManifestBuilder.builder()
                    .mainAttr(CREATED_BY, TOOL_ORIGINAL)
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .manifest()),

        // Multiple main attributes merged
        new TestData("multiple main attributes merged")
            .initial(
                ManifestBuilder.builder()
                    .mainAttr(CREATED_BY, TOOL_A)
                    .mainAttr(IMPL_TITLE, LIB_MY)
                    .manifest())
            .merge(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .mainAttr(IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest())
            .expect(
                ManifestBuilder.builder()
                    .mainAttr(CREATED_BY, TOOL_A)
                    .mainAttr(IMPL_TITLE, LIB_MY)
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .mainAttr(IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest()),

        // Named entry: new entry added
        new TestData("named entry new entry added")
            .initial(ManifestBuilder.empty())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, SEALED, TRUE).manifest())
            .expect(ManifestBuilder.builder().attr(PKG_EXAMPLE, SEALED, TRUE).manifest()),

        // Named entry: existing entry merged
        new TestData("named entry existing entry merged")
            .initial(
                ManifestBuilder.builder()
                    .attr(PKG_EXAMPLE, SEALED, FALSE)
                    .attr(PKG_EXAMPLE, IMPL_TITLE, LIB_MY)
                    .manifest())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, SEALED, TRUE).manifest())
            .expect(
                ManifestBuilder.builder()
                    .attr(PKG_EXAMPLE, SEALED, TRUE)
                    .attr(PKG_EXAMPLE, IMPL_TITLE, LIB_MY)
                    .manifest()),

        // Named entry: Multi-Release true wins
        new TestData("named entry Multi-Release true wins")
            .initial(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, TRUE).manifest())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, FALSE).manifest())
            .expect(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, TRUE).manifest()),

        // Named entry: Multi-Release false then true
        new TestData("named entry Multi-Release false then true")
            .initial(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, FALSE).manifest())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, TRUE).manifest())
            .expect(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, TRUE).manifest()),

        // Named entry: multiple entries from source
        new TestData("named entry multiple entries from source")
            .initial(ManifestBuilder.empty())
            .merge(
                ManifestBuilder.builder()
                    .attr(PKG_PACKAGE1, SEALED, TRUE)
                    .attr(PKG_PACKAGE2, SEALED, FALSE)
                    .attr(PKG_OTHER, IMPL_VERSION, VERSION_3_0)
                    .manifest())
            .expect(
                ManifestBuilder.builder()
                    .attr(PKG_PACKAGE1, SEALED, TRUE)
                    .attr(PKG_PACKAGE2, SEALED, FALSE)
                    .attr(PKG_OTHER, IMPL_VERSION, VERSION_3_0)
                    .manifest()),

        // Named entry: disjoint entries preserved
        new TestData("named entry disjoint entries preserved")
            .initial(ManifestBuilder.builder().attr(PKG_INTO, SEALED, TRUE).manifest())
            .merge(ManifestBuilder.builder().attr(PKG_FROM, SEALED, TRUE).manifest())
            .expect(
                ManifestBuilder.builder()
                    .attr(PKG_INTO, SEALED, TRUE)
                    .attr(PKG_FROM, SEALED, TRUE)
                    .manifest()),

        // Named entry: multiple merges preserve true
        new TestData("named entry multiple merges preserve true")
            .initial(ManifestBuilder.empty())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, FALSE).manifest())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, TRUE).manifest())
            .merge(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, FALSE).manifest())
            .expect(ManifestBuilder.builder().attr(PKG_EXAMPLE, MULTI_RELEASE, TRUE).manifest()),

        // Both main and named entries merged
        new TestData("both main and named entries merged")
            .initial(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, FALSE)
                    .attr(PKG_EXAMPLE, SEALED, FALSE)
                    .manifest())
            .merge(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .mainAttr(CREATED_BY, TOOL_BUILD)
                    .attr(PKG_EXAMPLE, SEALED, TRUE)
                    .attr(PKG_EXAMPLE, IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest())
            .expect(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .mainAttr(CREATED_BY, TOOL_BUILD)
                    .attr(PKG_EXAMPLE, SEALED, TRUE)
                    .attr(PKG_EXAMPLE, IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest()),

        // Named entry: default merger last value wins
        new TestData("named entry default merger last value wins")
            .initial(
                ManifestBuilder.builder()
                    .attr(PKG_EXAMPLE, IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest())
            .merge(
                ManifestBuilder.builder().attr(PKG_EXAMPLE, IMPL_VERSION, VERSION_2_0).manifest())
            .expect(
                ManifestBuilder.builder().attr(PKG_EXAMPLE, IMPL_VERSION, VERSION_2_0).manifest()),

        // Named entry: preserves non-overlapping attributes
        new TestData("named entry preserves non-overlapping attributes")
            .initial(
                ManifestBuilder.builder()
                    .attr(PKG_EXAMPLE, SEALED, TRUE)
                    .attr(PKG_EXAMPLE, IMPL_TITLE, LIB_MY)
                    .manifest())
            .merge(
                ManifestBuilder.builder()
                    .attr(PKG_EXAMPLE, IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest())
            .expect(
                ManifestBuilder.builder()
                    .attr(PKG_EXAMPLE, SEALED, TRUE)
                    .attr(PKG_EXAMPLE, IMPL_TITLE, LIB_MY)
                    .attr(PKG_EXAMPLE, IMPL_VERSION, ManifestBuilder.VERSION_1_0)
                    .manifest()),

        // Complex: multiple jars with various attributes
        new TestData("complex multiple jars with various attributes")
            .initial(ManifestBuilder.builder().mainAttr(IMPL_TITLE, LIB_BASE).manifest())
            .merge(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, FALSE)
                    .attr(PKG_LIB1, SEALED, TRUE)
                    .manifest())
            .merge(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .mainAttr(IMPL_VERSION, VERSION_2_0)
                    .attr(PKG_LIB2, SEALED, TRUE)
                    .manifest())
            .merge(
                ManifestBuilder.builder()
                    .mainAttr(MULTI_RELEASE, FALSE)
                    .attr(PKG_LIB1, IMPL_VERSION, VERSION_1_5)
                    .manifest())
            .expect(
                ManifestBuilder.builder()
                    .mainAttr(IMPL_TITLE, LIB_BASE)
                    .mainAttr(MULTI_RELEASE, TRUE)
                    .mainAttr(IMPL_VERSION, VERSION_2_0)
                    .attr(PKG_LIB1, SEALED, TRUE)
                    .attr(PKG_LIB1, IMPL_VERSION, VERSION_1_5)
                    .attr(PKG_LIB2, SEALED, TRUE)
                    .manifest()));
  }

  @Parameter public TestData testData;

  @Test
  public void testManifestMerge() {
    final Manifest result = testData.getInitialManifest();
    final JarManifestMerger merger = JarManifestMerger.get();

    for (Manifest manifestToMerge : testData.getManifestsToMerge()) {
      merger.merge(result, manifestToMerge);
    }

    testData.assertExpectations(result);
  }

  /** Test data containing manifests and expected results. */
  public static class TestData {
    private final String description;
    private final List<Manifest> manifestsToMerge = new ArrayList<>();
    private Manifest initialManifest;
    private Manifest expectedManifest;

    public TestData(String description) {
      this.description = description;
    }

    public TestData initial(Manifest manifest) {
      this.initialManifest = manifest;
      return this;
    }

    public TestData merge(Manifest manifest) {
      manifestsToMerge.add(manifest);
      return this;
    }

    public TestData expect(Manifest manifest) {
      this.expectedManifest = manifest;
      return this;
    }

    public Manifest getInitialManifest() {
      return initialManifest;
    }

    public List<Manifest> getManifestsToMerge() {
      return manifestsToMerge;
    }

    public void assertExpectations(Manifest result) {
      for (Object key : expectedManifest.getMainAttributes().keySet()) {
        assertEquals(
            "Main attribute " + key,
            expectedManifest.getMainAttributes().get(key),
            result.getMainAttributes().get(key));
      }
      for (Map.Entry<String, Attributes> entry : expectedManifest.getEntries().entrySet()) {
        final Attributes expectedAttrs = entry.getValue();
        final Attributes actualAttrs = result.getAttributes(entry.getKey());
        for (Object attrKey : expectedAttrs.keySet()) {
          assertEquals(
              "Named entry " + entry.getKey() + " attribute " + attrKey,
              expectedAttrs.get(attrKey),
              actualAttrs.get(attrKey));
        }
      }
    }

    @Override
    public String toString() {
      return description;
    }
  }

  /** Builder for creating {@link Manifest} objects with a fluent API. */
  public static class ManifestBuilder {

    public static final String VERSION_1_0 = "1.0";

    private final Manifest manifest;

    private ManifestBuilder() {
      this.manifest = new Manifest();
      this.manifest.getMainAttributes().putValue(MANIFEST_VERSION, VERSION_1_0);
    }

    public static ManifestBuilder builder() {
      return new ManifestBuilder();
    }

    public static Manifest empty() {
      return builder().manifest();
    }

    public ManifestBuilder mainAttr(String name, String value) {
      manifest.getMainAttributes().putValue(name, value);
      return this;
    }

    public ManifestBuilder attr(String entryName, String attrName, String attrValue) {
      manifest
          .getEntries()
          .computeIfAbsent(entryName, k -> new Attributes())
          .putValue(attrName, attrValue);
      return this;
    }

    public Manifest manifest() {
      return manifest;
    }
  }
}
