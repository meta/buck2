/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("OPT_IN_USAGE_ERROR")
@file:OptIn(
    com.facebook.DeprecatedForRemovalCompilerApiCompat::class,
    com.facebook.DirectDeclarationsAccessCompat::class,
)

package com.facebook

import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.io.File
import org.jetbrains.kotlin.cli.common.config.KotlinSourceRoot
import org.jetbrains.kotlin.cli.common.config.kotlinSourceRoots
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageLocation
import org.jetbrains.kotlin.cli.common.messages.CompilerMessageSeverity
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.cli.jvm.compiler.EnvironmentConfigFiles
import org.jetbrains.kotlin.cli.jvm.compiler.KotlinCoreEnvironment
import org.jetbrains.kotlin.cli.jvm.compiler.VfsBasedProjectEnvironment
import org.jetbrains.kotlin.cli.jvm.config.jvmClasspathRoots
import org.jetbrains.kotlin.com.intellij.openapi.diagnostic.Logger
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.com.intellij.openapi.util.Disposer
import org.jetbrains.kotlin.com.intellij.openapi.vfs.StandardFileSystems
import org.jetbrains.kotlin.com.intellij.openapi.vfs.VirtualFile
import org.jetbrains.kotlin.com.intellij.openapi.vfs.VirtualFileManager
import org.jetbrains.kotlin.com.intellij.psi.PsiManager
import org.jetbrains.kotlin.config.CommonConfigurationKeys
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.config.JVMConfigurationKeys
import org.jetbrains.kotlin.config.messageCollector
import org.jetbrains.kotlin.descriptors.ClassKind
import org.jetbrains.kotlin.descriptors.EffectiveVisibility
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.descriptors.Visibilities
import org.jetbrains.kotlin.extensions.CompilerConfigurationExtension
import org.jetbrains.kotlin.extensions.PreprocessedFileCreator
import org.jetbrains.kotlin.fir.FirElement
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirConstructor
import org.jetbrains.kotlin.fir.declarations.FirEnumEntry
import org.jetbrains.kotlin.fir.declarations.FirRegularClass
import org.jetbrains.kotlin.fir.declarations.FirTypeAlias
import org.jetbrains.kotlin.fir.declarations.impl.FirResolvedDeclarationStatusImpl
import org.jetbrains.kotlin.fir.declarations.utils.isConst
import org.jetbrains.kotlin.fir.declarations.utils.isFromLibrary
import org.jetbrains.kotlin.fir.expressions.FirAnonymousObjectExpression
import org.jetbrains.kotlin.fir.expressions.builder.buildLiteralExpression
import org.jetbrains.kotlin.fir.extensions.ExperimentalTopLevelDeclarationsGenerationApi
import org.jetbrains.kotlin.fir.extensions.FirAnalysisHandlerExtension
import org.jetbrains.kotlin.fir.extensions.FirDeclarationGenerationExtension
import org.jetbrains.kotlin.fir.extensions.FirExtensionRegistrar
import org.jetbrains.kotlin.fir.extensions.FirExtensionSessionComponent
import org.jetbrains.kotlin.fir.extensions.MemberGenerationContext
import org.jetbrains.kotlin.fir.pipeline.buildFirFromKtFiles
import org.jetbrains.kotlin.fir.pipeline.runResolution
import org.jetbrains.kotlin.fir.plugin.createMemberFunction
import org.jetbrains.kotlin.fir.plugin.createMemberProperty
import org.jetbrains.kotlin.fir.plugin.createTopLevelClass
import org.jetbrains.kotlin.fir.resolve.providers.dependenciesSymbolProvider
import org.jetbrains.kotlin.fir.resolve.providers.symbolProvider
import org.jetbrains.kotlin.fir.symbols.SymbolInternals
import org.jetbrains.kotlin.fir.symbols.impl.FirCallableSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirClassLikeSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirClassSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirEnumEntrySymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirNamedFunctionSymbol
import org.jetbrains.kotlin.fir.symbols.impl.FirPropertySymbol
import org.jetbrains.kotlin.fir.types.ConeClassLikeType
import org.jetbrains.kotlin.fir.types.ConeKotlinType
import org.jetbrains.kotlin.fir.types.ConeKotlinTypeProjection
import org.jetbrains.kotlin.fir.types.FirResolvedTypeRef
import org.jetbrains.kotlin.fir.types.coneType
import org.jetbrains.kotlin.fir.visitors.FirDefaultVisitorVoid
import org.jetbrains.kotlin.idea.KotlinFileType
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.util.isFileClass
import org.jetbrains.kotlin.metadata.jvm.JvmModuleProtoBuf
import org.jetbrains.kotlin.modules.Module
import org.jetbrains.kotlin.modules.TargetId
import org.jetbrains.kotlin.name.CallableId
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.psi.KtAnnotationEntry
import org.jetbrains.kotlin.psi.KtClass
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.psi.KtTreeVisitorVoid
import org.jetbrains.kotlin.types.ConstantValueKind

