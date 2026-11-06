/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner;

import com.facebook.buck.android.exopackage.AdbUtils;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.infer.annotation.Nullsafe;
import java.util.ArrayList;
import java.util.List;

/** Static methods to collect crash information from a device. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CrashCapturer {
  /**
   * Checks if the given trace string may contain crash trace information.
   *
   * @param trace the trace string to check
   * @return true if the trace string ends with "Check device logcat for details", false otherwise
   */
  public static boolean deviceHasCrashLogs(String trace) {
    return trace.endsWith("Check device logcat for details");
  }

  /**
   * Adds device logcat trace lines to the given trace string.
   *
   * @param androidDevice the AndroidDevice to get logcat from
   * @param adbUtils the AdbUtils instance for executing commands
   * @param trace the trace string to add the logcat trace to
   * @return the trace string with the added logcat trace
   */
  public static String addDeviceLogcatTrace(
      AndroidDevice androidDevice, AdbUtils adbUtils, String trace) {
    try {
      // Wait a short time for debuggerd to (hopefully) write some info out.
      Thread.sleep(1000);

      // Execute logcat command using AdbUtils
      String logcatOutput =
          adbUtils.executeAdbShellCommand(
              "logcat -d", androidDevice.getSerialNumber(), true /* ignoreFailure */);

      // Process the logcat output
      List<String> debugLines = new ArrayList<>();
      boolean inDebuggerdDump = false;

      String[] lines = logcatOutput.split("\n");
      for (String line : lines) {
        if (!line.contains("DEBUG")) {
          continue;
        }
        if (line.contains("*** *** ***")) {
          debugLines.clear();
          inDebuggerdDump = true;
        }
        if (inDebuggerdDump) {
          debugLines.add(line);
        }
      }

      StringBuilder builder = new StringBuilder();
      builder.append(trace);
      if (debugLines.isEmpty()) {
        builder.append("\nSearched logcat, but unable to find a crash.\n");
      } else {
        builder.append("\nFound a crash in logcat:\n\n");
        for (String line : debugLines) {
          builder.append(line).append('\n');
        }
      }
      trace = builder.toString();
    } catch (Exception e) {
      // Is this too verbose?  Should we just ignore failure here?
      // Logging isn't configured, unfortunately.
      System.err.println("Warning: Failed to collect more info from logcat.");
      e.printStackTrace();
    }

    return trace;
  }
}
