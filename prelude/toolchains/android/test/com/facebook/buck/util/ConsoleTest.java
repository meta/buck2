/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertSame;

import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import org.junit.Test;

/** Unit tests for {@link Console}. */
public class ConsoleTest {

  @Test
  public void testPrintErrorText_writesMessageToStdErr() {
    ByteArrayOutputStream errBytes = new ByteArrayOutputStream();
    PrintStream stdErr = new PrintStream(errBytes);
    PrintStream stdOut = new PrintStream(new ByteArrayOutputStream());

    Console console = new Console(Verbosity.STANDARD_INFORMATION, stdOut, stdErr);

    console.printErrorText("something went wrong");

    String errOutput = errBytes.toString(StandardCharsets.UTF_8);
    assertEquals("something went wrong" + System.lineSeparator(), errOutput);
  }

  @Test
  public void testPrintErrorText_doesNotWriteToStdOut() {
    ByteArrayOutputStream outBytes = new ByteArrayOutputStream();
    PrintStream stdOut = new PrintStream(outBytes);
    PrintStream stdErr = new PrintStream(new ByteArrayOutputStream());

    Console console = new Console(Verbosity.STANDARD_INFORMATION, stdOut, stdErr);

    console.printErrorText("error message");

    assertEquals(0, outBytes.size());
  }

  @Test
  public void testCreateNullConsole_hasSilentVerbosity() {
    Console nullConsole = Console.createNullConsole();

    assertEquals(Verbosity.SILENT, nullConsole.getVerbosity());
  }

  @Test
  public void testCreateNullConsole_returnsSameInstance() {
    Console first = Console.createNullConsole();
    Console second = Console.createNullConsole();

    assertSame(first, second);
  }
}