@SuppressWarnings("PackageLocationMismatch")
class K2JvmAbiFirAnalysisHandlerExtension(private val outputPath: String) :
    FirAnalysisHandlerExtension() {

  private val pipeline = AbiGenPipeline()

  override fun isApplicable(configuration: CompilerConfiguration): Boolean {
    return true
  }

  override fun doAnalysis(project: Project, configuration: CompilerConfiguration): Boolean {
    val updatedConfiguration =
        configuration.copy().apply {
          retainOutputInMemoryCompat()
          put(JVMConfigurationKeys.OUTPUT_DIRECTORY, File(outputPath))
          put(JVMConfigurationKeys.VALIDATE_BYTECODE, true)
          put(JVMConfigurationKeys.SKIP_BODIES, true)
        }
    // the disposable is responsible to dispose the project after the analysis is done
    val disposable = Disposer.newDisposable("K2KosabiSession.project")
    try {
      val projectEnvironment = createProjectEnvironmentCompat(
          updatedConfiguration,
          disposable,
          EnvironmentConfigFiles.JVM_CONFIG_FILES,
          configuration.messageCollector,
      )
      if (updatedConfiguration.messageCollector.hasErrors()) {
        return false
      }
      val messageCollector =
          updatedConfiguration.getNotNull(CommonConfigurationKeys.MESSAGE_COLLECTOR_KEY)
      val module =
          updatedConfiguration[JVMConfigurationKeys.MODULES]?.single()
              ?: error(
                  "Single module expected: ${updatedConfiguration[JVMConfigurationKeys.MODULES]}",
              )

      val sourceFiles = createSourceFilesFromSourceRoots(
          updatedConfiguration,
          projectEnvironment.project,
          configuration.kotlinSourceRoots,
      )

      // Run FIR frontend to analyze source files
      val analysisResults = runFrontendForKosabi(
          projectEnvironment,
          updatedConfiguration,
          sourceFiles,
          module,
      )

      // Decide before any FIR cleanup below rewrites declarations.
      pipeline.inlineBodies.decide(analysisResults, sourceFiles, messageCollector)

      // Collect class usage from the resolved FIR tree for dep file tracking.
      // The DependencyTracker compiler plugin's FIR checkers don't run in kosabi's
      // inner session (separate project, checkers skipped), so we walk the FIR tree
      // directly to extract classpath class references.
      KosabiClassUsageCollector().collectAndDump(analysisResults, configuration)

      // Detect missing transitive dependencies by checking if all supertypes can be resolved.
      // If any are found, strip their supertype references from FIR to prevent crashes
      // during FIR-to-IR fake override building.
      val missingTypes = collectMissingTypesFromFir(analysisResults)
      if (missingTypes.isNotEmpty()) {
        stripUnresolvableSupertypesFromFir(analysisResults, missingTypes)
      }

      runBackendForKosabi(
          messageCollector,
          projectEnvironment,
          updatedConfiguration,
          module,
          analysisResults,
          sourceFiles,
      )
    } finally {
      Disposer.dispose(disposable)
    }
    return true
  }

  // Collect missing transitive dependencies by checking if supertypes can be resolved.
  // When FIR loads a class from bytecode (e.g., from a stub JAR), it includes references
  // to that class's supertypes. If those supertypes are not on the classpath (because
  // they're transitive dependencies not included in the stub JAR), we detect them here.
  private fun collectMissingTypesFromFir(analysisResults: FrontendOutputCompat): Set<ClassId> {
    val missingTypes = mutableSetOf<ClassId>()

    for (output in analysisResults.outputs) {
      val session = output.session

      for (firFile in output.fir) {
        firFile.accept(MissingTypeCollectorVisitor(session, missingTypes))
      }
    }

    return missingTypes
  }

  // Visitor that walks FIR and checks if all referenced types can be resolved,
  // including the entire supertype hierarchy of dependency classes
  @OptIn(SymbolInternals::class)
  private class MissingTypeCollectorVisitor(
      private val session: FirSession,
      private val missingTypes: MutableSet<ClassId>,
  ) : FirNamedFunctionVisitorCompat() {

    // Track already-checked classes to avoid infinite recursion
    private val checkedClasses = mutableSetOf<ClassId>()

    override fun visitElement(element: FirElement) {
      // Only visit declarations, not expressions/bodies (avoid deep recursion)
      // We only care about type references in declarations
    }

    override fun visitFile(file: org.jetbrains.kotlin.fir.declarations.FirFile) {
      // Visit top-level declarations
      for (declaration in file.declarations) {
        declaration.accept(this)
      }
    }

    override fun visitRegularClass(regularClass: FirRegularClass) {
      // Check if all supertypes can be resolved, including their supertypes.
      // Use isFromDependencyChain=true because if a source class's supertype can't be
      // resolved anywhere (not in deps, not in sources), it's a missing transitive
      // dependency that will crash FIR-to-IR fake override building.
      for (superTypeRef in regularClass.superTypeRefs) {
        checkTypeResolvableRecursively(superTypeRef.coneType, isFromDependencyChain = true)
      }
      // Visit nested declarations (but not bodies)
      for (declaration in regularClass.declarations) {
        declaration.accept(this)
      }
    }

    override fun visitNamedFunctionCompat(namedFunction: FirNamedFunctionCompat) {
      // Check return type - these are from source, so start with isFromDependencyChain=false
      checkTypeResolvableRecursively(
          namedFunction.returnTypeRef.coneType,
          isFromDependencyChain = false,
      )

      // Check parameter types
      for (valueParameter in namedFunction.valueParameters) {
        checkTypeResolvableRecursively(
            valueParameter.returnTypeRef.coneType,
            isFromDependencyChain = false,
        )
      }
      // Do NOT call super - we don't need to visit function bodies
    }

    override fun visitProperty(property: org.jetbrains.kotlin.fir.declarations.FirProperty) {
      // Check property type
      checkTypeResolvableRecursively(property.returnTypeRef.coneType, isFromDependencyChain = false)
      // Do NOT call super - we don't need to visit property initializers
    }

    override fun visitConstructor(
        constructor: org.jetbrains.kotlin.fir.declarations.FirConstructor,
    ) {
      // Check parameter types
      for (valueParameter in constructor.valueParameters) {
        checkTypeResolvableRecursively(
            valueParameter.returnTypeRef.coneType,
            isFromDependencyChain = false,
        )
      }
      // Do NOT call super - we don't need to visit constructor bodies
    }

    override fun visitTypeAlias(typeAlias: FirTypeAlias) {
      // Check that the expanded type can be resolved - type aliases serialize
      // expandedType into metadata, and if it references a missing transitive
      // dependency the ABI jar will have corrupt TypeAlias protobuf
      checkTypeResolvableRecursively(
          typeAlias.expandedTypeRef.coneType,
          isFromDependencyChain = false,
      )
    }

    // Check if a type can be resolved via the symbol provider, and recursively
    // check all supertypes of resolved classes. This is necessary because stub JARs
    // may contain classes that extend other classes not in the stub JAR.
    // The isFromDependencyChain parameter tracks whether we're checking a type that
    // was found in the supertype chain of a dependency class.
    private fun checkTypeResolvableRecursively(
        type: org.jetbrains.kotlin.fir.types.ConeKotlinType,
        isFromDependencyChain: Boolean = false,
    ) {
      when (type) {
        is org.jetbrains.kotlin.fir.types.ConeErrorType -> {
          // FIR couldn't resolve this type - it represents a missing dependency.
          // ConeErrorType extends ConeClassLikeType, so we must check it first.
          // Extract the original class ID and add to missing types unconditionally,
          // since an error type always indicates something is unresolvable.
          try {
            val classId = type.lookupTag.classId
            if (!classId.isLocal && !isStdlibOrJdkPackage(classId.packageFqName.asString())) {
              missingTypes.add(classId)
            }
          } catch (_: Exception) {
            // If we can't extract the classId from the error type, skip it.
            // The error will surface later during FIR-to-IR conversion.
          }
        }
        is org.jetbrains.kotlin.fir.types.ConeClassLikeType -> {
          val classId = type.lookupTag.classId

          // Skip if already checked
          if (classId in checkedClasses) return
          checkedClasses.add(classId)

          // Skip standard library classes - they're always available
          if (isStdlibOrJdkPackage(classId.packageFqName.asString())) {
            return
          }

          // Skip local classes (anonymous classes, lambdas, object expressions)
          // These are local to the source file and can't be missing transitive dependencies.
          // Also, the symbol provider throws an error when trying to look them up.
          if (classId.isLocal) {
            return
          }

          // Try to resolve the class symbol from dependencies
          val depSymbol = session.dependenciesSymbolProvider.getClassLikeSymbolByClassId(classId)
          if (depSymbol == null) {
            // Not in dependencies - could be a source class (Kotlin or Java)
            // Try to resolve from all symbol providers (includes Java sources)
            val sourceSymbol = session.symbolProvider.getClassLikeSymbolByClassId(classId)
            if (sourceSymbol == null) {
              // Cannot be resolved at all
              if (isFromDependencyChain) {
                // This class is in the supertype chain of a dependency or source class
                // but cannot be resolved - it's a missing transitive dependency.
                // This includes nested/inner classes that FIR's symbol provider cannot
                // resolve by ClassId even when present in dependency JARs. We add ALL
                // unresolvable classes so their supertype references get stripped from
                // FIR before FIR-to-IR conversion (which would also fail to find them).
                missingTypes.add(classId)
              }
              return
            } else {
              // Found in sources (Kotlin or Java) - recursively check its supertypes
              // When checking source class supertypes, we're now in a dependency chain
              // because any supertype not in sources must come from dependencies
              when (val firDecl = sourceSymbol.fir) {
                is FirRegularClass -> {
                  for (superTypeRef in firDecl.superTypeRefs) {
                    checkTypeResolvableRecursively(
                        superTypeRef.coneType,
                        isFromDependencyChain = true,
                    )
                  }
                }
                is FirTypeAlias -> {
                  // Follow the expanded type of the type alias - any expanded type
                  // not in sources must come from dependencies
                  checkTypeResolvableRecursively(
                      firDecl.expandedTypeRef.coneType,
                      isFromDependencyChain = true,
                  )
                }
                else -> {}
              }
            }
          } else {
            // Class IS resolvable from dependencies - now recursively check ITS supertypes
            // These are now definitely from the dependency chain
            when (val firDecl = depSymbol.fir) {
              is FirRegularClass -> {
                for (superTypeRef in firDecl.superTypeRefs) {
                  checkTypeResolvableRecursively(
                      superTypeRef.coneType,
                      isFromDependencyChain = true,
                  )
                }
              }
              is FirTypeAlias -> {
                // Follow the expanded type of the type alias - it may reference
                // a transitive dependency not on the source-only ABI classpath
                checkTypeResolvableRecursively(
                    firDecl.expandedTypeRef.coneType,
                    isFromDependencyChain = true,
                )
              }
              else -> {}
            }
          }
        }
        is org.jetbrains.kotlin.fir.types.ConeTypeParameterType -> {
          // Type parameters are always resolvable
        }
        is org.jetbrains.kotlin.fir.types.ConeFlexibleType -> {
          // Check both bounds
          checkTypeResolvableRecursively(type.lowerBound, isFromDependencyChain)
          checkTypeResolvableRecursively(type.upperBound, isFromDependencyChain)
        }
        else -> {
          // Other types don't need checking
        }
      }
    }
  }

  // Strip supertypes that reference missing transitive dependencies from FIR classes.
  // This walks source FIR classes and follows their supertype chains into dependency
  // classes, removing any supertype reference whose ClassId is in the missing set.
  // This is done BEFORE FIR-to-IR conversion to prevent crashes during fake override
  // building (Fir2IrDeclarationStorage.findContainingIrClassSymbol).
  @OptIn(SymbolInternals::class)
  private fun stripUnresolvableSupertypesFromFir(
      analysisResults: FrontendOutputCompat,
      missingTypes: Set<ClassId>,
  ) {
    val visited = mutableSetOf<ClassId>()

    fun stripFromClassRecursively(firClass: FirRegularClass, session: FirSession) {
      val classId = firClass.symbol.classId
      if (classId in visited) return
      visited.add(classId)

      // Strip supertypes that reference missing types
      val toRemove =
          firClass.superTypeRefs.filter { typeRef ->
            val coneType =
                (typeRef as? FirResolvedTypeRef)?.coneType as? ConeClassLikeType
                    ?: return@filter false
            coneType.lookupTag.classId in missingTypes
          }
      if (toRemove.isNotEmpty()) {
        val filtered = firClass.superTypeRefs - toRemove.toSet()
        try {
          // Works for standard FirRegularClass (Kotlin classes from bytecode or source)
          firClass.replaceSuperTypeRefs(filtered)
        } catch (_: IllegalStateException) {
          // FirJavaClass doesn't support replaceSuperTypeRefs because superTypeRefs
          // is a lazy delegate. Use reflection to replace the lazy delegate's value.
          try {
            val delegateField = firClass.javaClass.getDeclaredField("superTypeRefs\$delegate")
            delegateField.isAccessible = true
            delegateField.set(firClass, lazyOf(filtered))
          } catch (_: Exception) {
            // If reflection fails, skip - the error will surface during FIR-to-IR
          }
        }
      }

      // Recursively process remaining supertypes' FIR classes (including dependency classes)
      for (superTypeRef in firClass.superTypeRefs) {
        val coneType =
            (superTypeRef as? FirResolvedTypeRef)?.coneType as? ConeClassLikeType ?: continue
        val superClassId = coneType.lookupTag.classId
        if (isStdlibOrJdkPackage(superClassId.packageFqName.asString())) {
          continue
        }
        val superSymbol =
            session.symbolProvider.getClassLikeSymbolByClassId(superClassId) ?: continue
        val superClass = superSymbol.fir as? FirRegularClass ?: continue
        stripFromClassRecursively(superClass, session)
      }
    }

    for (output in analysisResults.outputs) {
      val session = output.session

      for (firFile in output.fir) {
        firFile.accept(
            object : FirDefaultVisitorVoid() {
              override fun visitElement(element: FirElement) {}

              override fun visitFile(file: org.jetbrains.kotlin.fir.declarations.FirFile) {
                for (declaration in file.declarations) {
                  declaration.accept(this)
                }
              }

              override fun visitRegularClass(regularClass: FirRegularClass) {
                stripFromClassRecursively(regularClass, session)
                for (declaration in regularClass.declarations) {
                  declaration.accept(this)
                }
              }
            },
        )
      }
    }
  }

  fun convertAnalyzedFirToIr(
      configuration: CompilerConfiguration,
      targetId: TargetId,
      analysisResults: FrontendOutputCompat,
      environment: ModuleCompilerEnvironmentCompat,
      sourceFiles: List<KtFile>,
      project: Project,
  ): ModuleCompilerIrBackendInputCompat {
    val extensions = createJvmFir2IrExtensionsCompat(configuration)
    val composeAbiEnabled =
        configuration.get(K2JvmAbiConfigurationKeys.ENABLE_COMPOSE_ABI_EMULATION) == true
    val allIrExtensions = buildList {
      // Plugin extensions run first, before the emulations and the sanitizer read the IR they
      // change (e.g. Compose stability reads the superclass DataClassGenerate injects).
      addAll(abiRelevantPluginIrExtensions(project, configuration))
      // Compose ABI emulation runs before the IR sanitizer so that any emulated
      // declarations are present when non-ABI content is stripped.
      if (composeAbiEnabled) {
        add(pipeline.composeAbi.createExtension())
      }
      add(ParcelizeAbiEmulationExtension())
      // Restores kept inline bodies before the sanitizer, which leaves exactly those bodies alone.
      if (pipeline.inlineBodies.keptFirFunctions.isNotEmpty()) {
        add(pipeline.inlineBodies.createExtension(configuration.messageCollector))
      }
      add(pipeline.irSanitizer.createExtension(sourceFiles))
    }

    val (moduleFragment, components, pluginContext, irActualizedResult, _, symbolTable) =
        analysisResults.convertToIrAndActualizeForJvmCompat(
            extensions,
            configuration,
            environment.diagnosticsReporter,
            allIrExtensions,
        )

    return ModuleCompilerIrBackendInputCompat(
        targetId,
        configuration,
        extensions,
        moduleFragment,
        components,
        pluginContext,
        irActualizedResult,
        symbolTable,
    )
  }

  // A backend entry point for Kosabi.
  private fun runBackendForKosabi(
      messageCollector: MessageCollector,
      projectEnvironment: VfsBasedProjectEnvironment,
      configuration: CompilerConfiguration,
      module: Module,
      analysisResults: FrontendOutputCompat,
      sourceFiles: List<KtFile>,
  ): ModuleCompilerOutputCompat {
    val cleanDiagnosticReporter = createPendingReporterCompat(messageCollector)
    val compilerEnvironment =
        ModuleCompilerEnvironmentCompat(projectEnvironment, cleanDiagnosticReporter)

    // Phase 1: FIR pre-IR cleanup
    pipeline.firMetadataSanitizer.cleanupFirTree(analysisResults)

    // Phase 2: FIR-to-IR conversion (includes IR sanitizer as extension)

    val irInput = convertAnalyzedFirToIr(
        configuration,
        TargetId(module),
        analysisResults,
        compilerEnvironment,
        sourceFiles,
        projectEnvironment.project,
    )

    // Phase 3: FIR metadata post-IR cleanup
    val firSession = analysisResults.outputs.last().session
    pipeline.firMetadataSanitizer.cleanupFirMetadataSources(irInput.irModuleFragment, firSession)

    // Phase 4: Code generation
    val keptInlineBodies = pipeline.inlineBodies.restoredIrFunctions
    val result =
        if (keptInlineBodies.isEmpty()) {
          generateCodeFromIrCompat(irInput, compilerEnvironment)
        } else {
          generateCodeFromIrWithInlineBodiesCompat(
              irInput,
              compilerEnvironment,
              keptInlineBodies,
              messageCollector,
          )
        }

    // Write bytecode from generationState.factory to disk.
    // Apply bytecode post-processing (see BytecodeSanitizerStage) in-memory before writing to
    // avoid a separate read-back pass.
    val outputFiles =
        result.classFileFactoryCompat.asList().map { outputFile ->
          val originalBytes = outputFile.asByteArray()
          val bytes =
              if (outputFile.relativePath.endsWith(".class")) {
                pipeline.bytecodeSanitizer.transform(originalBytes)
              } else {
                originalBytes
              }
          AbiValidationOutputFile(outputFile.relativePath, bytes)
        }
    val outputDir = configuration[JVMConfigurationKeys.OUTPUT_DIRECTORY]
    if (outputDir != null) {
      outputFiles.forEach { outputFile ->
        val file = File(outputDir, outputFile.relativePath)
        file.parentFile?.mkdirs()
        file.writeBytes(outputFile.bytes)
      }
    }

    // Phase 5: Validation. Needs the written output files and the classpath, so it runs after
    // code generation rather than before it.
    pipeline.validator.validate(
        irInput.irModuleFragment,
        messageCollector,
        AbiRepairPolicy.parse(configuration.get(K2JvmAbiConfigurationKeys.ABI_VALIDATION_MODE)),
        AbiValidationInputs(outputFiles, configuration.jvmClasspathRoots),
    )

    // Generate .kotlin_module file
    generateKotlinModuleFile(irInput.irModuleFragment, module.getModuleName(), configuration)

    return result
  }

  // Generate the .kotlin_module file manually.
  // The standard generateCodeFromIr may not produce a complete .kotlin_module file
  // for source-only ABI generation. We scan the IR for file facade classes (*Kt.class)
  // and build the module metadata ourselves.
  private fun generateKotlinModuleFile(
      moduleFragment: IrModuleFragment,
      moduleName: String,
      configuration: CompilerConfiguration,
  ) {
    val outputDir = configuration[JVMConfigurationKeys.OUTPUT_DIRECTORY] ?: return

    // Collect all packages that have Kotlin IR files
    val allKotlinPackages = mutableSetOf<String>()
    // Collect package-to-file-facade mappings by scanning IR for *Kt classes
    val packageToFileFacades = mutableMapOf<String, MutableSet<String>>()

    // Iterate through all IR files to:
    // 1. Track all packages (for files with only objects/classes)
    // 2. Find file facade classes (classes for top-level functions)
    for (irFile in moduleFragment.files) {
      val packageFqName = irFile.packageFqName.asString()
      allKotlinPackages.add(packageFqName)

      for (decl in irFile.declarations) {
        if (decl is IrClass && decl.isFileClass) {
          // File facade classes are synthetic classes generated by the compiler to hold
          // top-level declarations. They are identified by their IrDeclarationOrigin
          // (FILE_CLASS, SYNTHETIC_FILE_CLASS, or JVM_MULTIFILE_CLASS).
          //
          // Only actual file facades may be listed as package parts in the kotlin_module.
          // Listing regular classes causes the consumer to read their Class protobuf as
          // a Package protobuf, where field 5 (TypeParameter) is misinterpreted as
          // TypeAlias — producing corrupt entries with no expandedType that crash with
          // "No expandedType in ProtoBuf.TypeAlias".
          packageToFileFacades.getOrPut(packageFqName) { mutableSetOf() }.add(decl.name.asString())
        }
      }
    }

    // Generate .kotlin_module if there are any Kotlin packages
    // even if they don't have file facades (e.g., files with only objects/classes)
    if (allKotlinPackages.isEmpty()) return

    // Build the Module protobuf
    val moduleBuilder = JvmModuleProtoBuf.Module.newBuilder()

    // Iterate over all packages (including those without file facades)
    for (packageFqName in allKotlinPackages.sorted()) {
      val packagePartsBuilder = JvmModuleProtoBuf.PackageParts.newBuilder()
      packagePartsBuilder.packageFqName = packageFqName

      // Add file facades if this package has any
      val fileFacades = packageToFileFacades[packageFqName]
      if (fileFacades != null) {
        for (fileFacade in fileFacades.sorted()) {
          packagePartsBuilder.addShortClassName(fileFacade)
        }
      }

      moduleBuilder.addPackageParts(packagePartsBuilder.build())
    }

    val moduleProto = moduleBuilder.build()

    // Serialize the module
    val versionArray = currentMetadataVersionArray()
    // 4KB initial buffer - aligns with memory page size and is sufficient for most .kotlin_module
    // files
    val baos = ByteArrayOutputStream(4096)
    val out = DataOutputStream(baos)
    out.writeInt(versionArray.size)
    versionArray.forEach { number -> out.writeInt(number) }
    // Write flags for Kotlin 1.4+
    out.writeInt(0)
    moduleProto.writeTo(out)
    out.flush()

    // Write to META-INF/<moduleName>.kotlin_module
    val metaInfDir = File(outputDir, "META-INF")
    metaInfDir.mkdirs()
    val moduleFile = File(metaInfDir, "$moduleName.kotlin_module")
    moduleFile.writeBytes(baos.toByteArray())
  }

  // get all the source files from the source roots and allows the caller to process them
  fun List<KotlinSourceRoot>.forAllFiles(
      configuration: CompilerConfiguration,
      project: Project,
      reportLocation: CompilerMessageLocation? = null,
      body: (VirtualFile, Boolean, moduleName: String?) -> Unit,
  ) {
    val localFileSystem =
        VirtualFileManager.getInstance().getFileSystem(StandardFileSystems.FILE_PROTOCOL)

    val processedFiles = mutableSetOf<VirtualFile>()

    val virtualFileCreator = PreprocessedFileCreator(project)

    var pluginsConfigured = false
    fun ensurePluginsConfigured() {
      if (!pluginsConfigured) {
        for (extension in CompilerConfigurationExtension.getInstances(project)) {
          extension.updateFileRegistryCompat(project)
        }
        pluginsConfigured = true
      }
    }

    for ((sourceRootPath, isCommon, hmppModuleName) in this) {
      val sourceRoot = File(sourceRootPath)
      val vFile = localFileSystem.findFileByPath(sourceRoot.normalize().path)
      if (vFile == null) {
        val message = "Source file or directory not found: $sourceRootPath"

        val buildFilePath = configuration.get(JVMConfigurationKeys.MODULE_XML_FILE)
        if (buildFilePath != null && Logger.isInitialized()) {
          Logger.getInstance(KotlinCoreEnvironment::class.java)
              .warn(
                  "$message\n\nbuild file path: $buildFilePath\ncontent:\n${buildFilePath.readText()}",
              )
        }

        configuration.messageCollector.report(
            CompilerMessageSeverity.ERROR,
            message,
            reportLocation,
        )
        continue
      }

      if (!vFile.isDirectory && vFile.extension != KotlinFileType.EXTENSION) {
        ensurePluginsConfigured()
        if (vFile.fileType != KotlinFileType.INSTANCE) {
          configuration.messageCollector.report(
              CompilerMessageSeverity.ERROR,
              "Source entry is not a Kotlin file: $sourceRootPath",
              reportLocation,
          )
          continue
        }
      }

      for (file in sourceRoot.walkTopDown()) {
        if (!file.isFile) continue

        val virtualFile: VirtualFile? =
            localFileSystem.findFileByPath(file.absoluteFile.normalize().path)?.let {
              virtualFileCreator.create(it)
            }
        if (virtualFile != null) {
          val hasNotBeenProcessedBefore = processedFiles.add(virtualFile)
          if (!hasNotBeenProcessedBefore) continue
          if (virtualFile.extension != KotlinFileType.EXTENSION) {
            ensurePluginsConfigured()
          }
          if (
              virtualFile.extension == KotlinFileType.EXTENSION ||
                  virtualFile.fileType == KotlinFileType.INSTANCE
          ) {
            body(virtualFile, isCommon, hmppModuleName)
          }
        }
      }
    }
  }

  // get all the source files from the source roots and matches them with corresponding PSI
  fun createSourceFilesFromSourceRoots(
      configuration: CompilerConfiguration,
      project: Project,
      sourceRoots: List<KotlinSourceRoot>,
      reportLocation: CompilerMessageLocation? = null,
  ): MutableList<KtFile> {
    val psiManager = PsiManager.getInstance(project)
    val result = mutableListOf<KtFile>()
    sourceRoots.forAllFiles(configuration, project, reportLocation) {
        virtualFile,
        isCommon,
        moduleName ->
      psiManager.findFile(virtualFile)?.let {
        if (it is KtFile) {
          it.isCommonSourceCompat = isCommon
          if (moduleName != null) {
            it.hmppModuleNameCompat = moduleName
          }
          result.add(it)
        }
      }
    }
    return result
  }

  /**
   * A place where a constant is passed to an annotation parameter.
   *
   * The parameter's declared type is the only remaining evidence of a missing constant's type once
   * the declaring class is off the classpath, so it is captured alongside the expression text.
   */
  private data class ConstUsageSite(
      val expressionText: String,
      val annotationShortName: String?,
      val parameterName: String?,
      val parameterIndex: Int,
  )

  private class MissingConstantsVisitor(
      private val usedConstants: MutableSet<String>,
      private val usageSites: MutableList<ConstUsageSite>,
  ) : KtTreeVisitorVoid() {
    override fun visitAnnotationEntry(annotationEntry: KtAnnotationEntry) {
      val annotationShortName = annotationEntry.shortName?.asString()
      // Use getArgumentExpression()?.text to get just the value, not the parameter name
      // For @Annotation(param = VALUE), we want "VALUE", not "param = VALUE"
      // For array expressions like [A, B], extract individual elements
      annotationEntry.valueArgumentList?.arguments?.forEachIndexed { index, argument ->
        val expressionText = argument.getArgumentExpression()?.text ?: return@forEachIndexed
        val parameterName = argument.getArgumentName()?.asName?.asString()
        fun record(text: String) {
          usedConstants.add(text)
          usageSites.add(ConstUsageSite(text, annotationShortName, parameterName, index))
        }
        if (expressionText.startsWith("[") && expressionText.endsWith("]")) {
          // Array expression: parse and extract individual elements
          val arrayContent = expressionText.substring(1, expressionText.length - 1).trim()
          if (arrayContent.isNotEmpty()) {
            // Simple comma split - for annotation arguments, we don't expect complex nesting
            arrayContent.split(",").forEach { element ->
              val trimmed = element.trim()
              if (trimmed.isNotEmpty()) {
                record(trimmed)
              }
            }
          }
        } else {
          // Regular expression: add as-is
          record(expressionText)
        }
      }
      return super.visitAnnotationEntry(annotationEntry)
    }
  }

  /**
   * Resolves the declared type of the annotation parameter [site] is passed to.
   *
   * This runs before `buildFirFromKtFiles`, so annotations declared in the module being compiled
   * are only available as PSI while annotations from dependencies are only available as FIR; both
   * are tried. Returns null whenever the answer is not certain - a wrong type here would be exactly
   * the defect this exists to fix.
   */
  private fun resolveAnnotationParameterType(
      site: ConstUsageSite,
      sourceFile: KtFile,
      sourceFiles: List<KtFile>,
      session: FirSession,
  ): ConstTypeHint? {
    val annotationShortName = site.annotationShortName ?: return null

    val importedFqName =
        sourceFile.importDirectives
            .mapNotNull { it.importedFqName }
            .firstOrNull { it.shortName().asString() == annotationShortName }
    val candidates = listOfNotNull(
        importedFqName,
        sourceFile.packageFqName.child(Name.identifier(annotationShortName)),
    )

    // Recovering the type is best-effort: without it the old assumed-String behaviour still
    // applies. It must therefore never be able to turn a working compilation red, which querying
    // half-resolved declarations at this stage otherwise can.
    return try {
      candidates.firstNotNullOfOrNull { candidate ->
        annotationParameterTypeFromPsi(candidate, site, sourceFiles)
            ?: annotationParameterTypeFromFir(candidate, site, session)
      }
    } catch (e: Exception) {
      pipeline.repairLog.recordFailedRepair(
          annotationShortName,
          "could not read annotation parameter type: ${e.javaClass.simpleName}: ${e.message}",
      )
      null
    }
  }

  private fun annotationParameterTypeFromPsi(
      annotationFqName: FqName,
      site: ConstUsageSite,
      sourceFiles: List<KtFile>,
  ): ConstTypeHint? {
    val ktClass =
        sourceFiles
            .flatMap { it.declarations }
            .filterIsInstance<KtClass>()
            .firstOrNull { it.isAnnotation() && it.fqName == annotationFqName } ?: return null
    val parameters = ktClass.primaryConstructor?.valueParameters ?: return null
    val parameter =
        if (site.parameterName != null) {
          parameters.firstOrNull { it.name == site.parameterName }
        } else {
          parameters.getOrNull(site.parameterIndex)
        } ?: return null
    return constTypeHintFromTypeText(parameter.typeReference?.text)
  }

  /**
   * This runs before source declarations exist in FIR, so it deliberately consults only
   * `dependenciesSymbolProvider` - the same provider the surrounding missing-constant logic uses.
   * Reaching into the full symbol provider here resolves source and Java declarations too early.
   */
  @OptIn(SymbolInternals::class)
  private fun annotationParameterTypeFromFir(
      annotationFqName: FqName,
      site: ConstUsageSite,
      session: FirSession,
  ): ConstTypeHint? {
    val classSymbol =
        session.dependenciesSymbolProvider.getClassLikeSymbolByClassId(
            ClassId.topLevel(annotationFqName),
        ) as? FirClassSymbol<*> ?: return null
    val firClass = classSymbol.fir as? FirRegularClass ?: return null
    if (firClass.classKind != ClassKind.ANNOTATION_CLASS) return null
    val primaryConstructor =
        firClass.declarations.filterIsInstance<FirConstructor>().firstOrNull { it.isPrimary }
            ?: return null
    val parameter =
        if (site.parameterName != null) {
          primaryConstructor.valueParameters.firstOrNull {
            it.name.asString() == site.parameterName
          }
        } else {
          primaryConstructor.valueParameters.getOrNull(site.parameterIndex)
        } ?: return null
    // Java-declared annotations carry a FirJavaTypeRef at this point; asking such a ref for its
    // cone type throws. No type is better than a crash, and better than a guessed one.
    val typeRef = parameter.returnTypeRef as? FirResolvedTypeRef ?: return null
    return constTypeHintFromConeType(typeRef.coneType)
  }

  // Collect missing constants from source files by analyzing imports
  private fun collectMissingConstantsFromSourceFiles(
      sourceFiles: List<KtFile>,
      session: FirSession,
      state: AbiGenState,
  ): Map<ClassId, Set<String>> {
    val missingConstants = mutableMapOf<ClassId, MutableSet<String>>()

    // Collect all packages that are defined in the source files being compiled
    // to exclude constants from those packages (they're not "missing")
    val sourcePackages = sourceFiles.map { it.packageFqName }.toSet()

    for (sourceFile in sourceFiles) {
      val usedConstants = mutableSetOf<String>()
      val usageSites = mutableListOf<ConstUsageSite>()
      sourceFile.accept(MissingConstantsVisitor(usedConstants, usageSites))

      // Records the annotation-parameter type for a constant we are about to synthesise. Two
      // annotations disagreeing about the type means the evidence is unusable, so the constant is
      // marked ambiguous and falls back rather than picking one arbitrarily.
      fun recordTypeHint(classId: ClassId, propertyName: String, usage: String) {
        val key = constHintKey(classId, propertyName)
        if (key in state.ambiguousConstantTypeHints) return
        val hint =
            usageSites
                .asSequence()
                .filter { it.expressionText == usage }
                .mapNotNull { resolveAnnotationParameterType(it, sourceFile, sourceFiles, session) }
                .firstOrNull() ?: return
        val existing = state.constantTypeHints[key]
        if (existing == null) {
          state.constantTypeHints[key] = hint
        } else if (existing != hint) {
          state.constantTypeHints.remove(key)
          state.ambiguousConstantTypeHints.add(key)
        }
      }

      for (importDirective in sourceFile.importDirectives) {
        val importPath = importDirective.importedFqName ?: continue
        val segments = importPath.pathSegments()
        if (segments.size < 2) continue

        val importedName = importPath.shortNameOrSpecial().asString()

        // Check if this import is used in annotations
        // usedConstants contains things like "THREAD_TAG" or "TestConstants.THREAD_TAG"
        val matchingUsages = usedConstants.filter { it.contains(importedName) }
        if (matchingUsages.isEmpty()) continue

        // Determine if this is a class import or a constant import
        // For class imports like "import pkg.Class", usages will be "Class.PROPERTY"
        // For constant imports like "import pkg.Class.CONSTANT", usages will be "CONSTANT"
        for (usage in matchingUsages) {
          if (usage.startsWith("$importedName.")) {
            // Class import: import pkg.Class, used as Class.PROPERTY
            // Extract the property name from the usage
            val propertyPath = usage.removePrefix("$importedName.")
            // propertyPath could be "PROPERTY" or "Api.PROPERTY" (nested class)
            val pathParts = propertyPath.split(".")
            val propertyName = pathParts.last()

            // Build full segments including nested class parts
            // For import other_package.enums.VersionCodes, used as VersionCodes.Api.O:
            //   segments = [other_package, enums, VersionCodes]
            //   pathParts = [Api, O]
            //   nestedClassParts = [Api] (all except the property name)
            //   fullSegments = [other_package, enums, VersionCodes, Api]
            val nestedClassParts = pathParts.dropLast(1)
            val fullSegments = segments + nestedClassParts.map { Name.identifier(it) }

            // For class imports, the full import path IS the class
            val classId = findClassIdForImportWithProperty(
                fullSegments,
                propertyName,
                session,
                sourcePackages,
                segments.size,
            )
            if (classId != null) {
              missingConstants.getOrPut(classId) { mutableSetOf() }.add(propertyName)
              recordTypeHint(classId, propertyName, usage)
            }
          } else if (usage == importedName) {
            // Constant import: import pkg.Class.CONSTANT, used as CONSTANT
            val classSegments = segments.dropLast(1)
            val classId = findClassIdForImportWithProperty(
                classSegments,
                importedName,
                session,
                sourcePackages,
                classSegments.size,
            )
            if (classId != null) {
              missingConstants.getOrPut(classId) { mutableSetOf() }.add(importedName)
              recordTypeHint(classId, importedName, usage)
            }
          }
        }
      }
    }

    return missingConstants
  }

  // Find the correct ClassId for an import path
  // For import other_package.constants.TestConstants.THREAD_TAG, segments would be
  // [other_package, constants, TestConstants] and we want ClassId(other_package.constants,
  // TestConstants)
  // Returns null if the class (or any parent class in the path) already exists in dependencies AND
  // has the property or enum entry
  @OptIn(SymbolInternals::class)
  private fun findClassIdForImportWithProperty(
      segments: List<Name>,
      propertyName: String,
      session: FirSession,
      sourcePackages: Set<FqName>,
      originalImportLength: Int,
  ): ClassId? {
    if (segments.size < 2) return null

    // Try different package/class splits to find if any class in the path exists in dependencies
    // For import like androidx.annotation.VisibleForTesting.Companion.PROTECTED (segments =
    // [androidx, annotation, VisibleForTesting, Companion])
    // We need to try:
    // - Package: androidx, Class: annotation (unlikely)
    // - Package: androidx.annotation, Class: VisibleForTesting (likely exists!)
    // - Package: androidx.annotation.VisibleForTesting, Class: Companion (won't exist as top-level)
    // Split 0 (root package + full nested path) finds root-packaged nested classes, and
    // no split is skipped for source packages: the dependencies provider cannot see
    // sources, so a hit always means the constant resolves from dependencies.
    for (splitPoint in segments.size - 1 downTo 0) {
      val packageFqName = FqName.fromSegments(segments.take(splitPoint).map { it.asString() })
      val classNameParts = segments.drop(splitPoint)

      // Build the ClassId - for nested classes like VisibleForTesting.Companion,
      // we need to use the proper nested class syntax
      val topLevelClassName = classNameParts.first()
      val classId =
          if (classNameParts.size == 1) {
            ClassId(packageFqName, topLevelClassName)
          } else {
            // For nested classes, create ClassId with nested class path
            ClassId(packageFqName, FqName.fromSegments(classNameParts.map { it.asString() }), false)
          }

      val classSymbol = session.dependenciesSymbolProvider.getClassLikeSymbolByClassId(classId)
      if (classSymbol != null) {
        val firClassSymbol = classSymbol as? FirClassSymbol<*>
        if (firClassSymbol != null) {
          // Check for property with matching name
          // For const vals, also verify the initializer is resolvable (not an error expression)
          // Source-only ABI JARs may have const vals with stripped initializers
          val hasResolvableProperty =
              firClassSymbol.declarationSymbols.filterIsInstance<FirPropertySymbol>().any { prop ->
                if (prop.name.asString() != propertyName) {
                  return@any false
                }

                // If it's not a const, it's usable as-is
                if (!prop.isConst) {
                  return@any true
                }

                // For const vals from library/bytecode dependencies, trust they have valid values
                // even if the initializer is not available in FIR (bytecode doesn't preserve
                // initializer expressions, but the ConstantValue is accessible at runtime)
                if (prop.fir.isFromLibrary) {
                  return@any true
                }

                // For source-compiled const vals, check if we can actually resolve the initializer
                // Const vals from source-only ABI JARs may exist but have no evaluable value
                val initializer = prop.fir.initializer
                initializer != null &&
                    !pipeline.firMetadataSanitizer.hasErrorExpressionRecursive(initializer)
              }

          // Check for any callable with matching name (covers Java static fields)
          // Exclude FirPropertySymbol since those are already handled by hasResolvableProperty
          // above.
          // Properties with non-resolvable initializers (like TODO()) need stubs, so we must not
          // let hasCallable short-circuit the check.
          val hasCallable =
              firClassSymbol.declarationSymbols.filterIsInstance<FirCallableSymbol<*>>().any {
                it.name.asString() == propertyName && it !is FirPropertySymbol
              }

          // Also check for enum entry with matching name
          val firRegularClass = firClassSymbol.fir as? FirRegularClass
          val hasEnumEntry =
              firRegularClass != null &&
                  firRegularClass.classKind == ClassKind.ENUM_CLASS &&
                  firClassSymbol.declarationSymbols.filterIsInstance<FirEnumEntrySymbol>().any {
                    it.name.asString() == propertyName
                  }

          // For annotation classes, constants are Java static fields which may not be
          // represented as FirPropertySymbol. Trust that the constant exists if the
          // annotation class is on the classpath.
          val isAnnotationClass =
              firRegularClass != null && firRegularClass.classKind == ClassKind.ANNOTATION_CLASS

          if (hasResolvableProperty || hasCallable || hasEnumEntry || isAnnotationClass) {
            // Found a class in dependencies that has the member - no stub needed
            return null
          }

          // Check companion object for const vals
          // Companion object const vals are accessible as Class.PROPERTY in Kotlin bytecode,
          // but from source-only ABI JARs, the initializer values may be stripped.
          // We need to check if the companion has the property AND if it's resolvable.
          val companionSymbol =
              firClassSymbol.declarationSymbols.filterIsInstance<FirClassSymbol<*>>().firstOrNull {
                it.name.asString() == "Companion"
              }
          if (companionSymbol != null) {
            val hasResolvableCompanionProperty =
                companionSymbol.declarationSymbols.filterIsInstance<FirPropertySymbol>().any { prop
                  ->
                  if (prop.name.asString() != propertyName) {
                    return@any false
                  }

                  // Non-const properties in companion are usable
                  if (!prop.isConst) {
                    return@any true
                  }

                  // For const vals from library/bytecode dependencies, trust they have valid values
                  if (prop.fir.isFromLibrary) {
                    return@any true
                  }

                  // For source-compiled const vals, check if initializer is resolvable
                  val initializer = prop.fir.initializer
                  initializer != null &&
                      !pipeline.firMetadataSanitizer.hasErrorExpressionRecursive(initializer)
                }
            if (hasResolvableCompanionProperty) {
              // Found resolvable companion property - no stub needed
              return null
            }
          }
        }
        // Class exists but doesn't have the property/entry - continue probing
      }
    }

    // No class found in dependencies with the property - generate a stub using heuristic
    // Use originalImportLength to determine package/class split
    // For import other_package.enums.VersionCodes (originalImportLength=3), used as
    // VersionCodes.Api.O:
    //   segments = [other_package, enums, VersionCodes, Api] (length=4, originalImportLength=3)
    //   packageFqName should be other_package.enums (first originalImportLength-1 segments)
    //   className should be VersionCodes.Api (remaining segments)
    val packageFqName =
        FqName.fromSegments(segments.take(originalImportLength - 1).map { it.asString() })
    val classNameParts = segments.drop(originalImportLength - 1)

    // Skip if from source package or a class in a source package
    // For self-imports like "import test.PermissionType.Companion.CONST" with package "test",
    // packageFqName would be "test.PermissionType" which starts with source package "test".
    // This indicates the constant is defined in source code, not in dependencies.
    if (
        sourcePackages.any { sourcePackage ->
          packageFqName == sourcePackage ||
              packageFqName.asString().startsWith(sourcePackage.asString() + ".")
        }
    ) {
      return null
    }

    // Build the ClassId with potential nested class path
    val topLevelClassName = classNameParts.first()
    val baseClassId =
        if (classNameParts.size == 1) {
          ClassId(packageFqName, topLevelClassName)
        } else {
          // For nested classes, create ClassId with nested class path
          ClassId(packageFqName, FqName.fromSegments(classNameParts.map { it.asString() }), false)
        }

    // If the class already exists in dependencies but doesn't have the property,
    // the property might be in its Companion object. Check if a Companion exists
    // and if so, generate the stub for the Companion class instead to avoid conflicts.
    val existingClass = session.dependenciesSymbolProvider.getClassLikeSymbolByClassId(baseClassId)
    if (existingClass != null) {
      val companionClassId = baseClassId.createNestedClassId(Name.identifier("Companion"))
      val existingCompanion =
          session.dependenciesSymbolProvider.getClassLikeSymbolByClassId(companionClassId)
      if (existingCompanion != null) {
        // Companion exists but doesn't have the property - generate stub in Companion
        return companionClassId
      }
      // Class exists but no Companion - can't generate stub without conflict
      // Return null to avoid generating conflicting class
      return null
    }

    return baseClassId
  }

  // A frontend entry point for Kosabi.
  // Generates FIR for all the sources in the module.
  fun runFrontendForKosabi(
      environment: VfsBasedProjectEnvironment,
      configuration: CompilerConfiguration,
      sources: List<KtFile>,
      module: Module,
  ): FrontendOutputCompat {
    return compileSourceFilesToAnalyzedFirViaPsi(
        sources,
        module.getModuleName(),
        module.getFriendPaths(),
        true,
        environment,
        configuration,
    )!!
  }

  private fun compileSourceFilesToAnalyzedFirViaPsi(
      ktFiles: List<KtFile>,
      rootModuleName: String,
      friendPaths: List<String>,
      ignoreErrors: Boolean = false,
      projectEnvironment: VfsBasedProjectEnvironment,
      configuration: CompilerConfiguration,
  ): FrontendOutputCompat? {

    val sourceScope =
        projectEnvironment.getSearchScopeByPsiFiles(ktFiles) +
            projectEnvironment.getSearchScopeForProjectJavaSources()

    val (librariesScope, providerAndScopeForIncrementalCompilation) =
        prepareLibrariesScopeAndIncrementalContextCompat(
            projectEnvironment,
            configuration,
            sourceScope,
        )
    val sessionsWithSources = prepareJvmSessionsCompat(
        ktFiles,
        rootModuleName,
        friendPaths,
        librariesScope,
        configuration,
        projectEnvironment,
        providerAndScopeForIncrementalCompilation,
    )

    val outputs = sessionsWithSources.map { (session, sources) ->
      val state = session.jvmAbiGenService.state
      val missingConstants = collectMissingConstantsFromSourceFiles(sources, session, state)
      state.missingConstants.putAll(missingConstants)
      // Skip checkers - ABI generation only needs resolved types, and third-party
      // plugin checkers (like Litho K2) crash on unresolved references from stubs.
      val firFiles = session.buildFirFromKtFiles(sources)
      val (scopeSession, fir) = session.runResolution(firFiles)
      // Constants are synthesised lazily during resolution, so this is the earliest point at
      // which the full set is known. Reporting here rather than in ValidationStage keeps the
      // signal even when a later phase fails.
      reportFabricatedConstants(state, configuration)
      moduleFrontendOutputCompat(session, scopeSession, fir)
    }

    return frontendOutputCompat(outputs)
  }

  private fun reportFabricatedConstants(state: AbiGenState, configuration: CompilerConfiguration) {
    if (state.fabricatedConstants.isEmpty()) return
    pipeline.repairLog.fabricatedConstants.addAll(state.fabricatedConstants)

    // A fabricated constant is never correct under any policy: the placeholder cannot match the
    // real declaration, so this runs unconditionally rather than behind `abiValidationMode`.
    for (constant in state.fabricatedConstants) {
      configuration.messageCollector.report(
          CompilerMessageSeverity.ERROR,
          "Kosabi source-only ABI: the value of constant `${constant.classId}.${constant.name}` " +
              "is not available, because no target on the source-only ABI classpath declares it. " +
              "A placeholder of type `${constant.emittedType}` " +
              "(${if (constant.typeSource == ConstTypeSource.ANNOTATION_PARAMETER)
                  "type taken from the annotation parameter it is passed to"
                else "type unknown, assumed String"}) " +
              "was emitted instead, so this ABI does not match a class-ABI build. " +
              "To fix, add the target that declares the constant to THIS target's " +
              "`source_only_abi_deps`. Reach for `required_for_source_only_abi = True` on the " +
              "declaring target only when many consumers need it: that is a global edge which " +
              "puts its ABI on the source-only ABI classpath of every consumer, not just this " +
              "one.",
      )
    }
  }
}

