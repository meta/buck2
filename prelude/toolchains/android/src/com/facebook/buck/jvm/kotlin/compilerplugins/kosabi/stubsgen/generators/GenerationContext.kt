/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators

import com.facebook.kotlin.compilerplugins.kosabi.common.FullTypeQualifier
import com.facebook.kotlin.compilerplugins.kosabi.common.parseClasspathFileClassesAndPackages
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.stub.container.StubsContainer
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.stub.container.StubsContainerImpl
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.JavaSourceInfo
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.KotlinClasspathDeclarations
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.calculateQualifierList
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.declaredTypes
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.toImportedClass
import java.io.File
import java.util.IdentityHashMap
import org.jetbrains.kotlin.com.intellij.psi.PsiElement
import org.jetbrains.kotlin.lexer.KtTokens
import org.jetbrains.kotlin.psi.KtAnnotationEntry
import org.jetbrains.kotlin.psi.KtCallExpression
import org.jetbrains.kotlin.psi.KtClass
import org.jetbrains.kotlin.psi.KtClassBody
import org.jetbrains.kotlin.psi.KtClassOrObject
import org.jetbrains.kotlin.psi.KtDelegatedSuperTypeEntry
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.psi.KtImportDirective
import org.jetbrains.kotlin.psi.KtNameReferenceExpression
import org.jetbrains.kotlin.psi.KtNullableType
import org.jetbrains.kotlin.psi.KtParameterList
import org.jetbrains.kotlin.psi.KtSecondaryConstructor
import org.jetbrains.kotlin.psi.KtSuperTypeCallEntry
import org.jetbrains.kotlin.psi.KtSuperTypeEntry
import org.jetbrains.kotlin.psi.KtSuperTypeList
import org.jetbrains.kotlin.psi.KtSuperTypeListEntry
import org.jetbrains.kotlin.psi.KtTreeVisitorVoid
import org.jetbrains.kotlin.psi.KtTypeAlias
import org.jetbrains.kotlin.psi.KtTypeParameter
import org.jetbrains.kotlin.psi.KtTypeParameterListOwner
import org.jetbrains.kotlin.psi.KtTypeReference
import org.jetbrains.kotlin.psi.KtUserType
import org.jetbrains.kotlin.psi.psiUtil.getChildOfType
import org.jetbrains.kotlin.psi.psiUtil.getChildrenOfType
import org.jetbrains.kotlin.psi.psiUtil.getStrictParentOfType
import org.jetbrains.kotlin.psi.psiUtil.referenceExpression

class GenerationContext {
  val projectFiles: Collection<KtFile>
  val stubsContainer: StubsContainer
  /**
   * Classpath-provided declarations the entry-name scan cannot see: classes in its skipped jars and
   * Kotlin top-level declarations (typealiases) recorded only in file facades. Consulted before
   * fabricating a class stub for an import; lazily built, so merely constructing it costs nothing
   * on modules with no such imports.
   */
  val kotlinClasspathDeclarations: KotlinClasspathDeclarations
  val importedDeclarations: Set<FullTypeQualifier>
  val importedTypes: Set<FullTypeQualifier>

  /**
   * Explicit Java imports, creation-only: stub generators may create stubs from them, but they must
   * never resolve a Kotlin usage or suppress same-package fabrication — a Java-only import would
   * otherwise claim a Kotlin simple name the Kotlin file never imported. Hence only
   * [PrimitiveStubsGenerator] and the import-driven half of [InnerClassStubsGenerator] read this.
   */
  val javaImportedTypes: Set<FullTypeQualifier>

  /** Java fully-qualified usages; creation-only, like [javaImportedTypes]. */
  val javaFullQualifierTypes: Set<FullTypeQualifier>

  /**
   * Each type a Java source writes with type arguments through a name that only a single-type
   * import can bind, resolved through that import, with its type-argument count. Read only to give
   * an existing stub its arity.
   */
  val javaGenericTypeUsages: Set<Pair<FullTypeQualifier, Int>>

