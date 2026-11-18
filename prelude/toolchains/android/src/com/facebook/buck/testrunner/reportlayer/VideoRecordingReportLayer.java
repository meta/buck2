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
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Path;
import java.util.Objects;
import java.util.concurrent.TimeUnit;
import java.util.regex.Pattern;
import org.jspecify.annotations.Nullable;

/**
 * report layer to start a process to record video of tests before they run and collect the video as
 * TRA after tests finish
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class VideoRecordingReportLayer extends ReportLayer {

  private static final String VIDEO_RECORDING_STORAGE_PATH =
      "/sdcard/test_result/video_recordings/";
  private static final String VIDEO_RECORDING_SCRIPT_NAME = "test-video-record.sh";
  private static final String VIDEO_RECORDING_FILE_NAME = "test-video-record.mp4";
  private static final int SCREENRECORD_STOP_GRACE_SECONDS = 10;
  private static final long RECORDING_PROCESS_STOP_TIMEOUT_SECONDS = 60;
  private static final long KILL_SCREENRECORD_TIMEOUT_SECONDS = 10;

  public static final String ARG = "--record-video";

  private @Nullable Process videoRecordingProcess;

  public VideoRecordingReportLayer(InstrumentationTestRunner runner) {
    super(runner);
  }

  @Override
  public void initialize() {
    this.videoRecordingProcess = this.startVideoRecording();
  }

  @Override
  public void report() {
    this.collectVideoRecording();
  }

  private String getVideoRecordingStoragePath() {
    return String.format(VIDEO_RECORDING_STORAGE_PATH, this.runner.getPackageName());
  }

  private @Nullable Process startVideoRecording() {
    try {
      File scriptFile = File.createTempFile("video-recording-", ".sh");
      FileWriter scriptFileWriter = new FileWriter(scriptFile);
      // create local script file
      scriptFileWriter.write(
          String.join(
              "\n",
              "exec 2>&1  # merge stdout and stderr",
              "set -xue",
              String.format(
                  "screenrecord --verbose %s &",
                  String.format(
                      "%s/%s", this.getVideoRecordingStoragePath(), VIDEO_RECORDING_FILE_NAME)),
              "pid=$!  # screenrecord pid",
              "read  # wait for stopword (any input)",
              "kill -INT $pid  # request screenrecord to stop gracefully",
              "# screenrecord can hang while stopping: force-kill it if it outlives the grace"
                  + " period",
              String.format(
                  "(sleep %d; kill -KILL $pid) </dev/null >/dev/null 2>&1 &",
                  SCREENRECORD_STOP_GRACE_SECONDS),
              "watchdog=$!",
              "wait $pid",
              "kill $watchdog 2>/dev/null || true"));
      scriptFile.deleteOnExit();
      scriptFileWriter.close();
      // push script to device
      String remoteScriptPath =
          String.format("%s/%s", this.getVideoRecordingStoragePath(), VIDEO_RECORDING_SCRIPT_NAME);
      this.runner.pushFileWithSyncService(scriptFile.getAbsolutePath(), remoteScriptPath);
      // start recording process via runner.exec() so it can be mocked in tests
      String command = String.format("adb shell sh %s", remoteScriptPath);
      return this.runner.exec(command);
    } catch (Exception e) {
      System.err.printf("Failed to start video recording process with error: %s\n", e);
      return null;
    }
  }

  private void collectVideoRecording() {
    try {
      // stop video recording
      Process videoRecordingProcess = Objects.requireNonNull(this.videoRecordingProcess);
      OutputStream videoRecordingInput =
          Objects.requireNonNull(videoRecordingProcess.getOutputStream());
      videoRecordingInput.write("STOP".getBytes());
      videoRecordingInput.write(System.lineSeparator().getBytes());
      videoRecordingInput.flush();
      if (!videoRecordingProcess.waitFor(
          RECORDING_PROCESS_STOP_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
        videoRecordingProcess.destroyForcibly();
        killScreenrecordOnDevice();
        throw new Exception(
            String.format(
                "Video recording process did not exit within %d seconds, Output: %s",
                RECORDING_PROCESS_STOP_TIMEOUT_SECONDS, readOutput(videoRecordingProcess)));
      }
      String output = readOutput(videoRecordingProcess);
      int exitCode = videoRecordingProcess.waitFor();
      if (exitCode != 0) {
        // check whether screenrecord terminated before we tried to kill it. We don't want to throw
        // an error in that case.
        Pattern noProcessKilled = Pattern.compile("kill: \\d+: No such process");
        if (!noProcessKilled.matcher(output).find()) {
          throw new Exception(
              String.format(
                  "Failed to stop video recording process with exit code %d, Output: %s",
                  exitCode, output));
        }
      }
      // pull file to TRA
      Path video_artifact =
          this.runner.createTRA(
              "video_recording_test_artifact", "video recording", VIDEO_RECORDING_FILE_NAME);
      if (null == video_artifact) {
        throw new Exception("Failed to create TRA for video recording");
      }
      this.runner.pullFileWithSyncService(
          String.format("%s/%s", this.getVideoRecordingStoragePath(), VIDEO_RECORDING_FILE_NAME),
          video_artifact.toString());
    } catch (Exception e) {
      System.err.printf("Failed to collect video recording process with error: %s", e);
    }
  }

  private static String readOutput(Process process) {
    try (InputStream is = process.getInputStream()) {
      return new String(readAllBytes(is));
    } catch (IOException e) {
      return "IOError: " + e.getMessage() + "\n";
    }
  }

  private void killScreenrecordOnDevice() {
    try {
      this.runner
          .exec("adb shell pkill -KILL screenrecord")
          .waitFor(KILL_SCREENRECORD_TIMEOUT_SECONDS, TimeUnit.SECONDS);
    } catch (IOException e) {
      System.err.printf("Failed to kill screenrecord on device: %s\n", e);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
    }
  }

  private static byte[] readAllBytes(InputStream inputStream) throws IOException {
    final int bufLen = 1024;
    byte[] buf = new byte[bufLen];
    int readLen;

    ByteArrayOutputStream outputStream = new ByteArrayOutputStream();

    while ((readLen = inputStream.read(buf, 0, bufLen)) != -1) {
      outputStream.write(buf, 0, readLen);
    }

    return outputStream.toByteArray();
  }
}