class AbiGenFirExtensionRegistrar() : FirExtensionRegistrar() {
  override fun ExtensionRegistrarContext.configurePlugin() {
    +{ session: FirSession -> MissingConstantDeclarationGenerationExtension(session) }
    +JvmAbiGenService.getFactory()
  }
}

@OptIn(ExperimentalTopLevelDeclarationsGenerationApi::class)
class MissingConstantDeclarationGenerationExtension(
    session: FirSession,
) : FirDeclarationGenerationExtension(session) {

  override fun getTopLevelClassIds(): Set<ClassId> {
    val missingConstants = session.jvmAbiGenService.state.missingConstants
    return missingConstants.keys.toSet()
  }

  override fun getTopLevelCallableIds(): Set<CallableId> {
    val missingConstants = session.jvmAbiGenService.state.missingConstants
    return missingConstants
        .flatMap { (classId, constants) ->
          constants.map { constantName -> CallableId(classId, Name.identifier(constantName)) }
        }
        .toSet()
  }

  override fun generateTopLevelClassLikeDeclaration(classId: ClassId): FirClassLikeSymbol<*>? {
    val missingConstants = session.jvmAbiGenService.state.missingConstants

    if (classId !in missingConstants) {
      return null
    }

    val createdClass = createTopLevelClass(classId, JvmAbiGenPlugin, ClassKind.OBJECT)
    createdClass.replaceStatus(
        FirResolvedDeclarationStatusImpl(
            Visibilities.Public,
            Modality.FINAL,
            EffectiveVisibility.Public,
        )
            .apply { isStatic = true },
    )
    return createdClass.symbol
  }

  @OptIn(SymbolInternals::class)
  override fun getCallableNamesForClass(
      classSymbol: FirClassSymbol<*>,
      context: MemberGenerationContext,
  ): Set<Name> {
    val classId = classSymbol.classId
    val state = session.jvmAbiGenService.state

    // Existing code for missing constants
    val missingConstants = state.missingConstants
    val constantNames =
        (missingConstants[classId] ?: emptySet()).map { Name.identifier(it) }.toSet()

    // Detect methods from internal interfaces
    val internalMethodNames = mutableSetOf<Name>()
    val classDecl = classSymbol.fir as? FirRegularClass

    if (classDecl != null) {
      // Check if this class is public
      val classVisibility = classDecl.status.visibility
      if (classVisibility == Visibilities.Public || classVisibility == Visibilities.Protected) {
        // Find internal interface supertypes
        val interfaceMethodInfos = mutableListOf<InterfaceMethodInfo>()

        for (superTypeRef in classDecl.superTypeRefs) {
          val coneType =
              (superTypeRef as? FirResolvedTypeRef)?.coneType as? ConeClassLikeType ?: continue
          val superClassId = coneType.lookupTag.classId
          val superSymbol =
              session.symbolProvider.getClassLikeSymbolByClassId(superClassId) as? FirClassSymbol<*>
                  ?: continue
          val superClass = superSymbol.fir as? FirRegularClass ?: continue

          // Check if interface is internal/private
          val superVisibility = superClass.status.visibility
          val isInternal =
              superVisibility != Visibilities.Public && superVisibility != Visibilities.Protected

          if (isInternal && superClass.classKind == ClassKind.INTERFACE) {
            // Collect methods from this internal interface
            for (decl in superClass.declarations) {
              if (decl is FirNamedFunctionCompat) {
                val visibility = decl.status.visibility
                if (visibility == Visibilities.Public || visibility == Visibilities.Protected) {
                  // Check if method is not already declared in the class
                  val alreadyDeclaredInClass =
                      classDecl.declarations.any {
                        it is FirNamedFunctionCompat && it.name == decl.name
                      }

                  // For enums, also check if method is overridden in any enum entry
                  val alreadyDeclaredInEnumEntry =
                      classDecl.classKind == ClassKind.ENUM_CLASS &&
                          classDecl.declarations.any { entry ->
                            if (entry is FirEnumEntry) {
                              val entryInit = entry.initializer as? FirAnonymousObjectExpression
                              entryInit?.anonymousObject?.declarations?.any {
                                it is FirNamedFunctionCompat && it.name == decl.name
                              } ?: false
                            } else {
                              false
                            }
                          }

                  val alreadyDeclared = alreadyDeclaredInClass || alreadyDeclaredInEnumEntry
                  if (!alreadyDeclared) {
                    internalMethodNames.add(decl.name)
                    interfaceMethodInfos.add(
                        InterfaceMethodInfo(
                            name = decl.name,
                            returnType = decl.returnTypeRef.coneType,
                            valueParameters =
                                decl.valueParameters.map { param ->
                                  param.name to param.returnTypeRef.coneType
                                },
                        ),
                    )
                  }
                }
              }
            }
          }
        }

        // Store for later generation
        if (interfaceMethodInfos.isNotEmpty()) {
          state.internalInterfaceMethods[classId] = interfaceMethodInfos.toMutableList()
        }
      }
    }

    return constantNames + internalMethodNames
  }

  override fun generateProperties(
      callableId: CallableId,
      context: MemberGenerationContext?,
  ): List<FirPropertySymbol> {
    val owner = context?.owner ?: return emptyList()
    val classId = callableId.classId ?: return emptyList()

    val missingConstants = session.jvmAbiGenService.state.missingConstants
    val constantsForClass = missingConstants[classId] ?: return emptyList()

    val constantName = callableId.callableName.asString()
    if (constantName !in constantsForClass) {
      return emptyList()
    }

    return listOf(generateConstantProperty(constantName, owner))
  }

  override fun generateFunctions(
      callableId: CallableId,
      context: MemberGenerationContext?,
  ): List<FirNamedFunctionSymbol> {
    val owner = context?.owner ?: return emptyList()
    val classId = callableId.classId ?: return emptyList()

    val state = session.jvmAbiGenService.state
    val methodInfos = state.internalInterfaceMethods[classId] ?: return emptyList()

    val methodName = callableId.callableName
    val methodInfo = methodInfos.find { it.name == methodName } ?: return emptyList()

    // Generate the function using the plugin utility
    val function = createMemberFunction(
        owner,
        JvmAbiGenPlugin,
        methodName,
        methodInfo.returnType,
    ) {
      // Add value parameters
      for ((paramName, paramType) in methodInfo.valueParameters) {
        valueParameter(paramName, paramType)
      }
    }

    // Set status: public and final (required for enum implementations)
    function.replaceStatus(
        FirResolvedDeclarationStatusImpl(
            Visibilities.Public,
            Modality.FINAL,
            EffectiveVisibility.Public,
        ),
    )

    return listOf(function.symbol)
  }

  override fun hasPackage(packageFqName: FqName): Boolean {
    val missingConstants = session.jvmAbiGenService.state.missingConstants

    // Return true for any package that contains classes we're generating
    return missingConstants.keys.any { classId -> classId.packageFqName == packageFqName }
  }

  /**
   * Synthesises a constant whose declaring class is not on the source-only ABI classpath.
   *
   * The real value is unrecoverable here by construction: the constant is being synthesised
   * precisely because nothing on the classpath declares it, and Kosabi never reads the provider's
   * sources. Only a placeholder can be emitted, so any constant reaching this function makes the
   * resulting ABI differ from a class-ABI build. The correct repair is to put the declaring target
   * on the consuming target's source-only ABI classpath, by adding it to that target's
   * `source_only_abi_deps`; this function only limits the damage in the meantime and records it for
   * [AbiGenRepairLog].
   *
   * The type, unlike the value, is often recoverable - see [resolveAnnotationParameterType]. Using
   * it matters: emitting `String` for an `Int` constant makes constant evaluation fail outright
   * (`ClassCastException` in `IrConstAnnotationTransformer`) rather than merely produce a wrong
   * number.
   */
  private fun generateConstantProperty(
      constantName: String,
      owner: FirClassSymbol<*>,
  ): FirPropertySymbol {
    val state = session.jvmAbiGenService.state
    val key = constHintKey(owner.classId, constantName)
    val hint = state.constantTypeHints[key]

    val property = createMemberProperty(
        owner,
        JvmAbiGenPlugin,
        Name.identifier(constantName),
        hint?.coneType(session) ?: session.builtinTypes.stringType.coneType,
    )
    property.replaceStatus(
        FirResolvedDeclarationStatusImpl(
            Visibilities.Public,
            Modality.FINAL,
            EffectiveVisibility.Public,
        )
            .apply { isConst = true },
    )
    property.replaceInitializer(
        buildLiteralExpression(
            source = null,
            kind = hint?.constantValueKind ?: ConstantValueKind.String,
            value = hint?.placeholderValue ?: "",
            setType = true,
        ),
    )

    state.fabricatedConstants.add(
        FabricatedConstant(
            classId = owner.classId.asString(),
            name = constantName,
            emittedType = hint?.typeName ?: "String",
            typeSource =
                if (hint != null) ConstTypeSource.ANNOTATION_PARAMETER
                else ConstTypeSource.ASSUMED_STRING,
        ),
    )
    return property.symbol
  }
}

