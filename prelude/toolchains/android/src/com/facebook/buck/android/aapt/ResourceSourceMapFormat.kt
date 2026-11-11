/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.aapt

import java.nio.charset.StandardCharsets
import java.util.Base64

/** Serialization contract for Robolectric resource source-map rows. */
object ResourceSourceMapFormat {
  private const val DELIMITER = '\t'
  private val fieldEncoder = Base64.getUrlEncoder().withoutPadding()
  private val fieldDecoder = Base64.getUrlDecoder()

  interface Row {
    fun serialize(): String
  }

  /** Format: `R\tpriority\ttype\tname\tqualifiers\tsource\townerBuildFile`. */
  data class ResourceRow(
      val priority: Int,
      val type: String,
      val name: String,
      val qualifiers: String,
      val source: String,
      val ownerBuildFile: String,
  ) : Row {
    override fun serialize(): String =
        listOf(TAG, priority.toString(), type, name, qualifiers, source, ownerBuildFile)
            .joinToString(DELIMITER.toString())

    companion object {
      private const val TAG = "R"

      private enum class Attribute {
        TAG,
        PRIORITY,
        TYPE,
        NAME,
        QUALIFIERS,
        SOURCE,
        OWNER_BUILD_FILE,
      }

      internal fun parse(fields: List<String>): ResourceRow? {
        if (fields.size != Attribute.entries.size || fields[Attribute.TAG.ordinal] != TAG)
            return null
        return ResourceRow(
            fields[Attribute.PRIORITY.ordinal].toIntOrNull() ?: return null,
            fields[Attribute.TYPE.ordinal],
            fields[Attribute.NAME.ordinal],
            fields[Attribute.QUALIFIERS.ordinal],
            fields[Attribute.SOURCE.ordinal],
            fields[Attribute.OWNER_BUILD_FILE.ordinal],
        )
      }
    }
  }

  /** Format: `A\tpriority\tencodedPath\tencodedSource\tencodedOwnerBuildFile`. */
  data class AssetRow(
      val priority: Int,
      val path: String,
      val source: String,
      val ownerBuildFile: String,
  ) : Row {
    override fun serialize(): String = listOf(
        TAG,
        priority.toString(),
        encodeField(path),
        encodeField(source),
        encodeField(ownerBuildFile),
    )
        .joinToString(DELIMITER.toString())

    companion object {
      private const val TAG = "A"

      private enum class Attribute {
        TAG,
        PRIORITY,
        PATH,
        SOURCE,
        OWNER_BUILD_FILE,
      }

      internal fun parse(fields: List<String>): AssetRow? {
        if (fields.size != Attribute.entries.size || fields[Attribute.TAG.ordinal] != TAG)
            return null
        return try {
          AssetRow(
              fields[Attribute.PRIORITY.ordinal].toIntOrNull() ?: return null,
              decodeField(fields[Attribute.PATH.ordinal]),
              decodeField(fields[Attribute.SOURCE.ordinal]),
              decodeField(fields[Attribute.OWNER_BUILD_FILE.ordinal]),
          )
        } catch (_: IllegalArgumentException) {
          null
        }
      }
    }
  }

  /** Format: `F\tpriority\tencodedPath\tencodedSource\tencodedOwnerBuildFile`. */
  data class FileRow(
      val priority: Int,
      val path: String,
      val source: String,
      val ownerBuildFile: String,
  ) : Row {
    override fun serialize(): String = listOf(
        TAG,
        priority.toString(),
        encodeField(path),
        encodeField(source),
        encodeField(ownerBuildFile),
    )
        .joinToString(DELIMITER.toString())

    companion object {
      private const val TAG = "F"

      private enum class Attribute {
        TAG,
        PRIORITY,
        PATH,
        SOURCE,
        OWNER_BUILD_FILE,
      }

      internal fun parse(fields: List<String>): FileRow? {
        if (fields.size != Attribute.entries.size || fields[Attribute.TAG.ordinal] != TAG)
            return null
        return try {
          FileRow(
              fields[Attribute.PRIORITY.ordinal].toIntOrNull() ?: return null,
              decodeField(fields[Attribute.PATH.ordinal]),
              decodeField(fields[Attribute.SOURCE.ordinal]),
              decodeField(fields[Attribute.OWNER_BUILD_FILE.ordinal]),
          )
        } catch (_: IllegalArgumentException) {
          null
        }
      }
    }
  }

  fun parse(line: String): Row? {
    val fields = line.split(DELIMITER)
    return ResourceRow.parse(fields) ?: AssetRow.parse(fields) ?: FileRow.parse(fields)
  }

  fun encodeField(value: String): String =
      fieldEncoder.encodeToString(value.toByteArray(StandardCharsets.UTF_8))

  fun decodeField(value: String): String =
      String(fieldDecoder.decode(value), StandardCharsets.UTF_8)
}
