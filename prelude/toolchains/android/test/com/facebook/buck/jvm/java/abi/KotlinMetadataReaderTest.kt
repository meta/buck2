/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi

import com.facebook.buck.jvm.java.abi.kotlin.KotlinMetadataReader
import com.facebook.buck.jvm.kotlin.testutil.compiler.KotlinTestCompiler
import java.util.jar.JarFile
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.objectweb.asm.ClassReader
import org.objectweb.asm.Opcodes
import org.objectweb.asm.tree.AnnotationNode
import org.objectweb.asm.tree.ClassNode

class KotlinMetadataReaderTest {

  @get:Rule val temp = TemporaryFolder()

  @Test
  fun isFilePrivateClass_returnsTrueForPrivateTopLevelClass() {
    val annotation = compileAndGetKotlinMetadata(
        "A.kt",
        arrayOf(
            "package com.example.buck",
            "private class PrivateHelper { fun helper(): Int = 42 }",
        ),
        "com/example/buck/PrivateHelper.class",
    )

    val metadata = KotlinMetadataReader.readMetadata(annotation)
    assertTrue(
        "File-private Kotlin class should be detected as private",
        KotlinMetadataReader.isFilePrivateClass(metadata),
    )
  }

  @Test
  fun isFilePrivateClass_returnsTrueForPrivateObjectDeclaration() {
    val annotation = compileAndGetKotlinMetadata(
        "A.kt",
        arrayOf(
            "package com.example.buck",
            "private object PrivateSingleton { fun compute(): Int = 42 }",
        ),
        "com/example/buck/PrivateSingleton.class",
    )

    val metadata = KotlinMetadataReader.readMetadata(annotation)
    assertTrue(
        "File-private Kotlin object should be detected as private",
        KotlinMetadataReader.isFilePrivateClass(metadata),
    )
  }

  @Test
  fun isFilePrivateClass_returnsFalseForPublicClass() {
    val annotation = compileAndGetKotlinMetadata(
        "A.kt",
        arrayOf("package com.example.buck", "class PublicClass"),
        "com/example/buck/PublicClass.class",
    )

    assertFalse(
        "Public Kotlin class should not be detected as file-private",
        KotlinMetadataReader.isFilePrivateClass(KotlinMetadataReader.readMetadata(annotation)),
    )
  }

  @Test
  fun isFilePrivateClass_returnsFalseForInternalClass() {
    val annotation = compileAndGetKotlinMetadata(
        "A.kt",
        arrayOf("package com.example.buck", "internal class InternalClass"),
        "com/example/buck/InternalClass.class",
    )

    assertFalse(
        "Internal Kotlin class should not be detected as file-private",
        KotlinMetadataReader.isFilePrivateClass(KotlinMetadataReader.readMetadata(annotation)),
    )
  }

  @Test
  fun isFilePrivateClass_returnsFalseForOpenClass() {
    val annotation = compileAndGetKotlinMetadata(
        "A.kt",
        arrayOf("package com.example.buck", "open class OpenClass"),
        "com/example/buck/OpenClass.class",
    )

    assertFalse(
        "Open Kotlin class should not be detected as file-private",
        KotlinMetadataReader.isFilePrivateClass(KotlinMetadataReader.readMetadata(annotation)),
    )
  }

  @Test
  fun isFilePrivateClass_returnsFalseForPrivateInnerClass() {
    // A private inner class is NOT a file-private class — it's private within
    // its enclosing class. isFilePrivateClass should return false for inner classes
    // because they are handled separately by the ACC_PRIVATE bytecode check.
    val annotation = compileAndGetKotlinMetadata(
        "A.kt",
        arrayOf(
            "package com.example.buck",
            "class Outer {",
            "  private class Inner { fun innerMethod(): Int = 0 }",
            "}",
        ),
        "com/example/buck/Outer\$Inner.class",
    )

    assertFalse(
        "Private inner class should NOT be detected as file-private",
        KotlinMetadataReader.isFilePrivateClass(KotlinMetadataReader.readMetadata(annotation)),
    )
  }

  /**
   * Compiles a Kotlin source file, extracts the @kotlin.Metadata annotation from the specified
   * class entry in the compiled JAR.
   */
  private fun compileAndGetKotlinMetadata(
      fileName: String,
      sourceLines: Array<String>,
      classEntryName: String,
  ): AnnotationNode {
    KotlinTestCompiler().use { compiler ->
      compiler.init()
      compiler.addSourceFileContents(fileName, *sourceLines)
      compiler.compile()

      val jarPath = temp.newFolder().toPath().resolve("output.jar")
      compiler.classes.createJar(jarPath, false)

      JarFile(jarPath.toFile()).use { jarFile ->
        val entry =
            jarFile.getJarEntry(classEntryName)
                ?: throw AssertionError(
                    "Expected class entry $classEntryName not found in compiled JAR",
                )

        val classNode = ClassNode(Opcodes.ASM9)
        val reader = ClassReader(jarFile.getInputStream(entry))
        reader.accept(classNode, ClassReader.SKIP_CODE)

        val annotations =
            classNode.visibleAnnotations
                ?: throw AssertionError("No visible annotations found on $classEntryName")

        return annotations.firstOrNull { it.desc == "Lkotlin/Metadata;" }
            ?: throw AssertionError("@kotlin.Metadata annotation not found on $classEntryName")
      }
    }
  }
}
