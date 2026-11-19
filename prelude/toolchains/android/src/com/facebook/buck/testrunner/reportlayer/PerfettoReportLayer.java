/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner.reportlayer;

import com.facebook.buck.testrunner.InstrumentationTestRunner;
import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.FileWriter;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.jspecify.annotations.Nullable;

/** Report layer to collect Perfetto system traces during instrumentation tests. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PerfettoReportLayer extends ReportLayer {

  public static final String ARG = "--collect-perfetto";

  private static final String PERFETTO_TRACE_DEVICE_DIR = "/data/misc/perfetto-traces/";
  private static final String PERFETTO_TRACE_FILENAME = "ait_perfetto_trace.perfetto-trace";
  private static final String PERFETTO_CONFIG_DEVICE_PATH = "/data/local/tmp/perfetto_config.txt";
  private static final Pattern PID_PATTERN = Pattern.compile("\\d+");

  // Default Perfetto config: ftrace events (sched, cpu freq/idle, wakeup),
  // process_stats, sys_stats, 64KB buffer with RING_BUFFER, 600s max duration, 100MB max file size
  private static final String PERFETTO_CONFIG =
      "buffers { size_kb: 65536 fill_policy: RING_BUFFER } "
          + "data_sources { config { name: \"linux.ftrace\" "
          + "ftrace_config { ftrace_events: \"sched/sched_switch\" "
          + "ftrace_events: \"power/cpu_frequency\" "
          + "ftrace_events: \"power/cpu_idle\" "
          + "ftrace_events: \"sched/sched_wakeup\" } } } "
          + "data_sources { config { name: \"linux.process_stats\" } } "
          + "data_sources { config { name: \"linux.sys_stats\" } } "
          + "duration_ms: 600000 "
          + "max_file_size_bytes: 104857600";

  private @Nullable String perfettoPid = null;

  public PerfettoReportLayer(InstrumentationTestRunner runner) {
    super(runner);
  }

  @Override
  public void initialize() {
    try {
      String tracePath = PERFETTO_TRACE_DEVICE_DIR + PERFETTO_TRACE_FILENAME;

      // Clean up any existing trace file
      this.runner.runShellCommand("rm -f " + tracePath);

      // Check if perfetto binary is available on the device
      try {
        this.runner.runShellCommand("which perfetto");
      } catch (Exception e) {
        System.err.println(
            "PerfettoReportLayer: 'perfetto' binary not found on device. "
                + "Perfetto tracing will not be available. Error: "
                + e.getMessage());
        return;
      }

      // Write perfetto config to a local temp file, then push it to the device.
      // This avoids using echo with pipes through adb shell, which can be
      // unreliable due to argument splitting in AdbUtils.
      String configPath = PERFETTO_CONFIG_DEVICE_PATH;
      File localConfigFile = File.createTempFile("perfetto-config-", ".txt");
      try {
        try (FileWriter writer = new FileWriter(localConfigFile)) {
          writer.write(PERFETTO_CONFIG);
        }
        this.runner.pushFileWithSyncService(localConfigFile.getAbsolutePath(), configPath);
      } finally {
        localConfigFile.delete();
      }

      String startCmd =
          String.format("perfetto --txt --background --config %s --out %s", configPath, tracePath);
      String output = this.runner.runShellCommand(startCmd);

      // Extract PID from perfetto output.
      // perfetto --background returns the PID in different formats depending on version:
      //   - "[NNN]" (older versions)
      //   - "NNN" (plain number, newer versions)
      if (output != null) {
        Matcher matcher = PID_PATTERN.matcher(output.trim());
        if (matcher.find()) {
          perfettoPid = matcher.group();
        }
      }

      if (perfettoPid != null) {
        System.out.printf("PerfettoReportLayer: Perfetto started with PID: %s%n", perfettoPid);
      } else {
        System.err.println(
            "PerfettoReportLayer: Warning: Could not determine Perfetto PID from output: "
                + output);
      }
    } catch (Exception e) {
      System.err.printf("PerfettoReportLayer: Failed to start Perfetto tracing: %s%n", e);
    }
  }

  @Override
  public void report() {
    String tracePath = PERFETTO_TRACE_DEVICE_DIR + PERFETTO_TRACE_FILENAME;
    try {
      // Stop perfetto gracefully via SIGTERM
      if (perfettoPid != null) {
        this.runner.runShellCommand("kill -TERM " + perfettoPid);
        // Wait for perfetto to flush and finalize the trace
        Thread.sleep(3000);
      } else {
        return;
      }

      // Check if trace file exists on device
      try {
        String lsOutput = this.runner.runShellCommand("ls -la " + tracePath);
        System.out.printf("PerfettoReportLayer: Trace file on device: %s%n", lsOutput);
      } catch (Exception e) {
        System.err.printf(
            "PerfettoReportLayer: Trace file not found on device at %s: %s%n", tracePath, e);
        return;
      }

      Path artifact =
          this.runner.createTRA("generic_blob", "Perfetto trace", PERFETTO_TRACE_FILENAME);
      if (artifact == null) {
        System.err.println(
            "PerfettoReportLayer: Cannot report Perfetto trace: TRA directory not configured");
        return;
      }

      this.runner.pullFileWithSyncService(tracePath, artifact.toString());

      if (!Files.exists(artifact) || Files.size(artifact) == 0) {
        System.err.println(
            "PerfettoReportLayer: Warning: Perfetto trace file is empty or missing after pull");
      } else {
        System.out.printf(
            "PerfettoReportLayer: Successfully collected Perfetto trace (%d bytes)%n",
            Files.size(artifact));
      }
    } catch (Exception e) {
      System.err.printf("PerfettoReportLayer: Failed to collect Perfetto trace: %s%n", e);
    } finally {
      // Clean up trace and config from device
      try {
        this.runner.runShellCommand("rm -f " + tracePath);
        this.runner.runShellCommand("rm -f " + PERFETTO_CONFIG_DEVICE_PATH);
      } catch (Exception e) {
        System.err.printf("PerfettoReportLayer: Failed to clean up device files: %s%n", e);
      }
    }
  }
}
