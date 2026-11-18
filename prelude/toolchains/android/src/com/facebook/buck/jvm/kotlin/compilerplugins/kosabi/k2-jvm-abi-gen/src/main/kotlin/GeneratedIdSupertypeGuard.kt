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

import java.io.File
import java.io.FileNotFoundException
import java.util.jar.JarFile
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageLocation
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageSeverity
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclaration
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.types.IrErrorType
import org.jetbrains.kotlin.ir.types.IrType
import org.jetbrains.kotlin.ir.types.classFqName
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.util.classId
import org.jetbrains.kotlin.ir.util.fileOrNull
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName

internal const val GENERATED_ID_PLUGIN_IR_EXTENSION =
    "com.facebook.kotlin.compilerplugins.generatedid.compilerplugin.GeneratedIdPluginExtension"

/**
 * Runs generatedid's extension, then settles the `IHaveGeneratedId` supertype it added to a class
 * whose supertypes it could not all see.
 *
 * generatedid declares `IHaveGeneratedId` on a `@GenerateId` class unless a supertype already
 * implements it, and counts a supertype it cannot resolve as one that does not. On the reduced
 * classpath a supertype is often a Kosabi stub, which declares no supertypes of its own:
 * `ComposerEvent : IEvent : IHaveGeneratedId` reads as a bare `ComposerEvent`. generatedid's choice
 * is therefore the library's only when every ancestor of the class is readable. Otherwise the class
 * keeps or loses the interface only on what the classpath proves:
 * - An `@Event` class compiled with the STAX Events plugin inherits the interface, because that
 *   plugin rejects an `@Event` class that does not implement `IEvent`, and `IEvent` extends
 *   `IHaveGeneratedId` on this classpath. The interface comes off the class.
 * - For any other class the library's choice cannot be known, and source-only ABI generation fails
 *   with an error that names the unreadable supertypes.
 *
 * The `_getGeneratedId` override that generatedid adds in both cases is kept.
 */