  /**
   * [importedTypes] split by the file that wrote the import. A usage must be attributed against its
   * own file's imports: the pooled set resolves a simple name to whichever file imported it first,
   * which is the wrong type as soon as two files import different types of the same name.
   */
  val importedTypesByFile: Map<KtFile, Set<FullTypeQualifier>>

  /**
   * [isCompanionCallableImport], precomputed: probing per candidate would rescan every file's
   * imports.
   */
  val companionCallableImports: Set<FullTypeQualifier>

  /**
   * This contains external types outside from the local source codes, etc: classpath, code
   * generation
   */
  val externalTypeReferences: Set<FullTypeQualifier>
  val knownGeneratedTypes: Set<FullTypeQualifier>
  val pkgsInClasspath: Set<List<String>>
  val interfaceTypes: List<KtUserType>

  /**
   * The entries of [interfaceTypes] that may be a superclass: the first supertype of a class that
   * has secondary constructors, no primary constructor and no constructor call in its supertype
   * list, when no secondary constructor delegates to `super` or takes no parameters. Kotlin writes
   * the superclass of such a class without parentheses, so the class alone does not say whether the
   * entry is a class or an interface.
   */
  val ambiguousInterfaceTypes: Set<KtUserType>

  /**
   * Supertypes written as constructor calls (`: A()`), which only a class can be. Only calls whose
   * name no declaration of the file and no containing class's supertype can shadow are kept: the
   * entry's class has no containing class with a supertype (whose nested classes, inherited ones
   * included, would be in scope), and no classifier, type alias or type parameter of the file has
   * the call's first name.
   */
  val superclassCallTypes: List<KtUserType>
  // Each entry is one multi-bound type parameter's bounds, in source order.
  val multiBoundGroups: List<List<KtUserType>>
  val annotationEntries: List<KtAnnotationEntry>
  val declaredTypes: Set<FullTypeQualifier>
  val fullQualifierTypes: Set<FullTypeQualifier>

  /**
   * [fullQualifierTypes] split by the file that wrote the qualifier. Same-package fabrication must
   * land in the using file's own package: the module's files can span packages.
   */
  val fullQualifierTypesByFile: Map<KtFile, Set<FullTypeQualifier>>
  val importAlias: Set<String>

  /**
   * Alias name -> the type `import a.B as Alias` names. An import's [FullTypeQualifier] carries the
   * imported FQN, whose last name is always the real name `B`, so a usage written `Alias` matches
   * no candidate by name and would otherwise resolve to nothing.
   */
  val importAliasQualifiers: Map<String, FullTypeQualifier>

  /** [importAliasQualifiers] split by the file that wrote the alias: an alias binds only there. */
  val importAliasQualifiersByFile: Map<KtFile, Map<String, FullTypeQualifier>>

  /**
   * Per file, the segments of the imports that the file writes only under an alias. `import a.B as
   * Alias` binds `Alias` alone, so in that file the plain `B` never resolves through `a.B`.
   */
  val aliasOnlyImportsByFile: Map<KtFile, Set<List<String>>>

  /**
   * The segments of the imports that every file writing them renames: no file of the module binds
   * their plain name.
   */
  val aliasOnlyImports: Set<List<String>>

  /**
   * Per file, the plain names that its renaming imports give up: the last name of each import the
   * file writes under an alias, unless the file also imports something of that name without one.
   * Such a name means whatever it would mean without the import, a type of the file's own package
   * included.
   */
  val aliasHiddenNamesByFile: Map<KtFile, Set<String>>
  val typeAliasSymbol: Set<String>
  val parameterNames: Set<String>

  // Element: (type simple name -> num of ctor args)
  val typeValueArgs: Map<String, Int>
  val usedUserTypes: Set<KtUserType>

  /**
   * Each candidate collection passed to [resolveImportedType], grouped by last name. A generator
   * builds its candidates once and resolves every usage against them, so the grouping is reused
   * instead of scanning all candidates per usage. Candidate collections are not changed after they
   * are built.
   */
  private val candidatesByLastName =
      IdentityHashMap<Collection<FullTypeQualifier>, Map<String, List<FullTypeQualifier>>>()