/** A constant type Kosabi is able to synthesise a placeholder literal for. */
enum class ConstTypeHint(
    val typeName: String,
    val constantValueKind: ConstantValueKind,
    val placeholderValue: Any,
) {
  BOOLEAN("Boolean", ConstantValueKind.Boolean, false),
  BYTE("Byte", ConstantValueKind.Byte, 0.toByte()),
  SHORT("Short", ConstantValueKind.Short, 0.toShort()),
  INT("Int", ConstantValueKind.Int, 0),
  LONG("Long", ConstantValueKind.Long, 0L),
  FLOAT("Float", ConstantValueKind.Float, 0.0f),
  DOUBLE("Double", ConstantValueKind.Double, 0.0),
  CHAR("Char", ConstantValueKind.Char, '\u0000'),
  STRING("String", ConstantValueKind.String, "");

  fun coneType(session: FirSession): ConeKotlinType =
      when (this) {
        BOOLEAN -> session.builtinTypes.booleanType.coneType
        BYTE -> session.builtinTypes.byteType.coneType
        SHORT -> session.builtinTypes.shortType.coneType
        INT -> session.builtinTypes.intType.coneType
        LONG -> session.builtinTypes.longType.coneType
        FLOAT -> session.builtinTypes.floatType.coneType
        DOUBLE -> session.builtinTypes.doubleType.coneType
        CHAR -> session.builtinTypes.charType.coneType
        STRING -> session.builtinTypes.stringType.coneType
      }

  companion object {
    fun bySimpleName(name: String): ConstTypeHint? = entries.firstOrNull { it.typeName == name }
  }
}