internal class GeneratedIdSupertypeGuard(
    private val delegate: IrGenerationExtension,
    private val stubClasspathRoots: List<File>,
    private val stubSourcePaths: Set<String>,
    /**
     * Classes whose supertypes on missing dependencies were stripped from FIR before IR conversion,
     * each with the supertypes it lost. Their IR supertypes no longer show what the library sees.
     */
    private val strippedSupertypes: Map<ClassId, Set<ClassId>>,
    /** Whether the STAX Events plugin, which checks `@Event` classes, is on this compilation. */
    private val staxEventsEnabled: Boolean,
    private val messageCollector: MessageCollector,
) : IrGenerationExtension {

  /**
   * JVM internal names of the classes the stub classpath declares; null, after reporting an error,
   * when it cannot be read. Every class off the source files then counts as a stub.
   */
  private val stubClassNames: Set<String>? by lazy {
    val names = HashSet<String>()
    for (root in stubClasspathRoots) {
      try {
        addClassNames(root, names)
      } catch (e: Exception) {
        messageCollector.report(
            CompilerMessageSeverity.ERROR,
            "Kosabi source-only ABI: cannot read the Kosabi stub classpath entry `$root`, which " +
                "tells which supertypes of `@GenerateId` classes are stubs: $e",
            null,
        )
        return@lazy null
      }
    }
    names
  }

  override fun generate(moduleFragment: IrModuleFragment, pluginContext: IrPluginContext) {
    val declaredSuperTypes = LinkedHashMap<IrClass, List<IrType>>()
    for (file in moduleFragment.files) {
      collectClasses(file.declarations, declaredSuperTypes)
    }

    delegate.generate(moduleFragment, pluginContext)

    for ((irClass, declared) in declaredSuperTypes) {
      val superTypes = irClass.superTypes
      if (
          superTypes.size != declared.size + 1 ||
              superTypes.last().classFqName != I_HAVE_GENERATED_ID ||
              superTypes.subList(0, declared.size) != declared
      ) {
        continue
      }
      val unreadable = LinkedHashSet<String>()
      val visited = HashSet<IrClass>()
      irClass.collectStrippedSupertypes(unreadable)
      for (superType in declared) {
        superType.collectUnreadableAncestors(visited, unreadable)
      }
      if (unreadable.isEmpty()) {
        continue
      }
      if (inheritsThroughIEvent(irClass, pluginContext)) {
        irClass.superTypes = declared
        dropGeneratedIdOverrideTarget(irClass)
      } else {
        reportUndecidable(irClass, unreadable)
      }
    }
  }

  private fun collectClasses(
      declarations: List<IrDeclaration>,
      into: MutableMap<IrClass, List<IrType>>,
  ) {
    for (declaration in declarations) {
      if (declaration is IrClass) {
        into[declaration] = declaration.superTypes
        collectClasses(declaration.declarations, into)
      }
    }
  }

  /**
   * Adds to [into] this type, or the ancestors reached through it, whose supertypes cannot be read:
   * Kosabi stubs and types that do not resolve, including supertypes stripped from FIR because they
   * do not resolve.
   */
  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrType.collectUnreadableAncestors(
      visited: MutableSet<IrClass>,
      into: MutableSet<String>,
  ) {
    val irClass = if (this is IrErrorType) null else classOrNull?.owner
    if (irClass == null) {
      into += classFqName?.asString() ?: "<unresolved type>"
      return
    }
    if (!visited.add(irClass)) {
      return
    }
    if (irClass.isStub()) {
      into += irClass.classId?.asSingleFqName()?.asString() ?: irClass.name.asString()
      return
    }
    irClass.collectStrippedSupertypes(into)
    for (superType in irClass.superTypes) {
      superType.collectUnreadableAncestors(visited, into)
    }
  }

  /** Adds to [into] the supertypes of this class that were stripped because they do not resolve. */
  private fun IrClass.collectStrippedSupertypes(into: MutableSet<String>) {
    val lost = strippedSupertypes[classId ?: return] ?: return
    lost.mapTo(into) { it.asSingleFqName().asString() }
  }

  /**
   * True when the library declares `IHaveGeneratedId` on [irClass] only through `IEvent`: STAX
   * Events checks it as an `@Event` class, and `IEvent` resolves to a readable class that extends
   * `IHaveGeneratedId`.
   */
  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun inheritsThroughIEvent(irClass: IrClass, pluginContext: IrPluginContext): Boolean {
    if (!staxEventsEnabled || !irClass.hasAnnotation(EVENT_ANNOTATION)) {
      return false
    }
    val iEvent = pluginContext.referenceClass(I_EVENT)?.owner ?: return false
    return iEvent.extendsGeneratedId(HashSet())
  }

  /** True when this class is `IHaveGeneratedId`, or a readable class that extends it. */
  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.extendsGeneratedId(visited: MutableSet<IrClass>): Boolean {
    if (classId?.asSingleFqName() == I_HAVE_GENERATED_ID) {
      return true
    }
    if (!visited.add(this) || isStub()) {
      return false
    }
    return superTypes.any { superType ->
      superType !is IrErrorType && superType.classOrNull?.owner?.extendsGeneratedId(visited) == true
    }
  }

  private fun reportUndecidable(irClass: IrClass, unreadable: Set<String>) {
    if (stubClassNames == null) {
      // The unreadable stub classpath is reported instead; edges would not help.
      return
    }
    val className = irClass.classId?.asSingleFqName()?.asString() ?: irClass.name.asString()
    val supertypes = unreadable.joinToString(", ") { "`$it`" }
    messageCollector.report(
        CompilerMessageSeverity.ERROR,
        "Kosabi source-only ABI: cannot tell whether `$className` inherits `IHaveGeneratedId`. " +
            "The generatedid plugin declares that interface on a `@GenerateId` class only when no " +
            "supertype implements it, and whether $supertypes does cannot be read here: each is a " +
            "Kosabi stub, which declares no supertypes, or does not resolve on the source-only ABI " +
            "classpath. Add the targets that provide them to `source_only_abi_deps`.",
        irClass.location(),
    )
  }

  private fun IrClass.location(): CompilerMessageLocation? {
    val file = fileOrNull ?: return null
    return CompilerMessageLocation.create(
        file.fileEntry.name,
        file.fileEntry.getLineNumber(startOffset) + 1,
        file.fileEntry.getColumnNumber(startOffset) + 1,
        null,
    )
  }

  private fun IrClass.isStub(): Boolean {
    val file = fileOrNull
    if (file != null) {
      return file.fileEntry.name in stubSourcePaths
    }
    val classId = classId ?: return true
    val names = stubClassNames ?: return true
    val packagePath = classId.packageFqName.asString().replace('.', '/')
    val className = classId.relativeClassName.asString().replace('.', '$')
    return (if (packagePath.isEmpty()) className else "$packagePath/$className") in names
  }

  /**
   * The class no longer declares `IHaveGeneratedId` here, so its `_getGeneratedId` stops naming the
   * interface's function as the one it overrides.
   */
  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun dropGeneratedIdOverrideTarget(irClass: IrClass) {
    for (function in irClass.declarations) {
      if (function !is IrSimpleFunction || function.name.asString() != GET_GENERATED_ID) {
        continue
      }
      function.overriddenSymbols =
          function.overriddenSymbols.filterNot {
            (it.owner.parent as? IrClass)?.classId?.asSingleFqName() == I_HAVE_GENERATED_ID
          }
    }
  }

  private companion object {
    val I_HAVE_GENERATED_ID =
        FqName("com.facebook.kotlin.compilerplugins.generatedid.interfaces.IHaveGeneratedId")
    val I_EVENT = ClassId.topLevel(FqName("com.facebook.xapp.messaging.events.interfaces.IEvent"))
    val EVENT_ANNOTATION = FqName("com.facebook.xapp.messaging.events.annotations.Event")
    const val GET_GENERATED_ID = "_getGeneratedId"

    /** Adds the JVM internal names of the classes under [root]; throws when it cannot read them. */
    fun addClassNames(root: File, into: MutableSet<String>) {
      when {
        root.isDirectory ->
            root
                .walkTopDown()
                .onFail { _, e -> throw e }
                .filter { it.isFile && it.extension == "class" }
                .forEach {
                  into += it.relativeTo(root).invariantSeparatorsPath.removeSuffix(".class")
                }
        root.isFile ->
            JarFile(root).use { jar ->
              jar.entries()
                  .asSequence()
                  .filter { !it.isDirectory && it.name.endsWith(".class") }
                  .forEach { into += it.name.removeSuffix(".class") }
            }
        else -> throw FileNotFoundException("$root is neither a file nor a directory")
      }
    }
  }
}
