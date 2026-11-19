/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage

/** Information gathered from a device during buck install. */
data class AndroidDeviceInfo(
    val locale: String,
    val abi: String,
    val abiList: Set<String>,
    val buildFingerprint: String,
    val dotsPerInch: String,
    val density: DensityClass,
    val sdk: String,
    val isEmulator: Boolean,
    val transport: String,
) {
  /** The display density category of the device. */
  enum class DensityClass(private val maxDotsPerInch: Int) {
    LDPI(120),
    MDPI(160),
    HDPI(240),
    XHDPI(320),
    XXHDPI(480),
    XXXHDPI(640),
    OTHER_DPI(-1),
    TVDPI(-1);

    companion object {
      @JvmStatic
      fun forPhysicalDensity(dpiString: String): DensityClass {
        try {
          val dpi = dpiString.toInt()
          if (dpi == TVDPI_DPI) {
            return TVDPI
          }
          for (d in entries) {
            if (dpi <= d.maxDotsPerInch) {
              return d
            }
          }
          return OTHER_DPI
        } catch (e: NumberFormatException) {
          return OTHER_DPI
        }
      }
    }
  }

  companion object {
    var TVDPI_DPI: Int = 213

    /**
     * Classifies how adb reaches a device, from its serial. Transfer throughput differs enough
     * between these that install timings are not comparable across them.
     */
    @JvmStatic
    fun transportOf(serialNumber: String): String =
        when {
          // `adb connect` target: a networked device, or one tunnelled to a remote host.
          serialNumber.contains(':') -> "tcp"
          serialNumber.startsWith("emulator-") -> "emulator"
          else -> "usb"
        }
  }
}