  constructor(
      projectFiles: Collection<KtFile>,
      classpath: List<File>,
      knownGeneratedTypes: Set<FullTypeQualifier> = emptySet(),
      lightweight: Boolean = true,
      javaSources: Collection<JavaSourceInfo> = emptyList(),
  ) {
    this.projectFiles = projectFiles
    this.stubsContainer = StubsContainerImpl()
    this.kotlinClasspathDeclarations = KotlinClasspathDeclarations(classpath)
    this.knownGeneratedTypes = knownGeneratedTypes

    if (lightweight) {
      this.externalTypeReferences = emptySet()
      this.pkgsInClasspath = emptySet()
      this.importedDeclarations = emptySet()
      this.importedTypes = emptySet()
      this.javaImportedTypes = emptySet()
      this.javaFullQualifierTypes = emptySet()
      this.javaGenericTypeUsages = emptySet()
      this.importedTypesByFile = emptyMap()
      this.companionCallableImports = emptySet()
      this.interfaceTypes = emptyList()
      this.ambiguousInterfaceTypes = emptySet()
      this.superclassCallTypes = emptyList()
      this.multiBoundGroups = emptyList()
      this.annotationEntries = emptyList()
      this.declaredTypes = emptySet()
      this.typeValueArgs = emptyMap<String, Int>()
      this.importAlias = emptySet()
      this.importAliasQualifiers = emptyMap()
      this.importAliasQualifiersByFile = emptyMap()
      this.aliasOnlyImportsByFile = emptyMap()
      this.aliasOnlyImports = emptySet()
      this.aliasHiddenNamesByFile = emptyMap()
      this.fullQualifierTypes = emptySet()
      this.fullQualifierTypesByFile = emptyMap()
      this.usedUserTypes = emptySet()
      this.typeAliasSymbol = emptySet()
      this.parameterNames = emptySet()
    } else {
      val importsByFile = projectFiles.associateWith { it.importList?.imports ?: emptyList() }
      val importDirectives = importsByFile.values.flatten()
      this.importedDeclarations = importDirectives.mapNotNull { it.toImportedClass() }.toSet()
      this.importAlias = importDirectives.mapNotNull { it.aliasName }.toSet()
      val rawImportedTypes = importedDeclarations.filterNot { it.isTopLevelDeclaration() }.toSet()
      // `importedTypes` and `importedTypesByFile` are assigned after the traversal below: both
      // gain the usage-proven class imports, which need the collected type usages.
      this.importAliasQualifiers =
          importDirectives
              .mapNotNull { directive ->
                directive.aliasName?.let { alias -> alias to directive.toImportedClass() }
              }
              .toMap()
      this.importAliasQualifiersByFile = importsByFile.mapValues { (_, imports) ->
        imports
            .mapNotNull { directive ->
              directive.aliasName?.let { alias -> alias to directive.toImportedClass() }
            }
            .toMap()
      }
      // `import a.B as B` keeps its plain name; any other alias gives it up.
      val explicitByFile = importsByFile.mapValues { (_, imports) ->
        imports.filterNot { it.isAllUnder }
      }
      val renamingByFile = explicitByFile.mapValues { (_, imports) ->
        imports.filter { it.renames() }
      }
      val plainByFile = explicitByFile.mapValues { (_, imports) ->
        imports.filterNot { it.renames() }
      }
      this.aliasOnlyImportsByFile =
          importsByFile.keys.associateWith { file ->
            val plain =
                plainByFile.getValue(file).mapTo(mutableSetOf()) {
                  it.toImportedClass().segments
                }
            renamingByFile.getValue(file).mapTo(mutableSetOf()) { it.toImportedClass().segments } -
                plain
          }
      val plainAnywhere =
          plainByFile.values.flatten().mapTo(mutableSetOf()) { it.toImportedClass().segments }
      this.aliasOnlyImports = aliasOnlyImportsByFile.values.flatten().toSet() - plainAnywhere
      this.aliasHiddenNamesByFile =
          importsByFile.keys.associateWith { file ->
            val plain = plainByFile.getValue(file).mapNotNullTo(mutableSetOf()) { it.plainName() }
            renamingByFile.getValue(file).mapNotNullTo(mutableSetOf()) { it.plainName() } - plain
          }

      val dataInClasspath: Pair<Set<FullTypeQualifier>, Set<List<String>>> =
          parseClasspathFileClassesAndPackages(classpath)
      externalTypeReferences = dataInClasspath.first + knownGeneratedTypes
      pkgsInClasspath = dataInClasspath.second

      // declaredTypes is a structural getChildrenOfType recursion (it intentionally excludes
      // local/anonymous classes in function bodies), so it stays out of the single-pass DFS below.
      // Java-declared types join so imports of same-module Java classes are not stubbed.
      this.declaredTypes =
          projectFiles.flatMap { it.declaredTypes() }.toSet() +
              javaSources.flatMap { it.declaredTypes }.toSet()
      this.javaImportedTypes =
          javaSources.flatMap { it.imports }.filterNot { it.isTopLevelDeclaration() }.toSet()
      this.javaFullQualifierTypes =
          javaSources.flatMap { it.fullQualifierTypes }.filterNot { it.isSdkQualifier() }.toSet()
      this.javaGenericTypeUsages =
          javaSources.flatMapTo(mutableSetOf()) { source ->
            source.genericTypeUsages.mapNotNull { (written, arity) ->
              source.qualifierOf(written)?.let { it to arity }
            }
          }

      // Single-pass PSI traversal: one DFS per file collects everything the generators need,
      // replacing 9 separate whole-tree collectDescendantsOfType walks. Semantics are preserved
      // 1:1 — same node sets, same filters, and typeValueArgs keeps its per-file
      // superCall -> callExpr -> class ordering so the last-key-wins toMap is unchanged.
      val annotations = mutableListOf<KtAnnotationEntry>()
      val aliasNames = mutableSetOf<String>()
      val paramNames = mutableSetOf<String>()
      val usedTypes = mutableSetOf<KtUserType>()
      val multiSegByFile = mutableMapOf<KtFile, MutableList<List<String>>>()
      val interfaceUserTypes = mutableListOf<KtUserType>()
      val ambiguousInterfaceUserTypes = mutableSetOf<KtUserType>()
      val superclassCallUserTypes = mutableListOf<KtUserType>()
      val multiBoundUserTypes = mutableListOf<List<KtUserType>>()
      val typeValueArgPairs = mutableListOf<Pair<String, Int>>()
      for (ktFile in projectFiles) {
        val superCallPairs = mutableListOf<Pair<String, Int>>()
        val callExprPairs = mutableListOf<Pair<String, Int>>()
        val classCtorPairs = mutableListOf<Pair<String, Int>>()
        val fileClassifierNames = mutableSetOf<String>()
        val fileSuperclassCalls = mutableListOf<KtUserType>()
        ktFile.accept(
            object : KtTreeVisitorVoid() {
              override fun visitElement(element: PsiElement) {
                if (element is KtAnnotationEntry) {
                  annotations.add(element)
                }
                if (element is KtTypeAlias) {
                  element.name?.let {
                    aliasNames.add(it)
                    fileClassifierNames.add(it)
                  }
                }
                if (element is KtTypeParameter) {
                  element.name?.let {
                    paramNames.add(it)
                    fileClassifierNames.add(it)
                  }
                }
                if (element is KtUserType && element.calculateQualifierList().size == 1) {
                  usedTypes.add(element)
                }
                if (element is KtTypeReference) {
                  element.userTypeForQualifier()?.calculateQualifierList()?.let { qualifier ->
                    if (qualifier.size >= 2) {
                      multiSegByFile.getOrPut(ktFile) { mutableListOf() }.add(qualifier)
                    }
                  }
                }
                if (element is KtClassOrObject) {
                  element.name?.let { fileClassifierNames.add(it) }
                  interfaceUserTypes.addAll(
                      element.getInterfaceTypes().mapNotNull { it.typeAsUserType },
                  )
                  element.ambiguousInterfaceEntry()?.typeAsUserType?.let {
                    ambiguousInterfaceUserTypes.add(it)
                  }
                }
                if (element is KtTypeParameterListOwner) {
                  multiBoundUserTypes.addAll(element.multiBoundGroups())
                }
                if (element is KtSuperTypeCallEntry) {
                  element.typeValueArgs()?.let { superCallPairs.add(it) }
                  if (element.resolvesInFileScope()) {
                    element.typeAsUserType?.let { fileSuperclassCalls.add(it) }
                  }
                }
                if (element is KtCallExpression) {
                  element.typeValueArgs()?.let { callExprPairs.add(it) }
                }
                if (element is KtClass) {
                  element.typeValueArgsFromSuperSecondaryConstructor()?.let {
                    classCtorPairs.add(it)
                  }
                }
                super.visitElement(element)
              }
            },
        )
        fileSuperclassCalls.filterTo(superclassCallUserTypes) {
          it.calculateQualifierList().first() !in fileClassifierNames
        }
        typeValueArgPairs.addAll(superCallPairs)
        typeValueArgPairs.addAll(callExprPairs)
        typeValueArgPairs.addAll(classCtorPairs)
      }

      this.annotationEntries = annotations
      this.typeAliasSymbol = aliasNames
      this.parameterNames = paramNames
      this.usedUserTypes = usedTypes
      this.interfaceTypes = interfaceUserTypes
      this.ambiguousInterfaceTypes = ambiguousInterfaceUserTypes
      this.superclassCallTypes = superclassCallUserTypes
      this.multiBoundGroups = multiBoundUserTypes
      fun qualifiersOf(quals: List<List<String>>): Set<FullTypeQualifier> =
          quals
              .distinct()
              .map { FullTypeQualifier(it).withMemberAsNestedName() }
              .filterNot { it.isSdkQualifier() }
              .toSet()
      this.fullQualifierTypesByFile = multiSegByFile.mapValues { (_, quals) -> qualifiersOf(quals) }
      this.fullQualifierTypes = fullQualifierTypesByFile.values.flatten().toSet()
      // An all-caps simple name (`IABJSOTA`, `OTA`, `URI`) parses as a static-const member, so
      // its import carries no class name and drops out of the raw type sets above. A member used
      // in type position is really a class: promote those imports, module-wide and per file, so
      // every consumer resolves the owner. A member never used as a type stays out (top-level
      // consts and funs).
      this.importedTypes = rawImportedTypes + promoteProvenClassImports(importedDeclarations)
      this.importedTypesByFile = importsByFile.mapValues { (_, imports) ->
        val quals = imports.mapNotNull { it.toImportedClass() }
        quals.filterNot { it.isTopLevelDeclaration() }.toSet() + promoteProvenClassImports(quals)
      }
      this.companionCallableImports =
          importedTypesByFile.values.flatMapTo(mutableSetOf()) { imports ->
            val classNames =
                imports.mapNotNullTo(mutableSetOf()) { other ->
                  other.names.lastOrNull()?.takeUnless { other.isThroughCompanion() }
                }
            imports.filter { imp ->
              imp.isThroughCompanion() && imp.names.lastOrNull() in classNames
            }
          }
      // TODO: We might have multiple ctors for each type.
      // For now we're just skipping to have one (the default)
      this.typeValueArgs = typeValueArgPairs.toMap()
    }
  }

