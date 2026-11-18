/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Calendar;
import java.util.zip.ZipEntry;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipConstants {

  static final long END_MAXLEN = 0xFFFF + ZipEntry.ENDHDR; // header max size ~64k

  static final long ZIP64_ENDSIG = 0x06064b50L;
  static final long ZIP64_LOCSIG = 0x07064b50L;
  static final int ZIP64_LOCOFF = 8; // zip64 end offset
  static final int ZIP64_LOCHDR = 20;
  static final int ZIP64_ENDTOT = 32; // total number of entries
  static final int ZIP64_ENDSIZ = 40; // central directory size in bytes
  static final int ZIP64_ENDOFF = 48; // offset of first CEN header
  static final int ZIP64_ENDHDR = 56;
  static final int ZIP64_EXTID = 0x0001;

  static final int ZIP64_MAGICCOUNT = 0xFFFF;
  static final long ZIP64_MAGICVAL = 0xFFFFFFFFL;

  // The fake time we use: 12:00:00 AM February 1, 1985
  public static final int DOS_FAKE_TIME = 172032000;

  private ZipConstants() {}

  /**
   * {@link java.util.zip.ZipEntry#setTime(long)} is timezone-sensitive. Use this value instead of a
   * hardcoded constant to produce timzeone-agnostic .zip files.
   *
   * @return time in milliseconds that represents a fixed date in the current timezone.
   */
  public static long getFakeTime() {
    // Using any timestamp before 1980 (which is supposedly the earliest time supported by the .zip
    // format) causes files created by JRE 1.7 to be different than JRE 1.8. This is because 1.8
    // actually supports setting timestamps to earlier than 1980, whereas 1.7 rounds up to 1980.
    // We also need to resort to the deprecated date constructor as the ZipEntry uses deprecated
    // Date methods that depend on the current timezone.
    // Finally 1980.01.01 doesn't work across all timezones across Java 1.7 and 1.8. Fun times.
    Calendar c = Calendar.getInstance();
    c.set(1985, Calendar.FEBRUARY, 1, 0, 0, 0);
    return c.getTimeInMillis();
  }
}
