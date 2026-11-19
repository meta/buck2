/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.common.stub.model

import com.facebook.kotlin.compilerplugins.kosabi.common.FullTypeQualifier
import kotlin.reflect.KClass

class KType
internal constructor(
    private val ftq: FullTypeQualifier,
    override val nullable: Boolean = false,
    override val generics: List<Type> = emptyList(),
) : Type {
  /** @param qualifierName is a fullTypeQualifier string representation, example: com.foo.A.B.C */
  constructor(
      qualifierName: String,
      nullable: Boolean = false,
  ) : this(FullTypeQualifier(qualifierName.split(".")), nullable)

  override val pkg: List<String>
    get() = ftq.pkg

  override val names: List<String>
    get() = ftq.names

  fun parametriseWith(generics: List<Type>): KType = KType(ftq, nullable, generics)

  companion object {
    /**
     * A class by its package and its class names, outermost first. Case does not decide where the
     * package ends: the caller knows the split from the declaration or the class file.
     */
    fun ofClass(pkg: List<String>, names: List<String>, nullable: Boolean = false): KType =
        KType(FullTypeQualifier.unsafeBuildQualifier(pkg + names, pkg, names), nullable)

    val ANY_NULLABLE: KType = Any::class.asNullableKType()
    val UNIT: KType = Unit::class.asKType()
    val BOOLEAN: KType = Boolean::class.asKType()
    val BOOLEAN_NULLABLE: KType = Boolean::class.asNullableKType()
    val BYTE: KType = Byte::class.asKType()
    val BYTE_NULLABLE: KType = Byte::class.asNullableKType()
    val CHAR: KType = Char::class.asKType()
    val CHAR_NULLABLE: KType = Char::class.asNullableKType()
    val INT: KType = Int::class.asKType()
    val INT_NULLABLE: KType = Int::class.asNullableKType()
    val DOUBLE: KType = Double::class.asKType()
    val DOUBLE_NULLABLE: KType = Double::class.asNullableKType()
    val FLOAT: KType = Float::class.asKType()
    val FLOAT_NULLABLE: KType = Float::class.asNullableKType()
    val LONG: KType = Long::class.asKType()
    val LONG_NULLABLE: KType = Long::class.asNullableKType()
    val SHORT: KType = Short::class.asKType()
    val SHORT_NULLABLE: KType = Short::class.asNullableKType()
  }
}

fun KClass<out Any>.asKType(): KType = KType(this.qualifiedName!!)

fun KClass<out Any>.asNullableKType(): KType = KType(this.qualifiedName!!, nullable = true)