  // This will handle the super call entry for the ctor params
  private fun KtSuperTypeCallEntry.typeValueArgs(): Pair<String, Int>? {
    typeAsUserType?.referencedName?.let {
      return it to valueArguments.size
    }
    return null
  }

  // This will handle the call expression for the ctor params
  private fun KtCallExpression.typeValueArgs(): Pair<String, Int>? {
    (referenceExpression() as? KtNameReferenceExpression)?.getReferencedName()?.let {
      return it to valueArguments.size
    }
    return null
  }

  // This will handle secondary constructor calls with super delegation call
  private fun KtClass.typeValueArgsFromSuperSecondaryConstructor(): Pair<String, Int>? {
    val superTypeList = this.getChildOfType<KtSuperTypeList>()
    val superTypeCallEntry = superTypeList?.getChildOfType<KtSuperTypeCallEntry>()
    val superTypeEntries = superTypeList?.getChildrenOfType<KtSuperTypeEntry>()

    if (
        superTypeList == null ||
            superTypeEntries.isNullOrEmpty() ||
            isInterface() ||
            superTypeCallEntry != null
    ) {
      return null
    }
    val clazzBody = this.getChildOfType<KtClassBody>()
    val superSecondaryConstructor = clazzBody?.getSecondaryConstructor()?.superConstructor()
    if (superSecondaryConstructor?.isNotEmpty() == true) {
      val referenceName = superTypeEntries.first().typeAsUserType?.referencedName
      referenceName?.let {
        return it to superSecondaryConstructor.first().getDelegationCall().valueArguments.size
      }
    }
    return null
  }

