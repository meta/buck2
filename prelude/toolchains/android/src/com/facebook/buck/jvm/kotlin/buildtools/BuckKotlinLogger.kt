/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.buildtools

import org.jetbrains.kotlin.buildtools.api.KotlinLogger // @oss-enable
import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.jvm.kotlin.buildtools.CompilerMessageFormatter.Severity
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
// @oss-disable: import com.facebook.kotlin.compilercompat.KotlinLoggerCompat
import java.io.PrintStream
import java.util.logging.Level

internal class BuckKotlinLogger(
    private val stdErr: PrintStream,
    private val loggingContext: KotlinCDLoggingContext,
)
: KotlinLogger // @oss-enable
// @oss-disable: : KotlinLoggerCompat()
{

  private val messageFormatter = CompilerMessageFormatter()

  override val isDebugEnabled: Boolean
    get() = LOG.isDebugEnabled

  override fun debug(msg: String) {
    if (!LOG.isDebugEnabled) return
    LOG.debug(msg)
  }

  override fun error(msg: String, throwable: Throwable?) {
    if (!LOG.isLoggable(Level.SEVERE)) return
    val severity = if (throwable == null) Severity.ERROR else Severity.EXCEPTION
    stdErr.println(messageFormatter.format(severity, msg))
    throwable?.printStackTrace(stdErr)
  }

  override fun info(msg: String) {
    if (msg.startsWith("Non-incremental compilation will be performed")) {
      loggingContext.addExtras(BuckKotlinLogger::class.java.simpleName, msg)
    }
    if (msg.startsWith("KOTLIN_BUILD_METRIC|")) {
      loggingContext.addExtras("BuildMetrics", msg.removePrefix("KOTLIN_BUILD_METRIC|"))
    }

    if (!LOG.isLoggable(Level.INFO)) return
    LOG.info(msg)
  }

  override fun lifecycle(msg: String) {
    if (!LOG.isLoggable(Level.INFO)) return
    LOG.info(msg)
  }

  override fun warn(msg: String) {
    if (!LOG.isLoggable(Level.WARNING)) return
    stdErr.println(messageFormatter.format(Severity.WARNING, msg))
  }

  // OSS-only: the internal `KotlinLoggerCompat` supertype lacks this Kotlin 2.2
  // overload on some toolchain configurations.
    override fun warn(msg: String, throwable: Throwable?) { // @oss-enable
      if (!LOG.isLoggable(Level.WARNING)) return // @oss-enable
      stdErr.println(messageFormatter.format(Severity.WARNING, msg)) // @oss-enable
      throwable?.printStackTrace(stdErr) // @oss-enable
    } // @oss-enable

  companion object {
    private val LOG: Logger = Logger.get(BuckKotlinLogger::class.java)
  }
}
