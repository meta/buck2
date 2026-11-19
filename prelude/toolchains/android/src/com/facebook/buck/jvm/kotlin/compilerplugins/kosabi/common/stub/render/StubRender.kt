/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.common.stub.render

import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KArrayType
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KCtorStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KFunStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KPropertyStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub.Type.ANNOTATION
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub.Type.CLASS
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub.Type.INTERFACE
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub.Type.OBJECT
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KType
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KTypeVariableName
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KWildcardType
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.Type
import com.squareup.kotlinpoet.AnnotationSpec
import com.squareup.kotlinpoet.ClassName
import com.squareup.kotlinpoet.CodeBlock
import com.squareup.kotlinpoet.FileSpec
import com.squareup.kotlinpoet.FunSpec
import com.squareup.kotlinpoet.KModifier
import com.squareup.kotlinpoet.ParameterSpec
import com.squareup.kotlinpoet.ParameterizedTypeName.Companion.parameterizedBy
import com.squareup.kotlinpoet.PropertySpec
import com.squareup.kotlinpoet.STAR
import com.squareup.kotlinpoet.TypeName
import com.squareup.kotlinpoet.TypeSpec
import com.squareup.kotlinpoet.TypeVariableName
import com.squareup.kotlinpoet.WildcardTypeName
import com.squareup.kotlinpoet.asClassName
import kotlin.text.StringBuilder

const val KOSABI_GENERATED_STUB_MARKER = "A stub @g" + "enerated by Kosabi"

class RenderedKStub(val stub: KStub, val render: String)

fun KStub.render(): String {
  val className = name
  // Only nested/inner classes should have pkg == null
  val packageName = pkg ?: ""

  val fileSpec =
      FileSpec.builder(packageName, className)
          .apply {
            stubImports
                .filterNot { import -> import.matches("""^(java|org\.jetbrains).*""".toRegex()) }
                .forEach { import -> addImport("", import) }
            if (!treatAsReal) {
              // We want to pretend this stub is a real file.
              addKosabiGeneratedMarker()
            }
            when (type) {
              KStub.Type.TOP_LEVEL_DECLARATION -> renderTopLevelDeclaration(this@render)
              else ->
                  addType(
                      renderType(
                          if (doNotRenderAsKotlinObject) type else returnObjectTypeIfNeeded(),
                      ),
                  )
            }
          }
          .build()

  return StringBuilder().apply { fileSpec.writeTo(this) }.toString()
}

private fun FileSpec.Builder.renderTopLevelDeclaration(stub: KStub) = apply {
  addAnnotation(
      AnnotationSpec.builder(JvmName::class)
          .addMember("%S", stub.name)
          .useSiteTarget(AnnotationSpec.UseSiteTarget.FILE)
          .build(),
  )

  stub.funStubs.map { it.renderFunction(isOpen = false) }.forEach { addFunction(it) }
}