  /**
   * This filters out possible class candidate while supporting delegation
   * 1. Class with default constructor at class body, assume first super type call entry is class
   * 2. Class with constructor delegation call has callee "super"
   */
  private fun KtClassOrObject.getInterfaceTypes(): List<KtSuperTypeListEntry> {
    val superTypeList = this.getChildOfType<KtSuperTypeList>()
    val superTypeCallEntry = superTypeList?.getChildOfType<KtSuperTypeCallEntry>()
    val superTypeEntries = superTypeList?.getChildrenOfType<KtSuperTypeEntry>()?.toMutableList()
    val delegatedSuperTypeEntries =
        superTypeList?.getChildrenOfType<KtDelegatedSuperTypeEntry>()?.toMutableList()
    if (
        superTypeList == null ||
            (superTypeEntries.isNullOrEmpty() && delegatedSuperTypeEntries.isNullOrEmpty())
    ) {
      return emptyList()
    }
    // Skip class body check if KtClassOrObject declaration keyword is interface
    if (isInterface()) {
      return superTypeEntries.orEmpty() + delegatedSuperTypeEntries.orEmpty()
    }
    // Assume first superTypeEntry is class if secondary constructor existed without super
    // type call entry
    if (superTypeCallEntry == null && !superTypeEntries.isNullOrEmpty()) {
      val clazzBody = this.getChildOfType<KtClassBody>()
      val secondaryConstructors = clazzBody?.getSecondaryConstructor()
      val hasDefaultConstructor = secondaryConstructors?.defaultConstructor()?.isNotEmpty() == true
      val hasSuperDelegationCallExpression =
          secondaryConstructors?.superConstructor()?.isNotEmpty() == true
      if (hasDefaultConstructor || hasSuperDelegationCallExpression) {
        superTypeEntries.removeFirstOrNull()
      }
    }
    return superTypeEntries.orEmpty() + delegatedSuperTypeEntries.orEmpty()
  }

