/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:SuppressWarnings("PackageLocationMismatch")

package com.facebook

import org.jetbrains.kotlin.metadata.ProtoBuf
import org.jetbrains.kotlin.metadata.deserialization.Flags
import org.jetbrains.kotlin.metadata.deserialization.TypeTable
import org.jetbrains.kotlin.metadata.jvm.deserialization.JvmProtoBufUtil
import org.jetbrains.org.objectweb.asm.AnnotationVisitor
import org.jetbrains.org.objectweb.asm.ClassReader
import org.jetbrains.org.objectweb.asm.ClassVisitor
import org.jetbrains.org.objectweb.asm.Opcodes

fun enumConsumerMetadataProblems(bytes: ByteArray): List<String> {
  val reader = ClassReader(bytes)
  if (reader.access and Opcodes.ACC_ENUM == 0) return emptyList()
  var hasMetadata = false
  var kind = 1
  val data = mutableListOf<String>()
  val strings = mutableListOf<String>()
  reader.accept(
      object : ClassVisitor(Opcodes.ASM9) {
        override fun visitAnnotation(descriptor: String, visible: Boolean): AnnotationVisitor? {
          if (descriptor != "Lkotlin/Metadata;") return null
          hasMetadata = true
          return object : AnnotationVisitor(Opcodes.ASM9) {
            override fun visit(name: String, value: Any) {
              if (name == "k") kind = value as Int
            }

            override fun visitArray(name: String): AnnotationVisitor? {
              val destination =
                  when (name) {
                    "d1" -> data
                    "d2" -> strings
                    else -> return null
                  }
              return object : AnnotationVisitor(Opcodes.ASM9) {
                override fun visit(name: String?, value: Any) {
                  destination.add(value as String)
                }
              }
            }
          }
        }
      },
      ClassReader.SKIP_CODE or ClassReader.SKIP_DEBUG or ClassReader.SKIP_FRAMES,
  )
  if (!hasMetadata || kind != 1) return emptyList()
  return try {
    val (resolver, metadata) =
        JvmProtoBufUtil.readClassDataFrom(data.toTypedArray(), strings.toTypedArray())
    if (Flags.CLASS_KIND.get(metadata.flags) != ProtoBuf.Class.Kind.ENUM_CLASS) return emptyList()
    val signatures = mutableSetOf<String>()
    val problems = linkedSetOf<String>()
    for (function in metadata.functionList) {
      val visibility = Flags.VISIBILITY.get(function.flags)
      if (visibility != ProtoBuf.Visibility.PUBLIC && visibility != ProtoBuf.Visibility.PROTECTED)
          continue
      val signature =
          JvmProtoBufUtil.getJvmMethodSignature(function, resolver, TypeTable(metadata.typeTable))
      if (signature == null) {
        problems.add(
            "`${reader.className}` has an uninspectable public Kotlin metadata method `${resolver.getString(function.name)}`.",
        )
      } else if (!signatures.add(signature.asString())) {
        problems.add(
            "`${reader.className}` has duplicate public Kotlin metadata method `${signature.asString()}`; Kotlin consumers cannot resolve the member.",
        )
      }
    }
    problems.toList()
  } catch (failure: Exception) {
    listOf(
        "`${reader.className}` has uninspectable enum Kotlin metadata: ${failure.javaClass.simpleName}.",
    )
  }
}
