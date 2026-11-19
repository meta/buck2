/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java

import com.facebook.buck.core.filesystems.RelPath
import java.util.Optional

/** Provides access to the various output paths for a java library. */
data class CompilerOutputPaths
@JvmOverloads
constructor(
    val classesDir: RelPath,
    val outputJarDirPath: RelPath,
    val abiJarPath: Optional<RelPath>,
    val annotationPath: RelPath,
    val pathToSourcesList: RelPath,
    val workingDirectory: RelPath,
    val outputJarPath: Optional<RelPath>,
    val compilerClassToSourceMap: RelPath? = null,
) {
  companion object {
    /**
     * Returns a path to a file that contains dependencies used in the compilation to be consumed by
     * buck2
     */
    @JvmStatic
    fun getDepFilePath(outputJarDirPath: RelPath): RelPath {
      return outputJarDirPath.resolveRel("dep-file.txt")
    }

    /** Returns a path to a file that contains the .jars used in the compilation */
    @JvmStatic
    fun getUsedJarsFilePath(outputJarDirPath: RelPath): RelPath {
      return outputJarDirPath.resolveRel("used-jars.json")
    }

    /** Returns a path to a file that contains dependencies used in the compilation */
    @JvmStatic
    fun getJavaDepFilePath(outputJarDirPath: RelPath): RelPath {
      return outputJarDirPath.resolveRel("used-classes.json")
    }

    /** Returns a path to a file that contains all dependencies used in Kotlin compilation */
    @JvmStatic
    fun getKotlinDepFilePath(outputJarDirPath: RelPath): RelPath {
      return outputJarDirPath.resolveRel("kotlin-used-classes.json")
    }

    @JvmStatic
    fun getJvmAbiGenFilePath(outputJarDirPath: RelPath): RelPath {
      return outputJarDirPath.resolveRel("jvm-abi-gen.jar")
    }

    /**
     * Returns a path to the temp file that contains dependencies directly used in Kotlin compiler
     */
    @JvmStatic
    fun getKotlinTempDepFilePath(reportDirPath: RelPath): RelPath {
      return reportDirPath.resolveRel("kotlin-used-classes-tmp.txt")
    }

    /** Returns a path to a file that contains files accessed by KAPT in Kotlin compilation */
    @JvmStatic
    fun getKAPTDepFilePath(reportDirPath: RelPath): RelPath {
      return reportDirPath.resolveRel("kapt-used-classes-tmp.txt")
    }

    /** Returns a path to a file that contains files accessed by KSP in Kotlin compilation */
    @JvmStatic
    fun getKspDepFilePath(reportDirPath: RelPath): RelPath {
      return reportDirPath.resolveRel("ksp-used-classes-tmp.txt")
    }
  }
}
