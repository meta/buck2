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
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Analyzes crash information from logcat output to detect specific crash types and provide enhanced
 * crash reporting.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CrashAnalyzer {

  /** Defines a crash type with its detection pattern and message. */
  private static class CrashType {
    final Pattern pattern;
    final String detectionMessage;
    final boolean isSignal;

    CrashType(Pattern pattern, String detectionMessage, boolean isSignal) {
      this.pattern = pattern;
      this.detectionMessage = detectionMessage;
      this.isSignal = isSignal;
    }
  }

  private static final CrashType[] CRASH_TYPES = {
    new CrashType(
        Pattern.compile("java\\.lang\\.OutOfMemoryError:.*"),
        "Out of memory error detected",
        false),
    new CrashType(
        Pattern.compile("java\\.lang\\.StackOverflowError"),
        "Stack overflow error detected",
        false),
    new CrashType(
        Pattern.compile("java\\.lang\\.NullPointerException"),
        "Null pointer exception detected",
        false),
    new CrashType(
        Pattern.compile(
            "java\\.lang\\.IndexOutOfBoundsException|java\\.lang\\.ArrayIndexOutOfBoundsException"),
        "Index out of bounds exception detected",
        false),
    new CrashType(
        Pattern.compile("java\\.lang\\.IllegalStateException"),
        "Illegal state exception detected",
        false),
    new CrashType(
        // Match a libc/debuggerd "signal 6 (SIGABRT)" line or debuggerd's "Abort message:". A bare
        // "Abort" token matched any line containing the word, e.g. WhatsApp's
        // "SafeModuleLoader ... Loading module: abort_hook" logged at every app start, which
        // produced a false SIGABRT verdict on every run of those apps.
        Pattern.compile("(?i)\\bsignal 6\\b|SIGABRT|\\bAbort message\\b"),
        "SIGABRT signal detected",
        true),
    new CrashType(
        // Match a real native-crash line (libc "Fatal signal 11 (SIGSEGV)" / debuggerd
        // "signal 11 (SIGSEGV)") or a "Segmentation fault" report. The bare "SIGSEGV" token was
        // intentionally removed: it matches any log line that merely mentions the word, e.g. the
        // benign MessageExecutionMonitor warning "...risk of ART nterp stack-walk SIGSEGV..."
        // (added
        // by D109574690, fires ~hundreds of times per run), which produced false "native crash"
        // verdicts. Real SIGSEGV crashes always carry "signal 11", so coverage is unaffected.
        Pattern.compile("(?i)Fatal signal 11|signal 11|segmentation fault"),
        "SIGSEGV signal detected (native crash)",
        true),
    new CrashType(
        Pattern.compile("(?i)Fatal signal 4|signal 4|SIGILL|illegal instruction"),
        "SIGILL signal detected (illegal instruction)",
        true),
    new CrashType(
        Pattern.compile("(?i)Fatal signal 8|signal 8|SIGFPE|floating point exception"),
        "SIGFPE signal detected (floating point exception)",
        true),
    new CrashType(
        Pattern.compile("(?i)Fatal signal 7|signal 7|SIGBUS|bus error"),
        "SIGBUS signal detected (bus error)",
        true),
    new CrashType(
        Pattern.compile("java\\.lang\\.ClassNotFoundException"),
        "Class not found exception detected",
        false),
  };

  /**
   * Analyzes logcat output for crash information and forwards it to stdout/stderr so the enhanced
   * crash detection system can detect specific crash types.
   *
   * @param logcatOutput The raw logcat output to analyze
   */
  public void analyzeCrashInformation(String logcatOutput) {
    if (logcatOutput == null || logcatOutput.trim().isEmpty()) {
      return;
    }

    System.err.println("=== AIT CRASH ANALYSIS ===");
    boolean crashDetected = false;

    for (CrashType crashType : CRASH_TYPES) {
      Matcher matcher = crashType.pattern.matcher(logcatOutput);
      boolean matched;
      if (crashType.isSignal) {
        matched = matcher.find();
      } else {
        // A Java-exception token only indicates a crash when it is surfaced by the crash
        // reporter, not when some component merely logs a caught exception (see
        // isCrashContext). Scan every occurrence so a genuine crash later in the log is still
        // found even when benign matches precede it.
        matched = false;
        while (matcher.find()) {
          if (isCrashContext(logcatOutput, matcher.start())) {
            matched = true;
            break;
          }
        }
      }
      if (matched) {
        crashDetected = true;
        System.err.println("AIT_CRASH_DETECTION: " + crashType.detectionMessage);

        if (crashType.isSignal) {
          extractSignalStackTrace(logcatOutput, matcher);
        } else {
          extractExceptionStackTrace(logcatOutput, matcher);
        }
      }
    }

    if (!crashDetected) {
      System.err.println("No crashes detected");
    }
    System.err.println("=== END AIT CRASH ANALYSIS ===");
  }

  /**
   * Decides whether a matched Java-exception token represents an actual crash rather than a
   * caught-and-logged exception. An uncaught exception that crashes the process is reported by the
   * Android runtime's uncaught-exception handler (the "AndroidRuntime" tag / "FATAL EXCEPTION"
   * header); a failure to load a test class is reported by the instrumentation "TestRunner". A
   * token logged under any other tag is a component logging a caught exception, e.g. the benign
   * lines that previously produced false "crash detected" verdicts on every run:
   *
   * <pre>
   *   E QPLProvider: java.lang.IllegalStateException: No QPL instance provided
   *   D CoverageInit: java.lang.ClassNotFoundException: com.facebook.coverage.e2e.CoverageSetup
   * </pre>
   *
   * The check is on the matched line itself, which works for both brief ("E/AndroidRuntime(pid):")
   * and threadtime ("... E AndroidRuntime:") logcat formats since the exception token shares the
   * line with its tag.
   *
   * @param logcatOutput The raw logcat output
   * @param matchStart Offset of the exception token match within {@code logcatOutput}
   * @return true if the match is on a crash-reporter / test-runner line
   */
  private static boolean isCrashContext(String logcatOutput, int matchStart) {
    int lineStart = logcatOutput.lastIndexOf('\n', matchStart) + 1;
    int lineEnd = logcatOutput.indexOf('\n', matchStart);
    if (lineEnd == -1) {
      lineEnd = logcatOutput.length();
    }
    String line = logcatOutput.substring(lineStart, lineEnd);
    return line.contains("AndroidRuntime")
        || line.contains("FATAL EXCEPTION")
        || line.contains("TestRunner");
  }

  /**
   * Extracts Java exception details and stack trace location.
   *
   * @param logcatOutput The raw logcat output
   * @param exceptionMatcher The matcher that found the Java exception
   */
  private void extractExceptionStackTrace(String logcatOutput, Matcher exceptionMatcher) {
    // Extract the Java exception line
    int matchStart = exceptionMatcher.start();
    int lineStart = logcatOutput.lastIndexOf('\n', matchStart) + 1;
    int lineEnd = logcatOutput.indexOf('\n', matchStart);
    if (lineEnd == -1) {
      lineEnd = logcatOutput.length();
    }

    String exceptionLine = logcatOutput.substring(lineStart, lineEnd).trim();
    if (!exceptionLine.isEmpty()) {
      System.err.println("Exception: " + exceptionLine);
    }

    // Look for the "at" stack trace line immediately following the exception
    int nextLineStart = lineEnd + 1;
    if (nextLineStart < logcatOutput.length()) {
      int nextLineEnd = logcatOutput.indexOf('\n', nextLineStart);
      if (nextLineEnd == -1) {
        nextLineEnd = logcatOutput.length();
      }

      String nextLine = logcatOutput.substring(nextLineStart, nextLineEnd).trim();
      // Check if the next line contains stack trace information starting with "at"
      if (nextLine.contains("\tat ") || nextLine.trim().startsWith("at ")) {
        System.err.println("Stack trace: " + nextLine.trim());
      }
    }
  }

  /**
   * Extracts signal crash details and backtrace location.
   *
   * @param logcatOutput The raw logcat output
   * @param signalMatcher The matcher that found the signal
   */
  private void extractSignalStackTrace(String logcatOutput, Matcher signalMatcher) {
    // Extract the signal line
    int matchStart = signalMatcher.start();
    int lineStart = logcatOutput.lastIndexOf('\n', matchStart) + 1;
    int lineEnd = logcatOutput.indexOf('\n', matchStart);
    if (lineEnd == -1) {
      lineEnd = logcatOutput.length();
    }

    String signalLine = logcatOutput.substring(lineStart, lineEnd).trim();
    if (!signalLine.isEmpty()) {
      System.err.println("Signal: " + signalLine);

      // Determine which signal type this is
      String signalType = determineSignalType(signalLine);

      // Extract signal code information as potential root cause indicator
      String signalCode = extractSignalCode(signalLine, signalType);
      if (!signalCode.isEmpty()) {
        System.err.println("ROOT CAUSE: " + signalCode);
      }
    }

    // Look for additional signal information (like backtrace) in following lines
    int nextLineStart = lineEnd + 1;
    for (int i = 0; i < 5 && nextLineStart < logcatOutput.length(); i++) {
      int nextLineEnd = logcatOutput.indexOf('\n', nextLineStart);
      if (nextLineEnd == -1) {
        nextLineEnd = logcatOutput.length();
      }

      String nextLine = logcatOutput.substring(nextLineStart, nextLineEnd).trim();
      // Look for backtrace information or native crash details
      if (nextLine.contains("backtrace:") || nextLine.contains("pc ") || nextLine.contains("#")) {
        System.err.println("Backtrace: " + nextLine.trim());
      }
      nextLineStart = nextLineEnd + 1;
    }
  }

  /**
   * Determines the signal type from a signal line.
   *
   * @param signalLine The signal line from logcat
   * @return The signal type (e.g., "SIGABRT", "SIGSEGV", "SIGILL", "SIGFPE", "SIGBUS"), or
   *     "UNKNOWN" if not determinable
   */
  private String determineSignalType(String signalLine) {
    String upperLine = signalLine.toUpperCase();

    // Check for SIGABRT (signal 6)
    if (upperLine.contains("SIGABRT")
        || upperLine.matches(".*SIGNAL\\s+6\\b.*")
        || upperLine.contains("ABORT")) {
      return "SIGABRT";
    }

    // Check for SIGSEGV first (signal 11)
    if (upperLine.contains("SIGSEGV")
        || upperLine.matches(".*SIGNAL\\s+11\\b.*")
        || upperLine.contains("SEGMENTATION")) {
      return "SIGSEGV";
    }

    // Check for SIGILL (signal 4)
    if (upperLine.contains("SIGILL")
        || upperLine.matches(".*SIGNAL\\s+4\\b.*")
        || upperLine.contains("ILLEGAL INSTRUCTION")) {
      return "SIGILL";
    }

    // Check for SIGFPE (signal 8)
    if (upperLine.contains("SIGFPE")
        || upperLine.matches(".*SIGNAL\\s+8\\b.*")
        || upperLine.contains("FLOATING POINT EXCEPTION")) {
      return "SIGFPE";
    }

    // Check for SIGBUS (signal 7)
    if (upperLine.contains("SIGBUS")
        || upperLine.matches(".*SIGNAL\\s+7\\b.*")
        || upperLine.contains("BUS ERROR")) {
      return "SIGBUS";
    }

    return "UNKNOWN";
  }

  /**
   * Extracts signal code information from a signal line to provide context about the signal origin.
   *
   * @param signalLine The signal line from logcat
   * @param signalType The type of signal (e.g., "SIGABRT", "SIGSEGV", "SIGILL", "SIGFPE", "SIGBUS")
   * @return A description of the signal code, or empty string if not determinable
   */
  private String extractSignalCode(String signalLine, String signalType) {
    // Check signal-specific codes first
    if ("SIGABRT".equals(signalType)) {
      if (signalLine.contains("code -6") || signalLine.contains("SI_TKILL")) {
        return "SIGABRT from abort() or assertion failure - check for Abort message in logs";
      }
    } else if ("SIGSEGV".equals(signalType)) {
      if (signalLine.contains("code 1") && signalLine.contains("SEGV_MAPERR")) {
        return "SIGSEGV due to invalid memory access (address not mapped)";
      } else if (signalLine.contains("code 2") && signalLine.contains("SEGV_ACCERR")) {
        return "SIGSEGV due to invalid permissions (access violation)";
      }
    } else if ("SIGILL".equals(signalType)) {
      if (signalLine.contains("code 1") && signalLine.contains("ILL_ILLOPC")) {
        return "SIGILL due to illegal opcode";
      } else if (signalLine.contains("code 2") && signalLine.contains("ILL_ILLOPN")) {
        return "SIGILL due to illegal operand";
      } else if (signalLine.contains("code 3") && signalLine.contains("ILL_PRVOPC")) {
        return "SIGILL due to privileged opcode";
      }
    } else if ("SIGFPE".equals(signalType)) {
      if (signalLine.contains("code 1") && signalLine.contains("FPE_INTDIV")) {
        return "SIGFPE due to integer divide by zero";
      } else if (signalLine.contains("code 2") && signalLine.contains("FPE_INTOVF")) {
        return "SIGFPE due to integer overflow";
      } else if (signalLine.contains("code 3") && signalLine.contains("FPE_FLTDIV")) {
        return "SIGFPE due to floating point divide by zero";
      } else if (signalLine.contains("code 4") && signalLine.contains("FPE_FLTOVF")) {
        return "SIGFPE due to floating point overflow";
      } else if (signalLine.contains("code 5") && signalLine.contains("FPE_FLTUND")) {
        return "SIGFPE due to floating point underflow";
      } else if (signalLine.contains("code 6") && signalLine.contains("FPE_FLTRES")) {
        return "SIGFPE due to floating point inexact result";
      } else if (signalLine.contains("code 7") && signalLine.contains("FPE_FLTINV")) {
        return "SIGFPE due to floating point invalid operation";
      } else if (signalLine.contains("code 8") && signalLine.contains("FPE_FLTSUB")) {
        return "SIGFPE due to subscript out of range";
      }
    } else if ("SIGBUS".equals(signalType)) {
      if (signalLine.contains("code 1") && signalLine.contains("BUS_ADRALN")) {
        return "SIGBUS due to invalid address alignment";
      } else if (signalLine.contains("code 2") && signalLine.contains("BUS_ADRERR")) {
        return "SIGBUS due to non-existent physical address";
      } else if (signalLine.contains("code 3") && signalLine.contains("BUS_OBJERR")) {
        return "SIGBUS due to object-specific hardware error";
      }
    }

    // Generic signal codes that apply to any signal type
    if (signalLine.contains("code 0") && signalLine.contains("SI_USER")) {
      return signalType + " sent by user code (android.os.Process.sendSignal or kill command)";
    } else if (signalLine.contains("SI_QUEUE")) {
      return signalType + " sent via sigqueue()";
    } else if (signalLine.contains("code -1")) {
      return signalType + " from kernel";
    }

    // If we can't determine the code, return empty
    return "";
  }
}
