/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aapt

import java.nio.charset.StandardCharsets
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class ResourceSourceMapExecutableMainTest {
  @get:Rule val temporaryFolder = TemporaryFolder()

  @Test
  fun `resource entries preserve qualifiers and overlay priority`() {
    val firstResources = temporaryFolder.newFolder("first-resources").toPath()
    val secondResources = temporaryFolder.newFolder("second-resources").toPath()
    Files.createDirectories(firstResources.resolve("values"))
    Files.createDirectories(firstResources.resolve("layout"))
    Files.createDirectories(secondResources.resolve("values-fr"))
    Files.writeString(
        firstResources.resolve("values/strings.xml"),
        "<resources><string name=\"title\">First</string></resources>",
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        secondResources.resolve("values-fr/strings.xml"),
        "<resources><string name=\"title\">Second</string></resources>",
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        firstResources.resolve("layout/screen.xml"),
        """
        <LinearLayout xmlns:android="http://schemas.android.com/apk/res/android">
          <TextView android:id="@+id/title_view" />
        </LinearLayout>
        """
            .trimIndent(),
        StandardCharsets.UTF_8,
    )

    val resourceDirs = temporaryFolder.newFile("resource-dirs").toPath()
    Files.write(
        resourceDirs,
        listOf(
            "$firstResources\tfirst/BUCK",
            "   ",
            "$secondResources\tsecond/BUCK",
        ),
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("asset-dirs").toPath()
    val output = temporaryFolder.newFile("source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output)

    val expectedLines = listOf(
        "R\t0\tid\ttitle_view\t\tfirst/BUCK\tfirst/BUCK",
        "R\t0\tlayout\tscreen\t\tfirst/BUCK\tfirst/BUCK",
        "R\t0\tstring\ttitle\t\tfirst/BUCK\tfirst/BUCK",
        "R\t1\tstring\ttitle\tfr\tsecond/BUCK\tsecond/BUCK",
    )
    assertEquals(
        expectedLines.joinToString(separator = "\n", postfix = "\n"),
        Files.readString(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `generated resource entries preserve their original source`() {
    val root = temporaryFolder.newFolder("repo").toPath()
    val generatedResources = root.resolve("buck-out/generated/res")
    val sourceResources = root.resolve("fbandroid/example/res")
    Files.createDirectories(generatedResources.resolve("values"))
    Files.createDirectories(sourceResources.resolve("values"))
    val contents = "<resources><string name=\"title\">Title</string></resources>"
    Files.writeString(
        generatedResources.resolve("values/strings.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        sourceResources.resolve("values/strings.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    val resourceDirs = temporaryFolder.newFile("generated-resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "$generatedResources\t$sourceResources\tfbandroid/example/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("generated-asset-dirs").toPath()
    val output = temporaryFolder.newFile("generated-source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output, root)

    assertEquals(
        listOf(
            "R\t0\tstring\ttitle\t\tfbandroid/example/res/values/strings.xml\tfbandroid/example/BUCK",
        ),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `generated extension changes preserve their original source`() {
    val root = temporaryFolder.newFolder("extension-change-repo").toPath()
    val generatedResources = root.resolve("buck-out/generated/res")
    val sourceResources = root.resolve("fbandroid/example/res")
    Files.createDirectories(generatedResources.resolve("drawable"))
    Files.createDirectories(sourceResources.resolve("drawable"))
    Files.writeString(
        generatedResources.resolve("drawable/icon.webp"),
        "generated webp",
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        sourceResources.resolve("drawable/icon.png"),
        "source png",
        StandardCharsets.UTF_8,
    )
    val resourceDirs = temporaryFolder.newFile("extension-change-resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "$generatedResources\t$sourceResources\tfbandroid/example/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("extension-change-asset-dirs").toPath()
    val output = temporaryFolder.newFile("extension-change-source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output, root)

    assertEquals(
        listOf(
            "R\t0\tdrawable\ticon\t\tfbandroid/example/res/drawable/icon.png\tfbandroid/example/BUCK",
        ),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `generated qualifier variants preserve their most specific original source`() {
    val root = temporaryFolder.newFolder("qualifier-change-repo").toPath()
    val generatedResources = root.resolve("buck-out/generated/res")
    val sourceResources = root.resolve("fbandroid/example/res")
    Files.createDirectories(generatedResources.resolve("drawable-night-v21"))
    Files.createDirectories(sourceResources.resolve("drawable"))
    Files.createDirectories(sourceResources.resolve("drawable-night"))
    val contents = "<shape xmlns:android=\"http://schemas.android.com/apk/res/android\" />"
    Files.writeString(
        generatedResources.resolve("drawable-night-v21/icon.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        sourceResources.resolve("drawable/icon.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        sourceResources.resolve("drawable-night/icon.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    val resourceDirs = temporaryFolder.newFile("qualifier-change-resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "$generatedResources\t$sourceResources\tfbandroid/example/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("qualifier-change-asset-dirs").toPath()
    val output = temporaryFolder.newFile("qualifier-change-source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output, root)

    assertEquals(
        listOf(
            "R\t0\tdrawable\ticon\tnight-v21\tfbandroid/example/res/drawable-night/icon.xml\tfbandroid/example/BUCK",
        ),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `generated qualifier variants preserve equally specific original sources`() {
    val root = temporaryFolder.newFolder("ambiguous-qualifier-repo").toPath()
    val generatedResources = root.resolve("buck-out/generated/res")
    val sourceResources = root.resolve("fbandroid/example/res")
    Files.createDirectories(generatedResources.resolve("drawable-land-night-v21"))
    Files.createDirectories(sourceResources.resolve("drawable-land"))
    Files.createDirectories(sourceResources.resolve("drawable-night"))
    val contents = "<shape xmlns:android=\"http://schemas.android.com/apk/res/android\" />"
    Files.writeString(
        generatedResources.resolve("drawable-land-night-v21/icon.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        sourceResources.resolve("drawable-land/icon.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        sourceResources.resolve("drawable-night/icon.xml"),
        contents,
        StandardCharsets.UTF_8,
    )
    val resourceDirs = temporaryFolder.newFile("ambiguous-qualifier-resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "$generatedResources\t$sourceResources\tfbandroid/example/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("ambiguous-qualifier-asset-dirs").toPath()
    val output = temporaryFolder.newFile("ambiguous-qualifier-source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output, root)

    assertEquals(
        listOf(
            "R\t0\tdrawable\ticon\tland-night-v21\tfbandroid/example/res/drawable-land/icon.xml\tfbandroid/example/BUCK",
            "R\t0\tdrawable\ticon\tland-night-v21\tfbandroid/example/res/drawable-night/icon.xml\tfbandroid/example/BUCK",
        ),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `source indexing failures do not fall back to owning build files`() {
    val root = temporaryFolder.newFolder("source-index-failure-repo").toPath()
    val generatedResources = root.resolve("buck-out/generated/res")
    val missingSourceResources = root.resolve("fbandroid/example/res")
    Files.createDirectories(generatedResources.resolve("drawable"))
    Files.writeString(
        generatedResources.resolve("drawable/icon.webp"),
        "generated webp",
        StandardCharsets.UTF_8,
    )
    val resourceDirs = temporaryFolder.newFile("source-index-failure-resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "$generatedResources\t$missingSourceResources\tfbandroid/example/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("source-index-failure-asset-dirs").toPath()
    val output = temporaryFolder.newFile("source-index-failure-source-map").toPath()

    org.junit.Assert.assertThrows(java.io.IOException::class.java) {
      ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output, root)
    }
  }

  @Test
  fun `excluded values entries remain available for coverage attribution`() {
    val root = temporaryFolder.newFolder("repo").toPath()
    val resources = root.resolve("fbandroid/example/res")
    Files.createDirectories(resources.resolve("values"))
    Files.createDirectories(resources.resolve("values-it"))
    Files.writeString(
        resources.resolve("values/strings.xml"),
        "<resources><string name=\"title\">Title</string></resources>",
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        resources.resolve("values-it/strings.xml"),
        """
        <resources exclude-from-buck-resource-map="true">
          <string name="title">Titolo</string>
        </resources>
        """
            .trimIndent(),
        StandardCharsets.UTF_8,
    )
    val resourceDirs = temporaryFolder.newFile("excluded-resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "$resources\tfbandroid/example/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("excluded-asset-dirs").toPath()
    val output = temporaryFolder.newFile("excluded-source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output, root)

    assertEquals(
        listOf(
            "R\t0\tstring\ttitle\t\tfbandroid/example/res/values/strings.xml\tfbandroid/example/BUCK",
            "R\t0\tstring\ttitle\tit\tfbandroid/example/res/values-it/strings.xml\tfbandroid/example/BUCK",
        ),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `entry point writes an empty map when generation fails`() {
    val resourceDirs = temporaryFolder.newFile("resource-dirs").toPath()
    Files.writeString(
        resourceDirs,
        "${temporaryFolder.root.toPath().resolve("missing-resources")}\tmissing/BUCK",
        StandardCharsets.UTF_8,
    )
    val assetDirs = temporaryFolder.newFile("asset-dirs").toPath()
    val output = temporaryFolder.root.toPath().resolve("fallback/source-map")

    ResourceSourceMapExecutableMain.main(
        arrayOf(
            "--resource-dirs",
            resourceDirs.toString(),
            "--asset-dirs",
            assetDirs.toString(),
            "--output=$output",
        ),
    )

    assertEquals(emptyList<String>(), Files.readAllLines(output, StandardCharsets.UTF_8))
  }

  @Test
  fun `generation skips malformed files without discarding valid resources`() {
    val resources = temporaryFolder.newFolder("resources").toPath()
    Files.createDirectories(resources.resolve("values"))
    Files.createDirectories(resources.resolve("layout"))
    Files.writeString(
        resources.resolve("values/strings.xml"),
        "<resources><string name=\"title\">Title</string></resources>",
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        resources.resolve("values/broken.xml"),
        "<resources><string name=\"broken\">",
        StandardCharsets.UTF_8,
    )
    Files.writeString(
        resources.resolve("layout/broken.xml"),
        "<LinearLayout>",
        StandardCharsets.UTF_8,
    )

    val resourceDirs = temporaryFolder.newFile("resource-dirs").toPath()
    Files.writeString(resourceDirs, "$resources\tresources/BUCK", StandardCharsets.UTF_8)
    val assetDirs = temporaryFolder.newFile("asset-dirs").toPath()
    Files.writeString(
        assetDirs,
        "${temporaryFolder.root.toPath().resolve("missing-assets")}\tassets/BUCK",
        StandardCharsets.UTF_8,
    )
    val output = temporaryFolder.newFile("source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output)

    assertEquals(
        listOf("R\t0\tstring\ttitle\t\tresources/BUCK\tresources/BUCK"),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `asset entries preserve paths and overlay priority`() {
    val firstAssets = temporaryFolder.newFolder("first-assets").toPath()
    val secondAssets = temporaryFolder.newFolder("second-assets").toPath()
    Files.createDirectories(firstAssets.resolve("images"))
    Files.createDirectories(secondAssets.resolve("images"))
    val assetPath = "images/icon.webp"
    Files.writeString(firstAssets.resolve(assetPath), "first", StandardCharsets.UTF_8)
    Files.writeString(secondAssets.resolve(assetPath), "second", StandardCharsets.UTF_8)

    val resourceDirs = temporaryFolder.newFile("resource-dirs").toPath()
    val assetDirs = temporaryFolder.newFile("asset-dirs").toPath()
    Files.write(
        assetDirs,
        listOf(
            "$firstAssets\tfirst/BUCK",
            "$secondAssets\tsecond/BUCK",
        ),
        StandardCharsets.UTF_8,
    )
    val output = temporaryFolder.newFile("source-map").toPath()

    ResourceSourceMapExecutableMain.generate(resourceDirs, assetDirs, output)

    assertEquals(
        listOf(
            assetSourceMapRow(0, assetPath, "first/BUCK", "first/BUCK"),
            assetSourceMapRow(1, assetPath, "second/BUCK", "second/BUCK"),
        ),
        Files.readAllLines(output, StandardCharsets.UTF_8),
    )
  }

  @Test
  fun `asset row serialization preserves delimiter characters`() {
    val assetRow =
        ResourceSourceMapFormat.AssetRow(
            2,
            "images/icon\tvariant.webp",
            "asset\nsource",
            "asset\tBUCK",
        )

    assertEquals(assetRow, ResourceSourceMapFormat.parse(assetRow.serialize()))
  }

  private fun assetSourceMapRow(
      priority: Int,
      path: String,
      source: String,
      ownerBuildFile: String,
  ): String = listOf(
      "A",
      priority.toString(),
      encodeField(path),
      encodeField(source),
      encodeField(ownerBuildFile),
  )
      .joinToString("\t")

  private fun encodeField(value: String): String =
      java.util.Base64.getUrlEncoder()
          .withoutPadding()
          .encodeToString(value.toByteArray(StandardCharsets.UTF_8))
}