// TODO: We might need to create different names for inner generic type parameters:
//  Now:
//    Outer<T0, T1> { Inner<T0, T1, T2> }
//  We might need:
//    Outer<T0, T1> { Inner<IT0, IT1, IT2> }
private fun KStub.renderType(stubType: KStub.Type): TypeSpec {
  return when (stubType) {
        CLASS ->
            TypeSpec.classBuilder(name).stubDefaultCtor(ctor).also {
              if (abstract) it.addModifiers(KModifier.ABSTRACT)
              else if (!isFinal) it.addModifiers(KModifier.OPEN)
              if (isInternal) it.addModifiers(KModifier.INTERNAL)
            }
        INTERFACE -> TypeSpec.interfaceBuilder(name)
        ANNOTATION -> TypeSpec.annotationBuilder(name).stubAnnotationCtor(ctor)
        OBJECT -> TypeSpec.objectBuilder(name)
        // TODO(sergei): change to render Exception
        else -> throw Error()
      }
      .stubGenericTypes(genericTypes, typeVariableList)
      .apply {
        stubAnnotations.forEach { annotation ->
          addAnnotation(
              AnnotationSpec.builder(ClassName(annotation.pkg, annotation.name))
                  .apply {
                    if (annotation.attributesText.isNotEmpty())
                        addMember(CodeBlock.of(annotation.attributesText))
                  }
                  .build(),
          )
        }
        extends?.let {
          if (stubType == INTERFACE) addSuperinterface(it.asTypeName())
          else superclass(it.asTypeName())
        }
        implements.map { it.asTypeName() }.forEach { addSuperinterface(it) }
        // A static property on a CLASS only becomes a static JVM field from inside the companion;
        // rendered on the class body it is an instance field. Scoped to the opted-in shape so the
        // OBJECT-coercion path below keeps emitting the layout every other stub already relies on.
        val companionProperties =
            if (stubType == CLASS && doNotRenderAsKotlinObject) propertyStubs.filter { it.static }
            else emptyList()
        propertyStubs.filterNot { it in companionProperties }.forEach { addProperty(it.render()) }
        innerStubs
            .map {
              it.renderType(
                  if (it.doNotRenderAsKotlinObject) it.type else it.returnObjectTypeIfNeeded(),
              )
            }
            .forEach { addType(it) }
        val openMembers = !(stubType == CLASS && isFinal)
        val staticFunctions = funStubs.filter { it.static }
        if (
            stubType == CLASS && (staticFunctions.isNotEmpty() || companionProperties.isNotEmpty())
        ) {
          val companionObject = TypeSpec.companionObjectBuilder()
          staticFunctions.map { it.renderFunction() }.forEach { companionObject.addFunction(it) }
          companionProperties.map { it.render() }.forEach { companionObject.addProperty(it) }
          addType(companionObject.build())
          funStubs
              .filterNot { it.static }
              .forEach { addFunction(it.renderFunction(isOpen = openMembers)) }
        } else {
          if (stubType == OBJECT) {
            funStubs.filterNot { it.isConstructor }.map { addFunction(it.renderFunction()) }
          } else {
            funStubs.map { addFunction(it.renderFunction(isOpen = openMembers)) }
          }
        }
      }
      .build()
}

private fun FileSpec.Builder.addKosabiGeneratedMarker(): FileSpec.Builder = apply {
  addFileComment(KOSABI_GENERATED_STUB_MARKER)
}

private fun KFunStub.renderFunction(isOpen: Boolean = true): FunSpec {
  val spec =
      if (isConstructor) {
        FunSpec.constructorBuilder().apply {
          if (private) {
            addModifiers(KModifier.PRIVATE)
          }
          if (constructorSuperDelegate.first) {
            val values =
                constructorSuperDelegate.second.map { nameArg ->
                  val value = nameArg.first
                  when (nameArg.second.pkg + nameArg.second.names) {
                    listOf("kotlin", "String") -> "$value"
                    else -> value
                  }
                }
            callSuperConstructor(*values.toTypedArray())
          }
        }
      } else {
        FunSpec.builder(name).apply {
          if (protected) {
            addModifiers(KModifier.PROTECTED)
          }
          if (isOverride) {
            addModifiers(KModifier.OVERRIDE)
          }
          // Operator function couldn't be open
          if (isOperator) {
            addModifiers(KModifier.OPERATOR)
          } else if (abstract) {
            addModifiers(KModifier.ABSTRACT)
          } else if (isFinal) {
            addModifiers(KModifier.FINAL)
          } else if (isOpen && !isOverride) {
            addModifiers(KModifier.OPEN)
          }

          returns(ret.asTypeName())
          if (ret != KType.UNIT && !abstract) {
            addStatement("return TODO()")
          }

          stubThrows(throws)
          if (static) {
            addAnnotation(JvmStatic::class)
          }
          typeVariableList.forEach { addTypeVariable(it.asTypeVariableName()) }
        }
      }

  // apply the parameter spec
  spec.apply {
    namedArgs.forEach { (name, type) -> addParameter(ParameterSpec(name, type.asTypeName())) }
  }

  annotations.forEach { annotation ->
    spec.addAnnotation(
        AnnotationSpec.builder(ClassName(annotation.pkg, annotation.name))
            .apply {
              if (annotation.attributesText.isNotEmpty())
                  addMember(CodeBlock.of(annotation.attributesText))
            }
            .build(),
    )
  }

  return spec.build()
}

