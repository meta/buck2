/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.core

import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.jvm.cd.serialization.RelPathSerializer
import com.facebook.buck.util.environment.Platform
import com.google.common.base.Strings
import java.util.Optional
import java.util.regex.Pattern

/** Extra params from [BuildTargetValue] needed for Kotlin. */
data class BuildTargetValueExtraParams(
    val cellRelativeBasePath: RelPath,
    val flavored: Boolean,
    val basePathForBaseName: RelPath,
    val shortNameAndFlavorPostfix: String,
    val shortName: String,
    val scratchDir: RelPath,
) {
  val moduleName: String
    get() {
      val pathSeparator = cellRelativeBasePath.fileSystem.separator
      return (cellRelativeBasePath.toString().replace(pathSeparator, ".") + "." + shortName)
    }

  val kaptAnnotationGenPath: RelPath
    get() = genPath.resolveRel("__kapt_generated__")

  val kspAnnotationGenPath: RelPath
    get() = genPath.resolveRel("__ksp_generated__")

  val annotationOutputBasePath: RelPath =
      scratchDir.resolveRel("annotation").resolve(basePathForBaseName)

  /** Returns annotation path for the given `target` and `format` */
  fun getAnnotationOutputPath(format: String): RelPath {
    require(!format.startsWith("/")) { "format string should not start with a slash" }

    return annotationOutputBasePath.resolveRel(formatLastSegment(format, shortNameAndFlavorPostfix))
  }

  val genPath: RelPath
    /** Returns `gen` directory path for the given `target` */
    get() {
      val format = if (flavored) "%s" else "%s__"
      return getGenPath(format)
    }

  /** Returns `gen` directory path for the given `target` and `format` */
  fun getGenPath(format: String): RelPath {
    require(!format.startsWith("/")) { "format string should not start with a slash" }
    return getRelativePath(format, scratchDir.resolveRel("gen"))
  }

  /** Returns `gen` directory path for the given `target` and `format` */
  fun getScratchPath(format: String): RelPath {
    require(!format.startsWith("/")) { "format string should not start with a slash" }
    return getRelativePath(format, scratchDir.resolveRel("bin"))
  }

  fun getRelativePath(format: String, directory: RelPath): RelPath {
    return directory.resolve(getBasePath(format))
  }

  fun getBasePath(format: String): RelPath {
    require(!format.startsWith("/")) { "format string should not start with a slash" }
    return basePathForBaseName.resolveRel(formatLastSegment(format, shortNameAndFlavorPostfix))
  }

  companion object {
    /** Creates [BuildTargetValueExtraParams] */
    fun of(
        cellRelativeBasePath: RelPath,
        flavored: Boolean,
        basePathForBaseName: RelPath,
        shortNameAndFlavorPostfix: String,
        shortName: String,
        scratchDir: RelPath,
    ): BuildTargetValueExtraParams {
      require(shortNameAndFlavorPostfix.startsWith(shortName)) {
        String.format(
            "shortNameAndFlavorPostfix:%s should start with shortName:%s",
            shortNameAndFlavorPostfix,
            shortName,
        )
      }
      return BuildTargetValueExtraParams(
          cellRelativeBasePath,
          flavored,
          basePathForBaseName,
          shortNameAndFlavorPostfix,
          shortName,
          scratchDir,
      )
    }

    @JvmStatic
    fun of(buildTargetValue: BuildTargetValue, buckOut: RelPath): BuildTargetValueExtraParams {
      // in buck1: cell//base_path:target#flavor vs. in buck2: cell//base_path:target[flavor]
      val buck1Style = "(#(.*))"
      val buck2Style = "(\\[([^\\]]*)\\])"
      val pattern =
          Pattern.compile(String.format("(.*)//([^:]*):([^#\\[]*)(%s|%s)?", buck1Style, buck2Style))
      val matcher = pattern.matcher(buildTargetValue.fullyQualifiedName)
      if (!matcher.matches()) {
        throw RuntimeException(
            "Can't reconstruct extra params from fullyQualifiedName: " +
                buildTargetValue.fullyQualifiedName,
        )
      }
      val basePath = matcher.group(2)
      val targetName = matcher.group(3)
      var flavor = Optional.empty<String>()
      if (!Strings.isNullOrEmpty(matcher.group(4))) {
        flavor =
            if (!Strings.isNullOrEmpty(matcher.group(6))) Optional.of(matcher.group(6))
            else Optional.of(matcher.group(8))
      }
      val shortNameAndFlavorPostfix = targetName + flavor.map { str: String -> "#$str" }.orElse("")
      return of(
          RelPathSerializer.deserialize(basePath),
          flavor.isPresent,
          RelPathSerializer.deserialize(basePath),
          shortNameAndFlavorPostfix,
          targetName,
          buckOut,
      )
    }

    /**
     * Returns a formatted string using the specified format string and a given argument. In case of
     * windows platform back slashes would be replaced with forward slashes before applying string
     * formatting.
     *
     * @param format [String.format] string for the path name. It should contain one "%s", which
     *   will be filled in with the rule's short name. It should not start with a slash.
     * @param arg an argument referenced by the format specifier in the format string.
     * @return A formatted string
     */
    private fun formatLastSegment(format: String, arg: String): String {
      var format = format
      require(!format.startsWith("/")) { "format string should not start with a slash" }

      if (Platform.detect() == Platform.WINDOWS) {
        // TODO(nga): prohibit backslashes in format
        format = format.replace('\\', '/')
      }

      return String.format(format, arg)
    }
  }
}
