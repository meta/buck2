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

import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.config.CompilerConfigurationKey
import org.jetbrains.kotlin.config.KotlinCompilerVersion
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.name.FqName

internal class ParcelizeAbiEmulationExtension
private constructor(
    private val parcelizeAnnotations: List<FqName>,
    private val polymorphicSealedAnnotations: List<FqName>,
) : IrGenerationExtension {

  override fun generate(moduleFragment: IrModuleFragment, pluginContext: IrPluginContext) {
    ParcelableCreatorEmulator(pluginContext, parcelizeAnnotations, polymorphicSealedAnnotations)
        .transform(moduleFragment)
  }

  companion object {
    private val SUPPORTED_IR_EXTENSION_CLASS_NAMES = setOf(
        "org.jetbrains.kotlin.parcelize.ParcelizeFirIrGeneratorExtension",
        "org.jetbrains.kotlin.parcelize.ParcelizeIrGeneratorExtension",
    )

    fun createIfRegistered(
        project: Project,
        configuration: CompilerConfiguration,
    ): ParcelizeAbiEmulationExtension? {
      val extension =
          registeredIrGenerationExtensionsCompat(project, configuration).firstOrNull {
            it.javaClass.name in SUPPORTED_IR_EXTENSION_CLASS_NAMES
          } ?: return null
      return ParcelizeAbiEmulationExtension(
          configuredAnnotations(extension, configuration),
          polymorphicSealedAnnotations(extension),
      )
    }

    // Kotlin 2.5 backends declare these names and generate a polymorphic sealed root's CREATOR;
    // earlier backends have neither.
    private fun polymorphicSealedAnnotations(extension: IrGenerationExtension): List<FqName> {
      val names =
          try {
            val parcelizeNames =
                extension.javaClass.classLoader.loadClass(
                    "org.jetbrains.kotlin.parcelize.ParcelizeNames",
                )
            val getter =
                parcelizeNames.methods.singleOrNull {
                  it.name == "getPOLYMORPHIC_SEALED_CLASS_FQ_NAMES" && it.parameterCount == 0
                }
            if (getter == null) {
              check(!hasPolymorphicSealedRoots(KotlinCompilerVersion.VERSION)) {
                "Cannot export Parcelize CREATOR: the selected Parcelize backend for Kotlin " +
                    "${KotlinCompilerVersion.VERSION} declares no polymorphic sealed annotations"
              }
              return emptyList()
            }
            getter.invoke(parcelizeNames.getField("INSTANCE").get(null))
          } catch (failure: ReflectiveOperationException) {
            throw IllegalStateException(
                "Cannot read selected Parcelize polymorphic sealed annotations",
                failure,
            )
          }
      check(names is List<*>) { "Invalid Parcelize polymorphic sealed annotations: $names" }
      val annotations = names.map {
        check(it is FqName) { "Invalid Parcelize polymorphic sealed annotation: $it" }
        it
      }
      check(annotations.isNotEmpty() || !hasPolymorphicSealedRoots(KotlinCompilerVersion.VERSION)) {
        "Cannot export Parcelize CREATOR: the selected Parcelize backend for Kotlin " +
            "${KotlinCompilerVersion.VERSION} declares an empty list of polymorphic sealed annotations"
      }
      return annotations
    }

    // An unparsable version counts as 2.5 or later, so a missing name list fails loudly.
    private fun hasPolymorphicSealedRoots(compilerVersion: String): Boolean {
      val parts = compilerVersion.substringBefore('-').split('.')
      val major = parts.getOrNull(0)?.toIntOrNull() ?: return true
      val minor = parts.getOrNull(1)?.toIntOrNull() ?: return true
      return major > 2 || (major == 2 && minor >= 5)
    }

    private fun configuredAnnotations(
        extension: IrGenerationExtension,
        configuration: CompilerConfiguration,
    ): List<FqName> {
      // The optional plugin owns the key identity; use its public API in its registering
      // classloader.
      val key =
          try {
            val keys =
                extension.javaClass.classLoader.loadClass(
                    "org.jetbrains.kotlin.parcelize.ParcelizeConfigurationKeys",
                )
            keys.getMethod("getADDITIONAL_ANNOTATION").invoke(keys.getField("INSTANCE").get(null))
          } catch (failure: ReflectiveOperationException) {
            throw IllegalStateException(
                "Cannot read selected Parcelize annotation configuration",
                failure,
            )
          }
      check(key is CompilerConfigurationKey<*>) { "Invalid Parcelize annotation configuration key" }
      val additional =
          when (val value = configuration.get(key)) {
            null -> emptyList()
            is List<*> ->
                value.map {
                  check(it is String) { "Invalid Parcelize additional annotation: $it" }
                  FqName(it)
                }
            else -> error("Invalid Parcelize annotation configuration: $value")
          }
      return listOf(
          FqName("kotlinx.parcelize.Parcelize"),
          FqName("kotlinx.android.parcel.Parcelize"),
      ) + additional
    }
  }
}
