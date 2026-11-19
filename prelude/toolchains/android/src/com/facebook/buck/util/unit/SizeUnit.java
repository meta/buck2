/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.unit;

import com.facebook.infer.annotation.Nullsafe;
import java.math.BigDecimal;

@Nullsafe(Nullsafe.Mode.LOCAL)
public enum SizeUnit {
  BYTES(0, "bytes"),
  KILOBYTES(1, "Kbytes"),
  MEGABYTES(2, "Mbytes"),
  GIGABYTES(3, "Gbytes"),
  TERABYTES(4, "Tbytes");

  private final int ordinal;
  private final String abbreviation;

  SizeUnit(int ordinal, String abbreviation) {
    this.ordinal = ordinal;
    this.abbreviation = abbreviation;
  }

  public int getOrdinal() {
    return ordinal;
  }

  public String getAbbreviation() {
    return abbreviation;
  }

  private static long multiplyByByteOrderOfMagnitude(double size, int magnitude) {
    if (magnitude == 0) {
      return (long) size;
    } else if (magnitude > 0) {
      return BigDecimal.valueOf(size).multiply(BigDecimal.valueOf(1024).pow(magnitude)).longValue();
    } else {
      return BigDecimal.valueOf(size)
          .divide(BigDecimal.valueOf(1024).pow(-1 * magnitude))
          .longValue();
    }
  }

  public long toBytes(double size) {
    return multiplyByByteOrderOfMagnitude(size, getOrdinal() - BYTES.getOrdinal());
  }

  public long toKilobytes(double size) {
    return multiplyByByteOrderOfMagnitude(size, getOrdinal() - KILOBYTES.getOrdinal());
  }

  public long toMegabytes(double size) {
    return multiplyByByteOrderOfMagnitude(size, getOrdinal() - MEGABYTES.getOrdinal());
  }

  public long toGigabytes(double size) {
    return multiplyByByteOrderOfMagnitude(size, getOrdinal() - GIGABYTES.getOrdinal());
  }

  public long toTerabytes(double size) {
    return multiplyByByteOrderOfMagnitude(size, getOrdinal() - TERABYTES.getOrdinal());
  }
}
