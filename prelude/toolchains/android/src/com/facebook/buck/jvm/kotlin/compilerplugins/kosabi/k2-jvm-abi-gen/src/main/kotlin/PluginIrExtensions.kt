/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook

import com.facebook.buck.jvm.kotlin.compilerplugins.common.isStub
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.cli.jvm.config.jvmClasspathRoots
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.psi.KtFile

/**
 * Registered plugin [IrGenerationExtension]s that abi-gen runs itself.
 *
 * Kosabi skips the standard backend, so plugin IR extensions never run in the source-only ABI
 * action and their ABI-visible effects are missing unless abi-gen reproduces them. A plugin listed
 * here has an extension that is safe on abi-gen's body-less IR and needs no emulation: abi-gen runs
 * the plugin's own extension, configured exactly as in the full compilation. Plugins not listed
 * stay skipped.
 *
 * Entries are exact class names; the extension is registered only when its plugin is on the
 * compilation, so listing a plugin does not enable it.
 */
internal val ABI_RELEVANT_PLUGIN_IR_EXTENSIONS: Set<String> = setOf(
    // DataClassGenerate replaces a data class's `Any` superclass with `DataClassSuper`.
    "com.facebook.kotlin.compilerplugins.dataclassgenerate.DataClassGenerateIrGenerationExtension",
    // STAX Events adds `getParentEventNames` to `@Event` classes, and the `EventHandler` supertype
    // with `invoke` and `traceName` overrides to `@EventHandler` classes. With `SKIP_BODIES` set,
    // which abi-gen sets on the configuration the extension is registered with, it runs only those
    // two transforms; its parity, handler-list and controller-config transforms stay skipped.
    STAX_EVENTS_PLUGIN_IR_EXTENSION,
    // generatedid adds the `IHaveGeneratedId` supertype and a `_getGeneratedId` override to
    // `@GenerateId` classes, including classes whose annotation is meta-annotated with it
    // (`@Event`). It runs inside a `GeneratedIdSupertypeGuard`.
    GENERATED_ID_PLUGIN_IR_EXTENSION,
)

private const val STAX_EVENTS_PLUGIN_IR_EXTENSION =
    "com.facebook.kotlin.compilerplugins.xapp.events.EventsPluginExtension"

internal fun abiRelevantPluginIrExtensions(
    project: Project,
    configuration: CompilerConfiguration,
    sourceFiles: List<KtFile>,
    messageCollector: MessageCollector,
    strippedSupertypes: Map<ClassId, Set<ClassId>>,
): List<IrGenerationExtension> {
  val extensions =
      registeredIrGenerationExtensionsCompat(project, configuration).filter {
        it.javaClass.name in ABI_RELEVANT_PLUGIN_IR_EXTENSIONS
      }
  val staxEventsEnabled = extensions.any { it.javaClass.name == STAX_EVENTS_PLUGIN_IR_EXTENSION }
  return extensions.map {
    if (it.javaClass.name == GENERATED_ID_PLUGIN_IR_EXTENSION) {
      GeneratedIdSupertypeGuard(
          delegate = it,
          stubClasspathRoots =
              configuration.jvmClasspathRoots.filter { root -> root.isStubClasspathRoot() },
          stubSourcePaths =
              sourceFiles
                  .filter { file -> file.isStub() }
                  .map { file -> file.viewProvider.virtualFile.path }
                  .toSet(),
          strippedSupertypes = strippedSupertypes,
          staxEventsEnabled = staxEventsEnabled,
          messageCollector = messageCollector,
      )
    } else {
      it
    }
  }
}
