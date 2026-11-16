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

import org.jetbrains.kotlin.fir.declarations.FirClass
import org.jetbrains.kotlin.fir.declarations.utils.classId
import org.jetbrains.kotlin.fir.java.FirJavaFacade
import org.jetbrains.kotlin.fir.java.deserialization.JvmClassFileBasedSymbolProvider
import org.jetbrains.kotlin.fir.moduleData
import org.jetbrains.kotlin.fir.resolve.providers.impl.FirCachingCompositeSymbolProvider
import org.jetbrains.kotlin.fir.resolve.providers.symbolProvider
import org.jetbrains.kotlin.load.java.structure.impl.VirtualFileBoundJavaClass

// KT-60555 workaround: library sessions never register the session-level javaSymbolProvider.
// 2.3 removed FirJavaAwareSymbolProvider and privatized the facade, so reflection is the only
// path. See:
// https://youtrack.jetbrains.com/issue/KT-60555/K2.-FirJavaClass-source-field-is-null
fun FirClass.toBinaryJavaClassCompat(): VirtualFileBoundJavaClass {
  val symbolProvider = moduleData.session.symbolProvider
  val composite =
      symbolProvider as? FirCachingCompositeSymbolProvider
          ?: error(
              "KT-60555 workaround needs FirCachingCompositeSymbolProvider, got ${symbolProvider::class}",
          )
  val service =
      composite.providers.firstOrNull { it is JvmClassFileBasedSymbolProvider }
          ?: error(
              "KT-60555 workaround needs JvmClassFileBasedSymbolProvider, found none among ${composite.providers.size} providers",
          )
  val facade =
      try {
        JvmClassFileBasedSymbolProvider::class
            .java
            .getDeclaredField("javaFacade")
            .apply { isAccessible = true }
            .get(service) as FirJavaFacade
      } catch (e: NoSuchFieldException) {
        error(
            "KT-60555 workaround broken by compiler change: no javaFacade field on JvmClassFileBasedSymbolProvider",
        )
      } catch (e: Exception) {
        error(
            "KT-60555 workaround broken by compiler/JDK change: cannot read javaFacade (${e::class.simpleName}: ${e.message})",
        )
      }
  val binaryClass =
      facade.findClass(classId)
          ?: error("KT-60555 workaround: FirJavaFacade cannot resolve $classId")
  if (binaryClass !is VirtualFileBoundJavaClass) {
    error("Unsupported kind of a JavaClass: ${binaryClass::class}")
  }
  return binaryClass
}
