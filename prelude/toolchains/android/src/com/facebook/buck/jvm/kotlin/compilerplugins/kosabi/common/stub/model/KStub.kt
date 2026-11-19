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

import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.Type as IType

class KStub(val pkg: String?, val name: String) {
  constructor(name: String) : this(null, name)

  var type: Type = Type.CLASS
    set(value: Type) {
      Logger.log(
          """
      |  [Update stub]
      |    - name: $pkg:$name
      |    - type: ${field.name} -> ${value.name}
    """
              .trimMargin(),
      )
      field = value
    }

  var genericTypes: Int = 0
    set(value: Int) {
      if (field != value) {
        Logger.log(
            """
      |  [Update stub]
      |    - name: $pkg:$name
      |    - type: ${type.name}
      |    - genericTypes: $field -> $value
    """
                .trimMargin(),
        )
      }
      field = value
    }

  var ctor: KCtorStub? = null
    set(value: KCtorStub?) {
      Logger.log(
          """
        |  [Update stub]
        |    - name: $pkg:$name
        |    - type: ${type.name}
        |    - ctorParams: $field -> $value
      """,
      )
      field = value
    }

  // Add extends declaration here when we will need it
  var extends: IType? = null
  var implements: List<IType> = emptyList()
  var typeVariableList: List<KTypeVariableName> = emptyList()
  var stubAnnotations: List<KAnnotation> = emptyList()
  var stubImports: List<String> = emptyList()

  var abstract: Boolean = false

  // A final CLASS renders without `open` on the class and on its non-override members.
  var isFinal: Boolean = false

  // An internal CLASS renders with the `internal` modifier.
  var isInternal: Boolean = false
  var doNotRenderAsKotlinObject: Boolean = false

  // TODO: We might want to optimise [innerStubs] for find operation
  val innerStubs: MutableSet<KStub> = mutableSetOf()
  var funStubs: MutableList<KFunStub> = mutableListOf()

  val propertyStubs: MutableList<KPropertyStub> = mutableListOf()

  // In some cases we want to treat [KStub] not as a stub, but as something that
  // came from a compilation context.
  // Faking Annotation Processors shapes is one example.
  var treatAsReal: Boolean = false

  override fun equals(other: Any?): Boolean {
    if (this === other) {
      return true
    }
    if (javaClass != other?.javaClass) {
      return false
    }

    other as KStub

    if (pkg != other.pkg) {
      return false
    }
    if (name != other.name) {
      return false
    }

    return true
  }

  override fun hashCode(): Int {
    var result = pkg?.hashCode() ?: 0
    result = 31 * result + name.hashCode()
    return result
  }

  enum class Type {
    ANNOTATION,
    CLASS,
    INTERFACE,
    OBJECT,
    TOP_LEVEL_DECLARATION,
  }
}