  // The entry [getInterfaceTypes] lists only because nothing in the class contradicts it. A
  // class with a primary constructor (written or implicit) must call its superclass, and an enum
  // has none, so their uncalled entries are interfaces.
  private fun KtClassOrObject.ambiguousInterfaceEntry(): KtSuperTypeEntry? {
    if (this !is KtClass || isInterface() || isEnum() || hasExplicitPrimaryConstructor()) {
      return null
    }
    val superTypeList = getChildOfType<KtSuperTypeList>() ?: return null
    if (superTypeList.getChildOfType<KtSuperTypeCallEntry>() != null) return null
    val first = superTypeList.getChildrenOfType<KtSuperTypeEntry>().firstOrNull() ?: return null
    val secondaryConstructors = getChildOfType<KtClassBody>()?.getSecondaryConstructor().orEmpty()
    if (
        secondaryConstructors.isEmpty() ||
            secondaryConstructors.defaultConstructor().isNotEmpty() ||
            secondaryConstructors.superConstructor().isNotEmpty()
    ) {
      return null
    }
    return first
  }

  // A supertype list sees the nested classes of every class containing its own, and those classes'
  // inherited nested classes, which the stubbed supertypes do not reveal.
  private fun KtSuperTypeCallEntry.resolvesInFileScope(): Boolean {
    val owner = getStrictParentOfType<KtClassOrObject>() ?: return false
    return generateSequence(owner.getStrictParentOfType<KtClassOrObject>()) {
          it.getStrictParentOfType<KtClassOrObject>()
        }
        .none { it.superTypeListEntries.isNotEmpty() }
  }

