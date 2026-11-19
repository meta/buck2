/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.compilerplugins.kosabi.common.stub.render

import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KPropertyStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import java.io.File
import java.io.FileOutputStream
import kotlin.metadata.KmClassifier
import kotlin.metadata.KmFunction
import kotlin.metadata.KmPackage
import kotlin.metadata.KmType
import kotlin.metadata.KmValueParameter
import kotlin.metadata.Visibility
import kotlin.metadata.isNullable
import kotlin.metadata.jvm.JvmMetadataVersion
import kotlin.metadata.jvm.JvmMethodSignature
import kotlin.metadata.jvm.KotlinClassMetadata
import kotlin.metadata.jvm.signature
import kotlin.metadata.visibility
import org.jetbrains.org.objectweb.asm.AnnotationVisitor
import org.jetbrains.org.objectweb.asm.ClassVisitor
import org.jetbrains.org.objectweb.asm.ClassWriter
import org.jetbrains.org.objectweb.asm.Opcodes

// Kotlin metadata classifier names use '/' separated internal names.
private const val UNIT_CLASSIFIER = "kotlin/Unit"
private const val ANY_CLASSIFIER = "kotlin/Any"

object StubBytecodeRender {

  fun KStub.supportBytecode(): Boolean {
    // For KStubs that should be treated as real (e.g JavaKStub for KSP output, or IG JsonHelper
    // type), we don't want to generate bytecode, but should
    // keep
    // passing them as KTFile for compilation, as these classes needed to be part as final ABI
    // output
    if (treatAsReal) {
      return false
    }

    return true
  }

  fun KPropertyStub.modifier(): Int {
    var modifier = Opcodes.ACC_PUBLIC
    if (static) {
      modifier = modifier or Opcodes.ACC_STATIC
    }
    return modifier
  }

  fun KStub.accessFlags(): Int {
    // A Kotlin file facade (the class holding top level declarations) is always a final class.
    if (type == KStub.Type.TOP_LEVEL_DECLARATION) {
      return Opcodes.ACC_PUBLIC or Opcodes.ACC_FINAL or Opcodes.ACC_SUPER
    }

    var flag =
        when (type) {
          // ACC_ANNOTATION alone is not a valid annotation type: JVMS 4.1 requires ACC_INTERFACE
          // (and therefore ACC_ABSTRACT) alongside it, and the class must implement
          // java/lang/annotation/Annotation. A class carrying only ACC_ANNOTATION does not resolve
          // in an annotation position, so the use site keeps failing even once the stub exists.
          KStub.Type.ANNOTATION ->
              Opcodes.ACC_ANNOTATION or Opcodes.ACC_INTERFACE or Opcodes.ACC_ABSTRACT
          KStub.Type.INTERFACE -> Opcodes.ACC_INTERFACE
          else -> Opcodes.ACC_SUPER
        }

    if (abstract) {
      flag = flag or Opcodes.ACC_ABSTRACT
    }

    flag = flag or Opcodes.ACC_PUBLIC

    return flag
  }

  // The internal name of a class is its FQN,
  // where '.' are replaced by '/') for the package name part, and $ are use to concat the name of
  // inner classes
  // e.g com.foo.bar.Outer.Inner -> com/foo/bar/Outer$Inner
  private fun KStub.internalName(): String {

    var internalName = "$pkg.$name".replace(".", "/")

    var currentSlash = internalName.lastIndexOf('/')
    while (currentSlash > 0) {
      val previousSlash = internalName.lastIndexOf('/', currentSlash - 1)
      internalName =
          if (previousSlash > 0 && Character.isUpperCase(internalName[previousSlash + 1])) {
            ("${internalName.substring(0, currentSlash)}$${internalName.substring(currentSlash + 1)}")
          } else {
            break
          }
      currentSlash = previousSlash
    }
    return internalName
  }

  // Where should we put the class file, respecting the package name hierarchy
  // e.g com/foor/bar/Outer$Inner -> File(root, "com/foo/bar")
  private fun KStub.classFilePath(root: File): File {
    return if (pkg == null) root
    else File(root, internalName().substring(0, internalName().lastIndexOf('/')))
  }

  // The name of the class file output
  // e.g com/foor/bar/Outer$Inner -> "Outer$Inner.class"
  private fun KStub.classFileName(): String {
    return "${internalName().split("/").last()}.class"
  }

  private fun KStub.superClass(): String {
    return "java/lang/Object"
  }

  private fun KStub.interfaces(): Array<String>? {
    return if (type == KStub.Type.ANNOTATION) arrayOf("java/lang/annotation/Annotation") else null
  }

  fun KStub.signature(): String? {
    if (genericTypes == 0) return null
    return "<${(0 until genericTypes).map { "T$it:Ljava/lang/Object;" }.joinToString("")}>"
  }

  private fun KStub.renderInnerClass(visitor: ClassVisitor, dir: File) {
    if (innerStubs.isEmpty()) return

    val containerClassInternalName = internalName()
    innerStubs.forEach { innerStub ->
      renderKStubBytecode(innerStub, dir)
      visitor.visitInnerClass(
          innerStub.internalName(),
          containerClassInternalName,
          innerStub.name,
          innerStub.accessFlags() or Opcodes.ACC_STATIC,
      )
    }
  }