internal fun constHintKey(classId: ClassId, constantName: String): String =
    "${classId.asString()}.$constantName"

/**
 * Maps a type as written in source to a constant kind.
 *
 * Array types are unwrapped once because no constant can itself be an array - an array-typed
 * annotation parameter is always fed element-wise. Anything not recognised (a typealias, an enum, a
 * generic) yields null so the caller falls back instead of guessing.
 */
internal fun constTypeHintFromTypeText(typeText: String?): ConstTypeHint? {
  var text = typeText?.trim()?.removeSuffix("?")?.trim() ?: return null
  if (text.endsWith(">")) {
    val open = text.indexOf('<')
    if (open <= 0) return null
    if (text.take(open).substringAfterLast('.') != "Array") return null
    text = text.substring(open + 1, text.length - 1).removePrefix("out ").trim()
  }
  val simpleName = text.substringAfterLast('.').removeSuffix("?").trim()
  ConstTypeHint.bySimpleName(simpleName)?.let {
    return it
  }
  return when (simpleName) {
    "BooleanArray" -> ConstTypeHint.BOOLEAN
    "ByteArray" -> ConstTypeHint.BYTE
    "ShortArray" -> ConstTypeHint.SHORT
    "IntArray" -> ConstTypeHint.INT
    "LongArray" -> ConstTypeHint.LONG
    "FloatArray" -> ConstTypeHint.FLOAT
    "DoubleArray" -> ConstTypeHint.DOUBLE
    "CharArray" -> ConstTypeHint.CHAR
    else -> null
  }
}