  private fun KtClassOrObject.isInterface(): Boolean =
      getDeclarationKeyword() == KtTokens.INTERFACE_KEYWORD

  private fun KtClassBody.getSecondaryConstructor(): List<KtSecondaryConstructor> =
      getChildrenOfType<KtSecondaryConstructor>().toList()

  private fun List<KtSecondaryConstructor>.defaultConstructor() = filter { secondaryConstructor ->
    val emptyParameterList =
        secondaryConstructor.getChildOfType<KtParameterList>()?.parameters?.isEmpty() == true
    emptyParameterList && secondaryConstructor.hasImplicitDelegationCall()
  }

  private fun List<KtSecondaryConstructor>.superConstructor() = filter {
    !it.hasImplicitDelegationCall() && it.getDelegationCall().calleeExpression?.isThis == false
  }

  /**
   * The [KtUserType] directly under a [KtTypeReference], unwrapping a single nullable wrapper.
   * Mirrors the former UserTypeUtil.getUserType used by collectMultiSegmentQualifiers.
   *
   * When non-nullable: KtTypeReference - KtUserType. When nullable: KtTypeReference -
   * KtNullableType
   * - KtUserType.
   */
  private fun KtTypeReference.userTypeForQualifier(): KtUserType? {
    val nullableTypeWrapper = getChildOfType<KtNullableType>()
    return (nullableTypeWrapper ?: this).getChildOfType<KtUserType>()
  }

  // Grouped rather than flattened: the consumer decides per parameter, from the first bound.
  private fun KtTypeParameterListOwner.multiBoundGroups(): List<List<KtUserType>> {
    val boundsByParameter = mutableMapOf<String, MutableList<KtTypeReference>>()
    typeParameters.forEach { parameter ->
      val name = parameter.name ?: return@forEach
      parameter.extendsBound?.let { boundsByParameter.getOrPut(name) { mutableListOf() }.add(it) }
    }
    typeConstraints.forEach { constraint ->
      val name = constraint.subjectTypeParameterName?.getReferencedName() ?: return@forEach
      constraint.boundTypeReference?.let {
        boundsByParameter.getOrPut(name) { mutableListOf() }.add(it)
      }
    }
    return boundsByParameter.values
        .filter { it.size > 1 }
        .mapNotNull { bounds ->
          val userTypes = bounds.map { it.userTypeForQualifier() }
          // The consumer keys the whole-parameter decision off the source-order first bound.
          // Compacting nulls away would shift a later bound into first position, so a first bound
          // that doesn't resolve to a KtUserType (e.g. a function-type bound) drops the group.
          if (userTypes.first() == null) null else userTypes.filterNotNull()
        }
        .filter { it.size > 1 }
  }

  /**
   * A member-only import (`import a.b.OTA`) whose name is used in type position names a class: an
   * all-caps simple name parses as a static-const member, so the import carries no class name until
   * usage proves it. A member never used as a type stays a declaration (a top-level const or fun).
   */
  fun isProvenClassImport(imp: FullTypeQualifier): Boolean {
    val name =
        imp.member?.takeIf { imp.names.isEmpty() && it.first().isUpperCase() } ?: return false
    return usedUserTypes.any { it.referencedName == name }
  }

  /**
   * A lowercase trailing segment parses as a static-method member, so the import carries no nested
   * name until type-position usage proves it.
   */
  fun isProvenNestedClassImport(imp: FullTypeQualifier): Boolean {
    val member = imp.member ?: return false
    if (imp.names.isEmpty() || !member.first().isLowerCase()) return false
    return usedUserTypes.any { it.referencedName == member }
  }

  /**
   * A companion-path import (`import a.Outer.Companion.X`) colliding with a non-companion import of
   * the same simple name in the same file names the callable: a factory fun and a class share the
   * name in separate namespaces, and the type usages resolve to the other import.
   */
  fun isCompanionCallableImport(imp: FullTypeQualifier): Boolean {
    // The set matches by segments, but the probe always inspected the candidate's own names: a
    // rebuilt qualifier can share an import's segments while parsing its names differently.
    return imp.isThroughCompanion() && imp in companionCallableImports
  }

