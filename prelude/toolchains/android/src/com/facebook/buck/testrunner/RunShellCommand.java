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

import com.facebook.infer.annotation.Nullsafe;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;

/** class for running shell commands through bash. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class RunShellCommand {

  /** runs command and args through bash. */
  public static String run(String commandAndArgs) {
    try {
      System.out.println("RunShellCommand. Command:  >" + commandAndArgs + "<");
      Process process =
          new ProcessBuilder(new String[] {"bash", "-c", commandAndArgs})
              .redirectErrorStream(true)
              .start();

      String out = inputStreamToString(process.getInputStream());
      int exitValue = process.waitFor();
      if (!commandAndArgs.contains("logcat")) {
        System.out.println("RunShellCommand. Output: >" + out + "<");
      }
      if (exitValue != 0) {
        throw new RuntimeException(
            "command failed! exitValue: " + exitValue + ", for command >" + commandAndArgs + "<");
      }
      return out;
    } catch (IOException e) {
      throw new RuntimeException(e);
    } catch (InterruptedException e) {
      throw new RuntimeException(e);
    }
  }

  public static String run(String command, String args) {
    return run(command + " " + args);
  }

  private static String inputStreamToString(InputStream inputStream) throws IOException {
    StringBuilder stringBuilder = new StringBuilder();
    BufferedReader bufferedReader = new BufferedReader(new InputStreamReader(inputStream));
    String line;
    while ((line = bufferedReader.readLine()) != null) {
      stringBuilder.append(line).append("\n");
    }
    bufferedReader.close();
    return stringBuilder.toString();
  }
}
