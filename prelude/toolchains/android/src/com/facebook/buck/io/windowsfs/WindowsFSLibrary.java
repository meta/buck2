/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.io.windowsfs;

import com.facebook.infer.annotation.Nullsafe;
import com.sun.jna.Library;
import com.sun.jna.Native;
import com.sun.jna.win32.W32APIOptions;

/** Utility class to bridge native windows FS calls to Java using JNA. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface WindowsFSLibrary extends Library {
  WindowsFSLibrary INSTANCE =
      Native.loadLibrary("kernel32", WindowsFSLibrary.class, W32APIOptions.UNICODE_OPTIONS);

  int SYMBOLIC_LINK_FLAG_DIRECTORY = 1;
  int SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE = 2;

  // Native Windows error codes.
  int INVALID_PARAMETER_ERROR = 87;
  int ERROR_ALREADY_EXISTS = 183;
  int ERROR_PRIVILEGE_NOT_HELD = 1314;

  /*
   * The CreateSymbolicLinkW API on Windows returns
   * a BOOLEAN, which is really a byte in Java (8 bit numeric value).
   * At the JVM level, boolean, byte, short, char, and int are all treated as ints, and boolean is
   * indeed represented as 0 and 1. The API is successful if the return value is not 0.
   * So, returning 0xD0 should be a success, but the 0th bit is not set and Java will interpret this
   * as boolean false. This is fixed by making the function imported from native as a byte return
   * and compering the return value explicitly to 0.
   *
   * @param lpSymlinkFileName The new symlink name.
   * @param lpTargetFileName The existing file to link into.
   * @param dwFlags Whether it is a directiory.
   * @return Anh 8 bit integer which is interpreted as success if not 0, and failure as 0.
   */
  byte CreateSymbolicLinkW(String lpSymlinkFileName, String lpTargetFileName, int dwFlags);

  int GetLastError();
}
