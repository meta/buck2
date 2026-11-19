/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd;

import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.PrintStream;
import java.util.Objects;
import java.util.function.Function;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.jetbrains.annotations.Nullable;

/**
 * A PrintStream implementation that intercepts stderr output and applies syntax highlighting to
 * compiler messages based on the file type
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ErrorInterceptor extends PrintStream {

  private static final String RED = "\033[91m";
  private static final String YELLOW = "\033[93m";
  private static final String GREEN = "\033[92m";
  private static final String MAGENTA = "\033[95m";
  private static final String BLUE = "\033[94m";
  private static final String BOLD = "\033[1m";
  private static final String RESET = "\033[0m";

  // Source: https://docs.oracle.com/javase/tutorial/java/nutsandbolts/_keywords.html
  private static final String[] JAVA_KEYWORDS = {
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "null",
    "true",
    "false",
    "record",
    "sealed",
    "permits",
    "non-sealed",
    "var"
  };

  // Source: https://kotlinlang.org/docs/keyword-reference.html
  private static final String[] KOTLIN_KEYWORDS = {
    "as",
    "as?",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "!in",
    "interface",
    "is",
    "!is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "val",
    "var",
    "when",
    "while",
    "by",
    "catch",
    "constructor",
    "delegate",
    "dynamic",
    "field",
    "file",
    "finally",
    "get",
    "import",
    "init",
    "param",
    "property",
    "receiver",
    "set",
    "setparam",
    "where",
    "actual",
    "abstract",
    "annotation",
    "companion",
    "const",
    "crossinline",
    "data",
    "enum",
    "expect",
    "external",
    "final",
    "infix",
    "inline",
    "inner",
    "internal",
    "lateinit",
    "noinline",
    "open",
    "operator",
    "out",
    "override",
    "private",
    "protected",
    "public",
    "reified",
    "sealed",
    "suspend",
    "tailrec",
    "vararg",
    "field",
    "it"
  };

  private static final Pattern ERROR_PATTERN =
      Pattern.compile("\\berror\\b(:.*?\\n)((?:.*?\\n)+?\\s*)(\\^+)\\n", Pattern.CASE_INSENSITIVE);
  private static final Pattern WARNING_PATTERN =
      Pattern.compile("\\bwarning\\b(:.*?)\\n", Pattern.CASE_INSENSITIVE);
  private static final Pattern JAVA_FILE_PATTERN =
      Pattern.compile("(?:\\x1b\\[[0-9;]*m)*(/?(?:[\\w-]+/)*[\\w-]+\\.java)\\b:(\\d+):");
  private static final Pattern KOTLIN_FILE_PATTERN =
      Pattern.compile("(?:\\x1b\\[[0-9;]*m)*(/?(?:[\\w-]+/)*[\\w-]+\\.kt)\\b:(\\d+):(\\d+):");

  /** Enum representing supported file types with their associated patterns and keywords. */
  public enum FileType {
    JAVA(JAVA_FILE_PATTERN, JAVA_KEYWORDS),
    KOTLIN(KOTLIN_FILE_PATTERN, KOTLIN_KEYWORDS),
    UNKNOWN(null, new String[0]);

    @Nullable private final Pattern filePattern;
    private final String[] keywords;

    FileType(@Nullable Pattern filePattern, String[] keywords) {
      this.filePattern = filePattern;
      this.keywords = keywords;
    }

    /** Null for {@link #UNKNOWN}, which by definition matches no file. */
    @Nullable
    public Pattern getFilePattern() {
      return filePattern;
    }

    public String[] getKeywords() {
      return keywords;
    }
  }

  public ErrorInterceptor() {
    super(System.err);
  }

  @Override
  public void print(@Nullable String message) {
    super.print(prettyPrint(message));
  }

  @Nullable
  public static String prettyPrint(@Nullable String exception) {
    if (exception == null || exception.isEmpty()) {
      return exception;
    }

    if (System.getenv("NO_COLOR") != null) {
      return exception;
    }

    FileType fileType = determineFileType(exception);

    switch (fileType) {
      case JAVA:
        return prettyPrintJavaException(exception);
      case KOTLIN:
        return prettyPrintKotlinException(exception);
      default:
        return exception;
    }
  }

  // Creates a clickable hyperlink in the terminal using OSC 8 escape sequences.
  // Supports both VS Code and Android Studio links based on ANDROID_EDITOR environment variable.
  private static String createHyperlink(String file, int line, String text) {
    // Keep in sync with fbcode/buck2/prelude/java/tools/utils.py
    boolean isVsCode =
        "vscode".equals(System.getenv("TERM_PROGRAM"))
            || "od".equals(System.getenv("FBVSCODE_REMOTE_ENV_NAME"));

    boolean isHyperlinkDisabled =
        new File(System.getProperty("user.home") + "/.disable_buck_jvm_path_hyperlink").exists();

    if (isVsCode || isHyperlinkDisabled) {
      return text;
    }

    String OSC = "\033]";
    String ST = "\033\\";
    String uri;

    boolean isJetBrains =
        System.getenv("ANDROID_EDITOR") != null
            || new File(
                    System.getProperty("user.home")
                        + "/.jetbrains-fb/.buck_path_hyperlink_uses_jetbrains")
                .isFile();

    if (isJetBrains) {
      uri = "fb-ide-opener://open/?ide=intellij&filepath=/fbsource/" + file + "&line=" + line;
    } else {
      uri =
          "https://www.internalfb.com/intern/nuclide/open/arc/?project=fbsource&paths[0]="
              + file
              + "&lines[0]="
              + line;
    }

    return OSC + "8;;" + uri + ST + text + OSC + "8;;" + ST;
  }

  private static FileType determineFileType(String exception) {
    for (FileType type : FileType.values()) {
      Pattern filePattern = type.getFilePattern();
      if (filePattern != null && filePattern.matcher(exception).find()) {
        return type;
      }
    }

    // Default to "UNKNOWN" if file type cannot be determined
    return FileType.UNKNOWN;
  }

  private static String prettyPrintJavaException(String exception) {
    exception =
        colorizePattern(
            ERROR_PATTERN, exception, match -> highlightError(match, FileType.JAVA.getKeywords()));
    exception = colorizePattern(WARNING_PATTERN, exception, ErrorInterceptor::highlightWarning);
    exception = colorizePattern(JAVA_FILE_PATTERN, exception, ErrorInterceptor::highlightJavaFile);
    return exception;
  }

  private static String prettyPrintKotlinException(String exception) {
    exception =
        colorizePattern(
            ERROR_PATTERN,
            exception,
            match -> highlightError(match, FileType.KOTLIN.getKeywords()));
    exception = colorizePattern(WARNING_PATTERN, exception, ErrorInterceptor::highlightWarning);
    exception =
        colorizePattern(KOTLIN_FILE_PATTERN, exception, ErrorInterceptor::highlightKotlinFile);
    return exception;
  }

  /**
   * Applies a colorizer function to all matches of a pattern in a string.
   *
   * @param pattern The pattern to match
   * @param message The string to search
   * @param colorizer The function to apply to each match
   * @return The colorized string
   */
  private static String colorizePattern(
      Pattern pattern, String message, Function<Matcher, String> colorizer) {
    Matcher matcher = pattern.matcher(message);
    StringBuffer result = new StringBuffer();
    while (matcher.find()) {
      String replacement = colorizer.apply(matcher);
      matcher.appendReplacement(result, Matcher.quoteReplacement(replacement));
    }
    matcher.appendTail(result);
    return result.toString();
  }

  private static String highlightError(Matcher match, String[] keywords) {
    return RED
        + "error"
        + RESET
        + BOLD
        + match.group(1)
        + RESET
        + highlightCode(Objects.requireNonNull(match.group(2)), keywords)
        + RED
        + match.group(3)
        + RESET
        + "\n";
  }

  private static String highlightWarning(Matcher match) {
    return YELLOW + "warning" + RESET + BOLD + match.group(1) + RESET + "\n";
  }

  private static String highlightJavaFile(Matcher match) {
    String file = Objects.requireNonNull(match.group(1));
    int line = Integer.parseInt(Objects.requireNonNull(match.group(2)));

    return GREEN
        + createHyperlink(file, line, file)
        + RESET
        + ":"
        + MAGENTA
        + match.group(2)
        + RESET
        + ":";
  }

  private static String highlightKotlinFile(Matcher match) {
    String file = Objects.requireNonNull(match.group(1));
    int line = Integer.parseInt(Objects.requireNonNull(match.group(2)));

    return GREEN
        + createHyperlink(file, line, file)
        + RESET
        + ":"
        + MAGENTA
        + match.group(2)
        + RESET
        + ":"
        + MAGENTA
        + match.group(3)
        + RESET
        + ":";
  }

  private static String highlightCode(String code, String[] keywords) {
    return code.replaceAll("\\b(" + String.join("|", keywords) + ")\\b", BLUE + "$1" + RESET);
  }
}