private fun TypeSpec.Builder.stubGenericTypes(
    params: Int,
    typeVariables: List<KTypeVariableName>,
): TypeSpec.Builder = apply {
  if (params <= 0 && typeVariables.isEmpty()) return@apply
  if (typeVariables.isNotEmpty()) {
    typeVariables.forEach { addTypeVariable(it.asTypeVariableName()) }
  } else {
    repeat(params) { addTypeVariable(TypeVariableName("T${it}")) }
  }
}

private fun TypeSpec.Builder.stubDefaultCtor(ctorStub: KCtorStub?): TypeSpec.Builder = apply {
  if (ctorStub == null || ctorStub.namedArgs.isEmpty()) return@apply

  // Expecting ctor(p0: T1, p1: T2, p2: T3)
  val ctor =
      FunSpec.constructorBuilder()
          .apply {
            ctorStub.namedArgs.forEach { (name, type) -> addParameter(name, type.asTypeName()) }
          }
          .build()
  primaryConstructor(ctor)

  ctorStub.namedArgs.forEach { (name, type) ->
    addProperty(
        PropertySpec.builder(name, type.asTypeName()).initializer(name).mutable(true).build(),
    )
  }
}

private fun TypeSpec.Builder.stubAnnotationCtor(ctorStub: KCtorStub?): TypeSpec.Builder = apply {
  if (ctorStub == null || ctorStub.namedArgs.isEmpty()) return@apply

  val ctor =
      FunSpec.constructorBuilder()
          .apply {
            ctorStub.namedArgs.forEach { (name, type) -> addParameter(name, type.asTypeName()) }
          }
          .build()
  primaryConstructor(ctor)

  ctorStub.namedArgs.forEach { (name, type) ->
    addProperty(PropertySpec.builder(name, type.asTypeName()).initializer(name).build())
  }
}

private fun FunSpec.Builder.stubThrows(e: List<KType>): FunSpec.Builder = apply {
  if (e.isEmpty()) return@apply

  val throwsAnno =
      AnnotationSpec.builder(Throws::class)
          .apply { e.map { it.asTypeName() }.forEach { addMember("$it::class") } }
          .build()

  addAnnotation(throwsAnno)
}

private fun KTypeVariableName.asTypeVariableName(): TypeVariableName =
    TypeVariableName(name, *bounds.map { it.asTypeName() }.toTypedArray())

// If the class contains all static functions (filter out private and constructor) or all inner
// class is Object type, we will render as object class instead.
// This is special handler for JavaKStub, as the companion object is not expected in Dex level.
private fun KStub.returnObjectTypeIfNeeded(): KStub.Type {
  val allPropertiesAreStatic = propertyStubs.isNotEmpty() && propertyStubs.all { it.static }
  val innerFuns = funStubs.filterNot { it.private || it.isConstructor }
  val allFunsAreStatic = innerFuns.isNotEmpty() && innerFuns.all { it.static }
  val allInnerClassesAreObject =
      innerStubs.isNotEmpty() && innerStubs.all { it.returnObjectTypeIfNeeded() == OBJECT }
  return if (
      type == CLASS &&
          ctor == null &&
          (allFunsAreStatic || allInnerClassesAreObject || allPropertiesAreStatic)
  ) {
    OBJECT
  } else {
    type
  }
}

fun Type.asTypeName(): TypeName {
  if (this is KTypeVariableName) {
    return asTypeVariableName()
  }
  val type = ClassName(pkg.joinToString(separator = "."), names)

  val typeName =
      if (generics.isNotEmpty()) {
            type.parameterizedBy(generics.map { it.asTypeName() })
          } else {
            type
          }
          .copy(nullable)

  return when (this) {
    is KArrayType -> {
      Array::class.asClassName().parameterizedBy(typeName).copy(nullable)
    }

    is KWildcardType -> {
      when (bound) {
        KWildcardType.BoundType.SUPER -> WildcardTypeName.consumerOf(typeName)
        KWildcardType.BoundType.EXTENDS -> WildcardTypeName.producerOf(typeName)
        KWildcardType.BoundType.UNBOUNDED -> STAR
      }
    }

    else -> {
      typeName
    }
  }
}

private fun KPropertyStub.render(): PropertySpec {
  val spec =
      PropertySpec.builder(name, ret.asTypeName()).apply {
        if (static) addAnnotation(JvmField::class)
        if (value != null) {
          initializer(value, *args.map { it.asTypeName() }.toTypedArray())
        }
      }
  return spec.build()
}