  private fun promoteProvenClassImports(
      quals: Collection<FullTypeQualifier>,
  ): Set<FullTypeQualifier> =
      quals
          .mapNotNull { imp ->
            imp.member
                ?.takeIf { isProvenClassImport(imp) }
                ?.let { name ->
                  FullTypeQualifier.unsafeBuildQualifier(imp.segments, imp.pkg, listOf(name))
                }
          }
          .toSet()

  /**
   * The candidate an unqualified [simpleName] in [file] refers to.
   *
   * `import a.B as Alias` binds `Alias` and does not bind `B`. An alias resolves to the type it
   * names. Any other name resolves by the last name of an import's FQN, but never through an import
   * that [file] writes only under an alias: the plain `B` then means whatever it would mean without
   * that import, such as a type of the file's own package, which the caller handles as unresolved.
   * Without a [file], only imports that every file renames are passed over.
   *
   * An alias naming a type the caller's own filter excluded stays unresolved: [candidates] is the
   * authority on what this pass may act on. [candidates] must not change after the first call that
   * passes it: its grouping by last name is cached per collection instance.
   */
  fun resolveImportedType(
      candidates: Collection<FullTypeQualifier>,
      simpleName: String,
      file: KtFile? = null,
  ): FullTypeQualifier? {
    val aliases =
        if (file == null) importAliasQualifiers else importAliasQualifiersByFile[file].orEmpty()
    aliases[simpleName]?.let { aliased ->
      candidates
          .find { it.segments == aliased.segments }
          ?.let {
            return it
          }
    }
    val renamed = if (file == null) aliasOnlyImports else aliasOnlyImportsByFile[file].orEmpty()
    val grouped =
        candidatesByLastName.getOrPut(candidates) { candidates.groupBy { it.names.last() } }
    val byLastName = grouped[simpleName].orEmpty().filter { it.segments !in renamed }
    return byLastName.firstOrNull { candidate ->
      aliases.values.none { it.segments == candidate.segments }
    } ?: byLastName.firstOrNull()
  }

  fun packageName(): String? {
    return projectFiles
        .map { it -> it.packageFqName.asString() }
        .filter { it -> it.isNotEmpty() }
        .firstOrNull()
  }

  /**
   * Package segments a same-package stub for a usage in [file] belongs to: the file's own package,
   * falling back to the module's first package for default-package files.
   */
  fun packageSegmentsOf(file: KtFile): List<String> {
    val own = file.packageFqName.asString()
    if (own.isNotEmpty()) return own.split(".")
    return packageName()?.split(".").orEmpty()
  }
}

/** Whether this import binds a name other than its own last name. */
private fun KtImportDirective.renames(): Boolean {
  val alias = aliasName ?: return false
  return alias != importedFqName?.shortName()?.asString()
}

/** The last name of the imported FQN: the name an import without an alias binds. */
private fun KtImportDirective.plainName(): String? = importedFqName?.shortName()?.asString()

/**
 * The type a name written in a type position of this Java source denotes when javac must bind it
 * through a single-type import: exactly one non-static import binds the name's first segment, and
 * no type declared in the source has that name. Any other first segment may denote a type that no
 * source shows: one of the source's own package, one an on-demand or a static import brings in, or
 * a type named like a package, which obscures it (`a.Box` is a nested `Box` when a type `a` is in
 * scope). Such a name stays unresolved.
 */
private fun JavaSourceInfo.qualifierOf(written: List<String>): FullTypeQualifier? {
  val head = written.first()
  if (head in declaredSimpleNames) return null
  val imported =
      imports
          .filter { it !in staticImports && it.segments.last() == head }
          .distinct()
          .singleOrNull() ?: return null
  return FullTypeQualifier(imported.segments + written.drop(1)).withMemberAsNestedName().takeIf {
    it.names.isNotEmpty()
  }
}

private fun FullTypeQualifier.isThroughCompanion(): Boolean {
  for (i in 0 until names.size - 1) {
    if (names[i] == "Companion") return true
  }
  return false
}