/** FIR counterpart of [constTypeHintFromTypeText], for annotations coming from dependencies. */
internal fun constTypeHintFromConeType(type: ConeKotlinType): ConstTypeHint? {
  val classId = (type as? ConeClassLikeType)?.lookupTag?.classId ?: return null
  val fqName = classId.asFqNameString()
  if (fqName == "kotlin.Array") {
    val elementType =
        (type.typeArguments.firstOrNull() as? ConeKotlinTypeProjection)?.type ?: return null
    return constTypeHintFromConeType(elementType)
  }
  return constTypeHintFromTypeText(fqName.takeIf { it.startsWith("kotlin.") })
}

class JvmAbiGenService(session: FirSession, state: AbiGenState) :
    FirExtensionSessionComponent(session) {
  companion object {
    fun getFactory(): Factory {
      return Factory { JvmAbiGenService(it, AbiGenState()) }
    }
  }

  val state: AbiGenState = state
}

class AbiGenState {
  val missingConstants: MutableMap<ClassId, Set<String>> = mutableMapOf()

  /**
   * Declared type of each synthesised constant, keyed by [constHintKey], recovered from the
   * annotation parameter it is passed to. Absent means the type could not be established.
   */
  val constantTypeHints: MutableMap<String, ConstTypeHint> = mutableMapOf()

  /** Constants whose usages disagreed about the type; treated the same as having no hint. */
  val ambiguousConstantTypeHints: MutableSet<String> = mutableSetOf()

  /** Every constant synthesised during this compilation. See [AbiGenRepairLog]. */
  val fabricatedConstants: MutableList<FabricatedConstant> = mutableListOf()
  // Track methods from internal interfaces that need to be generated for classes
  // Key: owning class's ClassId, Value: List of interface method details
  val internalInterfaceMethods: MutableMap<ClassId, MutableList<InterfaceMethodInfo>> =
      mutableMapOf()
}

val FirSession.jvmAbiGenService: JvmAbiGenService by FirSession.sessionComponentAccessor()