  private fun KStub.renderConstructor(visitor: ClassVisitor) {
    if (ctor == null) return
    // An annotation type is an interface, and an interface has no <init>: emitting one makes the
    // class file unverifiable, which would trade the unresolved use site for a worse failure.
    if (type == KStub.Type.ANNOTATION) return
    visitor.visitMethod(
        Opcodes.ACC_PUBLIC,
        "<init>",
        "(${"Ljava/lang/Object;".repeat(ctor?.namedArgs?.size ?: 0)})V",
        null,
        null,
    )
  }

  // A top level function resolved into the single source of truth shared by the emitted bytecode
  // method and its `@kotlin/Metadata` entry: the JVM `signature` (name + descriptor) MUST be
  // identical in both, otherwise the Kotlin compiler will not resolve the top level declaration
  // against the emitted method. Deriving both from this one value keeps them in lockstep.
  private class TopLevelFun(val paramNames: List<String>, val signature: JvmMethodSignature)

  // Top level function stubs are always generated as `(Any?, ...) -> Unit` (see
  // ClassLevelFunctionStubsGenerator), so their JVM descriptor is a fixed shape. Two stubs sharing
  // the same name and arity would collapse into the same JVM method, so we deduplicate to avoid
  // emitting an invalid class file with duplicate members.
  private fun KStub.topLevelFuns(): List<TopLevelFun> =
      funStubs
          .distinctBy { it.name to it.namedArgs.size }
          .map { funStub ->
            val descriptor = "(${"Ljava/lang/Object;".repeat(funStub.namedArgs.size)})V"
            TopLevelFun(
                paramNames = funStub.namedArgs.map { (paramName, _) -> paramName },
                signature = JvmMethodSignature(funStub.name, descriptor),
            )
          }

  private fun renderTopLevelFunctions(visitor: ClassVisitor, topLevelFuns: List<TopLevelFun>) {
    topLevelFuns.forEach { fn ->
      val methodVisitor =
          visitor.visitMethod(
              Opcodes.ACC_PUBLIC or Opcodes.ACC_STATIC or Opcodes.ACC_FINAL,
              fn.signature.name,
              fn.signature.descriptor,
              null,
              null,
          )
      methodVisitor?.apply {
        visitCode()
        visitInsn(Opcodes.RETURN)
        // Real values are recomputed by COMPUTE_MAXS.
        visitMaxs(0, 0)
        visitEnd()
      }
    }
  }

  private fun renderFileFacadeMetadata(visitor: ClassVisitor, topLevelFuns: List<TopLevelFun>) {
    val kmPackage = KmPackage()
    topLevelFuns.forEach { fn ->
      val kmFunction =
          KmFunction(fn.signature.name).apply {
            visibility = Visibility.PUBLIC
            returnType = KmType().apply { classifier = KmClassifier.Class(UNIT_CLASSIFIER) }
            fn.paramNames.forEach { paramName ->
              valueParameters.add(
                  KmValueParameter(paramName).apply {
                    type =
                        KmType().apply {
                          classifier = KmClassifier.Class(ANY_CLASSIFIER)
                          isNullable = true
                        }
                  },
              )
            }
            signature = fn.signature
          }
      kmPackage.functions.add(kmFunction)
    }

    val metadata =
        KotlinClassMetadata.FileFacade(kmPackage, JvmMetadataVersion.LATEST_STABLE_SUPPORTED, 0)
            .write()

    visitor.visitAnnotation("Lkotlin/Metadata;", true)?.writeMetadata(metadata)
  }

  private fun AnnotationVisitor.writeMetadata(metadata: Metadata) {
    visit("k", metadata.kind)
    visit("mv", metadata.metadataVersion)
    visitArray("d1")?.apply {
      metadata.data1.forEach { visit(null, it) }
      visitEnd()
    }
    visitArray("d2")?.apply {
      metadata.data2.forEach { visit(null, it) }
      visitEnd()
    }
    if (metadata.extraString.isNotEmpty()) visit("xs", metadata.extraString)
    if (metadata.packageName.isNotEmpty()) visit("pn", metadata.packageName)
    visit("xi", metadata.extraInt)
    visitEnd()
  }

  fun renderKStubBytecode(stub: KStub?, outputRoot: File) {
    if (stub == null) return

    val classWriter = ClassWriter(ClassWriter.COMPUTE_MAXS)

    classWriter.visit(
        Opcodes.ASM9,
        stub.accessFlags(),
        stub.internalName(),
        stub.signature(),
        stub.superClass(),
        stub.interfaces(),
    )

    stub.apply {
      if (type == KStub.Type.TOP_LEVEL_DECLARATION) {
        // A file facade has no constructor; it exposes its top level declarations as static
        // methods and carries a @kotlin/Metadata annotation so the Kotlin compiler resolves them
        // as top level declarations rather than plain Java statics. The bytecode methods and the
        // metadata entries must be computed from the same list, so we resolve it once here.
        val topLevelFuns = topLevelFuns()
        renderFileFacadeMetadata(classWriter, topLevelFuns)
        renderTopLevelFunctions(classWriter, topLevelFuns)
      } else {
        renderInnerClass(classWriter, outputRoot)
        renderConstructor(classWriter)
      }
    }

    classWriter.visitEnd()

    val bytecode = classWriter.toByteArray()

    val folder = stub.classFilePath(outputRoot)
    if (!folder.exists()) folder.mkdirs()

    val classFile = File(folder, stub.classFileName())
    FileOutputStream(classFile).apply {
      write(bytecode)
      close()
    }
  }
}
