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

import com.android.ddmlib.FileListingService;
import com.android.ddmlib.IDevice;
import com.android.ddmlib.IShellEnabledDevice;
import com.android.ddmlib.IShellOutputReceiver;
import com.android.ddmlib.testrunner.ITestRunListener;
import com.android.ddmlib.testrunner.InstrumentationResultParser;
import com.android.ddmlib.testrunner.RemoteAndroidTestRunner;
import com.facebook.buck.android.TestAndroidDevice;
import com.facebook.buck.android.TestDevice;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import com.facebook.buck.testrunner.reportlayer.LogExtractorReportLayer;
import com.facebook.buck.testrunner.reportlayer.PerfettoReportLayer;
import com.facebook.buck.testrunner.reportlayer.TombstonesReportLayer;
import com.facebook.buck.testrunner.reportlayer.VideoRecordingReportLayer;
import com.facebook.buck.testutil.TemporaryPaths;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.google.common.collect.ImmutableSet;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.reflect.Field;
import java.net.URI;
import java.nio.file.FileSystem;
import java.nio.file.FileSystems;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.TimeUnit;
import javax.xml.parsers.DocumentBuilderFactory;
import org.junit.Assert;
import org.junit.Rule;
import org.junit.Test;

/** Tests {@link InstrumentationTestRunner} */
public class InstrumentationTestRunnerTest {
  private Process execProcessOverride = null;

  @Rule public final TemporaryPaths tmp = new TemporaryPaths();

  @Test
  public void structuredResultsReportRunFailureWithoutXml() throws Throwable {
    Path reportDir = tmp.newFolder("structured_reports").getPath();
    Path jsonPath = reportDir.resolve("results.json");
    Map<String, String> env = new HashMap<>();
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            new HashMap<>(), env, "--output", reportDir.toString());
    List<ITestRunListener> listeners = new ArrayList<>();
    TestResultsOutputSender sender =
        TestResultsOutputSender.fromFilePath(jsonPath.toString()).get();
    try {
      ITestRunListener resultListener = runner.addResultListeners(listeners, Optional.of(sender));
      Assert.assertEquals(1, listeners.size());
      Assert.assertSame(listeners.get(0), resultListener);
      resultListener.testRunStarted("setup_crash", 0);
      resultListener.testRunFailed("instrumentation crashed before discovering tests");
      resultListener.testRunEnded(0, new HashMap<>());
    } finally {
      sender.close();
    }

    JsonNode failure = new ObjectMapper().readTree(Files.readString(jsonPath)).get("run_failure");
    Assert.assertEquals(2, failure.get("status").asInt());
    Assert.assertEquals(
        "instrumentation crashed before discovering tests", failure.get("details").asText());
    try (var files = Files.list(reportDir)) {
      Assert.assertEquals(1, files.count());
    }
  }

  @Test
  public void missingStructuredSenderUsesXml() throws Throwable {
    Path reportDir = tmp.newFolder("xml_reports").getPath();
    Map<String, String> env = new HashMap<>();
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            new HashMap<>(), env, "--output", reportDir.toString());
    List<ITestRunListener> listeners = new ArrayList<>();
    ITestRunListener resultListener = runner.addResultListeners(listeners, Optional.empty());
    Assert.assertEquals(1, listeners.size());
    Assert.assertSame(listeners.get(0), resultListener);
    resultListener.testRunStarted("setup_crash", 0);
    resultListener.testRunFailed("setup failed");
    resultListener.testRunEnded(0, new HashMap<>());
    var document =
        DocumentBuilderFactory.newInstance()
            .newDocumentBuilder()
            .parse(reportDir.resolve(BuckXmlTestRunListener.TEST_RESULT_FILE).toFile());
    Assert.assertEquals(1, document.getElementsByTagName("failure").getLength());
  }

  @Test
  public void launcherReturnsFailureWhenRunnerThrows() {
    int exitCode =
        InstrumentationMain.runForExitCode(
            () -> {
              throw new IOException("runner failed before reporting results");
            });

    Assert.assertEquals(1, exitCode);
  }

  @Test
  public void launcherReturnsSuccessWhenRunnerCompletes() {
    Assert.assertEquals(0, InstrumentationMain.runForExitCode(() -> {}));
  }

  /** Just verifies the reflection is legit */
  @Test
  public void testSetTrimLinesHappyPath() throws Throwable {
    IShellEnabledDevice shellEnabledDevice = new TestDevice();
    RemoteAndroidTestRunner runner =
        new RemoteAndroidTestRunner("foobar", "blah", shellEnabledDevice);

    Field field = RemoteAndroidTestRunner.class.getDeclaredField("mParser");
    field.setAccessible(true);
    field.set(runner, new InstrumentationResultParser("fooBar", new ArrayList<>()));

    InstrumentationTestRunner.setTrimLine(runner, true);
  }

  @Test
  public void testSyncExopackageDir() throws Exception {
    final Set<String> pushedFilePaths = new HashSet<>();
    Path rootFolder = tmp.newFolder("dummy_exo_contents").getPath();
    // Set up the files to be written, including the metadata file which specifies the base
    // directory
    Files.write(
        rootFolder.resolve("metadata.txt"), "/data/local/tmp/exopackage/com.example".getBytes());
    Path contents = rootFolder.resolve(Paths.get("a", "b", "c"));
    Files.createDirectories(contents);
    Files.write(contents.resolve("foo"), "Hello World".getBytes());

    // Perform the sync with a lambda that tracks pushed files
    InstrumentationTestRunner.syncExopackageDir(
        rootFolder, (localPath, remotePath) -> pushedFilePaths.add(remotePath));

    // Now verify the results
    Set<String> expectedPaths =
        ImmutableSet.of(
            "/data/local/tmp/exopackage/com.example/metadata.txt",
            "/data/local/tmp/exopackage/com.example/a/b/c/foo");
    Assert.assertEquals(expectedPaths, pushedFilePaths);
  }

  @Test
  public void runsTestAndClearsLogcat() throws Throwable {
    ArrayList<String> shellCommands = new ArrayList<>();
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            shellCommands, new HashMap<>(), new HashMap<>(), new HashMap<>());
    runner.run();
    List<String> expectedCommands =
        Arrays.asList(
            "logcat -G 16M",
            "logcat -c",
            "am instrument -w -r   com.example.test/com.example.test.TestRunner",
            "logcat -G 256K");
    Assert.assertEquals(expectedCommands, shellCommands);
  }

  @Test
  public void clearsPackageData() throws Throwable {
    ArrayList<String> shellCommands = new ArrayList<>();

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            shellCommands,
            new HashMap<>(),
            new HashMap<>(),
            new HashMap<>(),
            "--clear-package-data");
    runner.run();
    List<String> expectedCommands =
        Arrays.asList(
            "pm clear com.example.test",
            "pm clear com.example",
            "logcat -G 16M",
            "logcat -c",
            "am instrument -w -r   com.example.test/com.example.test.TestRunner",
            "logcat -G 256K");
    Assert.assertEquals(expectedCommands, shellCommands);
  }

  @Test
  public void disablesAnimations() throws Throwable {
    ArrayList<String> shellCommands = new ArrayList<>();
    Map<String, String> mockShellResponses =
        new HashMap<String, String>() {
          {
            put("settings get global window_animation_scale", "1.0");
            put("settings get global transition_animation_scale", "0.9");
            put("settings get global animator_duration_scale", "0.8");
          }
        };

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            shellCommands,
            new HashMap<>(),
            new HashMap<>(),
            mockShellResponses,
            "--disable-animations");
    runner.run();
    Assert.assertEquals(13, shellCommands.size());

    // Verify that the animations are disabled
    Assert.assertTrue(shellCommands.contains("settings put global window_animation_scale 0.0"));
    Assert.assertTrue(shellCommands.contains("settings put global transition_animation_scale 0.0"));
    Assert.assertTrue(shellCommands.contains("settings put global animator_duration_scale 0.0"));

    // Verify that the animations are restored
    List<String> lastFourCommands =
        shellCommands.subList(shellCommands.size() - 4, shellCommands.size());
    Assert.assertTrue(lastFourCommands.contains("settings put global window_animation_scale 1.0"));
    Assert.assertTrue(
        lastFourCommands.contains("settings put global transition_animation_scale 0.9"));
    Assert.assertTrue(lastFourCommands.contains("settings put global animator_duration_scale 0.8"));
  }

  @Test
  public void extractLogWithRegex() throws Throwable {
    // set up TRA
    Map<String, String> env = new HashMap<>();
    Path tra = Files.createTempDirectory("ait-tra-");
    Path tra_annot = Files.createTempDirectory("ait-tra-annot-");
    env.put("TEST_RESULT_ARTIFACTS_DIR", tra.toString());
    env.put("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR", tra_annot.toString());

    List<String> expectedShellCommands = new ArrayList<>();
    expectedShellCommands.add("logcat -G 16M");
    expectedShellCommands.add("logcat -c");
    expectedShellCommands.add("am instrument -w -r   com.example.test/com.example.test.TestRunner");
    expectedShellCommands.add("logcat -d -b \"crash\"");
    expectedShellCommands.add("logcat -d -b \"system\"");
    expectedShellCommands.add("logcat -d -b \"main\"");

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            new HashMap<>(), env, "--log-extractor", "Breadcrumbs=TEST_BREADCRUMBS:");
    runner.run();
    Path breadcrumbsTra = tra.resolve("Breadcrumbs");
    Path breadcrumbsTraAnnotation = tra_annot.resolve("Breadcrumbs.annotation");
    Assert.assertTrue(Files.exists(breadcrumbsTra));
    Assert.assertTrue(Files.exists(breadcrumbsTraAnnotation));

    Assert.assertEquals(
        "{\"type\": {\"formatted_log\": {\"log_source\": 5}}, \"description\": \"Breadcrumbs\"}",
        Files.readString(breadcrumbsTraAnnotation));
  }

  @Test
  public void collectVideoRecordingToTRA() throws Throwable {
    // set up TRA
    Map<String, String> env = new HashMap<>();
    Path tra = Files.createTempDirectory("ait-tra-");
    Path tra_annot = Files.createTempDirectory("ait-tra-annot-");
    env.put("TEST_RESULT_ARTIFACTS_DIR", tra.toString());
    env.put("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR", tra_annot.toString());

    ArrayList<String> expectedShellCommands = new ArrayList<>();
    expectedShellCommands.add("logcat -G 16M");
    expectedShellCommands.add("logcat -c");
    expectedShellCommands.add("am instrument -w -r   com.example.test/com.example.test.TestRunner");
    expectedShellCommands.add("logcat -d -b \"crash\"");
    expectedShellCommands.add("logcat -d -b \"system\"");
    expectedShellCommands.add("logcat -d -b \"main\"");
    expectedShellCommands.add("logcat -G 256K");

    Map<Path, byte[]> files = new HashMap<>();
    files.put(
        Paths.get("/sdcard/test_result/video_recordings/test-video-record.mp4"),
        "test-video-record.mp4".getBytes());
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(files, env, "--record-video");
    runner.run();
    Path video_file = tra.resolve("test-video-record.mp4");
    Path video_file_annotation = tra_annot.resolve("test-video-record.mp4.annotation");
    Assert.assertTrue(Files.exists(video_file_annotation));
    Assert.assertTrue(Files.exists(video_file));

    Assert.assertEquals(
        "{\"type\": {\"video_recording_test_artifact\": {}}, \"description\": \"video recording\"}",
        Files.readString(video_file_annotation));
  }

  @Test
  public void videoRecordingThatDoesNotStopIsKilledAndRunCompletes() throws Throwable {
    Map<String, String> env = new HashMap<>();
    env.put("TEST_RESULT_ARTIFACTS_DIR", Files.createTempDirectory("ait-tra-").toString());
    env.put(
        "TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR",
        Files.createTempDirectory("ait-tra-annot-").toString());
    HangingProcess hangingProcess = new HangingProcess();
    execProcessOverride = hangingProcess;

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(new HashMap<>(), env, "--record-video");
    runner.run();

    Assert.assertTrue(hangingProcess.destroyed);
  }

  /** A process that never exits on its own, like a screenrecord session stuck while stopping. */
  private static class HangingProcess extends Process {
    boolean destroyed = false;

    @Override
    public OutputStream getOutputStream() {
      return new java.io.ByteArrayOutputStream();
    }

    @Override
    public InputStream getInputStream() {
      return new ByteArrayInputStream(new byte[0]);
    }

    @Override
    public InputStream getErrorStream() {
      return new ByteArrayInputStream(new byte[0]);
    }

    @Override
    public int waitFor() {
      throw new AssertionError("waitFor() without a timeout would block forever");
    }

    @Override
    public boolean waitFor(long timeout, TimeUnit unit) {
      return destroyed;
    }

    @Override
    public int exitValue() {
      if (!destroyed) {
        throw new IllegalThreadStateException();
      }
      return 137;
    }

    @Override
    public void destroy() {
      destroyed = true;
    }

    @Override
    public Process destroyForcibly() {
      destroyed = true;
      return this;
    }
  }

  @Test
  public void reportsTombstonesToTRA() throws Throwable {
    // set up TRA
    Map<String, String> env = new HashMap<>();
    Path tra = Files.createTempDirectory("ait-tra-");
    Path tra_annot = Files.createTempDirectory("ait-tra-annot-");
    env.put("TEST_RESULT_ARTIFACTS_DIR", tra.toString());
    env.put("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR", tra_annot.toString());

    ArrayList<String> expectedShellCommands = new ArrayList<>();
    expectedShellCommands.add("logcat -G 16M");
    expectedShellCommands.add("logcat -c");
    expectedShellCommands.add("am instrument -w -r   com.example.test/com.example.test.TestRunner");
    expectedShellCommands.add("logcat -d -b \"crash\"");
    expectedShellCommands.add("logcat -d -b \"system\"");
    expectedShellCommands.add("logcat -d -b \"main\"");
    expectedShellCommands.add("logcat -G 256K");
    // Provide mock response for shell command to check directory existence
    Map<String, String> mockShellResponses = new HashMap<>();
    mockShellResponses.put("test -d /data/tombstones/ && echo exists", "exists");
    Map<Path, byte[]> files = new HashMap<>();
    files.put(Paths.get("/data/tombstones/tombstone"), "tombstone".getBytes());
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            files, env, mockShellResponses, "--collect-tombstones");
    runner.run();
    Path tombstone_zip = tra.resolve("tombstones.zip");
    Path tombstone_annotation = tra_annot.resolve("tombstones.zip.annotation");
    Assert.assertTrue(Files.exists(tombstone_zip));
    Assert.assertTrue(Files.exists(tombstone_annotation));

    Assert.assertEquals(
        "{\"type\": {\"generic_blob\": {}}, \"description\": \"Zip file with all tombstones\"}",
        Files.readString(tombstone_annotation));

    URI uri = URI.create("jar:" + tombstone_zip.toUri());
    HashMap<String, String> zipenv = new HashMap<>();
    zipenv.put("create", "false");
    try (FileSystem zipfs = FileSystems.newFileSystem(uri, zipenv)) {
      Path tombstone = zipfs.getPath("tombstone");
      Assert.assertEquals("tombstone", Files.readString(tombstone));
    }
  }

  @Test
  public void reportsTombstonesDirectoryToTRA() throws Throwable {
    // set up TRA
    Map<String, String> env = new HashMap<>();
    Path tra = Files.createTempDirectory("ait-tra-");
    Path tra_annot = Files.createTempDirectory("ait-tra-annot-");
    env.put("TEST_RESULT_ARTIFACTS_DIR", tra.toString());
    env.put("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR", tra_annot.toString());

    ArrayList<String> expectedShellCommands = new ArrayList<>();
    expectedShellCommands.add("logcat -G 16M");
    expectedShellCommands.add("logcat -c");
    expectedShellCommands.add("am instrument -w -r   com.example.test/com.example.test.TestRunner");
    expectedShellCommands.add("logcat -d -b \"crash\"");
    expectedShellCommands.add("logcat -d -b \"system\"");
    expectedShellCommands.add("logcat -d -b \"main\"");
    expectedShellCommands.add("logcat -G 256K");
    // Provide mock responses for shell commands
    Map<String, String> mockShellResponses = new HashMap<>();
    mockShellResponses.put("test -d /data/tombstones/ && echo exists", "exists");
    Map<Path, byte[]> files = new HashMap<>();
    files.put(Paths.get("/data/tombstones/subdir/tombstone"), "tombstone".getBytes());
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            files, env, mockShellResponses, "--collect-tombstones");
    runner.run();
    Path tombstone_zip = tra.resolve("tombstones.zip");
    Path tombstone_annotation = tra_annot.resolve("tombstones.zip.annotation");
    Assert.assertTrue(Files.exists(tombstone_zip));
    Assert.assertTrue(Files.exists(tombstone_annotation));

    Assert.assertEquals(
        "{\"type\": {\"generic_blob\": {}}, \"description\": \"Zip file with all tombstones\"}",
        Files.readString(tombstone_annotation));

    URI uri = URI.create("jar:" + tombstone_zip.toUri());
    HashMap<String, String> zipenv = new HashMap<>();
    zipenv.put("create", "false");
    try (FileSystem zipfs = FileSystems.newFileSystem(uri, zipenv)) {
      Path tombstone = zipfs.getPath("subdir/tombstone");
      Assert.assertEquals("tombstone", Files.readString(tombstone));
    }
  }

  @Test
  public void collectPerfettoTraceToTRA() throws Throwable {
    Map<String, String> env = new HashMap<>();
    Path tra = Files.createTempDirectory("ait-tra-");
    Path tra_annot = Files.createTempDirectory("ait-tra-annot-");
    env.put("TEST_RESULT_ARTIFACTS_DIR", tra.toString());
    env.put("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR", tra_annot.toString());

    Map<Path, byte[]> files = new HashMap<>();
    files.put(
        Paths.get("/data/misc/perfetto-traces/ait_perfetto_trace.perfetto-trace"),
        "fake perfetto trace data".getBytes());

    // Mock shell command responses for PerfettoReportLayer
    Map<String, String> mockResponses = new HashMap<>();
    mockResponses.put(
        "perfetto --txt --background --config /data/local/tmp/perfetto_config.txt"
            + " --out /data/misc/perfetto-traces/ait_perfetto_trace.perfetto-trace",
        "[123]");
    mockResponses.put(
        "ls -la /data/misc/perfetto-traces/ait_perfetto_trace.perfetto-trace",
        "-rw-r--r-- 1 root root 1234 Jan 1 00:00 ait_perfetto_trace.perfetto-trace");

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(files, env, mockResponses, "--collect-perfetto");
    runner.run();

    Path traceFile = tra.resolve("ait_perfetto_trace.perfetto-trace");
    Path traceAnnotation = tra_annot.resolve("ait_perfetto_trace.perfetto-trace.annotation");
    Assert.assertTrue("Trace file should exist", Files.exists(traceFile));
    Assert.assertTrue("Annotation file should exist", Files.exists(traceAnnotation));
    Assert.assertEquals(
        "{\"type\": {\"generic_blob\": {}}, \"description\": \"Perfetto trace\"}",
        Files.readString(traceAnnotation));
  }

  @Test
  public void addsInstrumentationArgsFromPrefixedEnv() throws Throwable {
    Map<String, String> env = new HashMap<>();
    env.put("AIT_TEST_ONE", "foo");
    env.put("AIT_TEST_TWO", "bar");
    env.put("OTHER_RANDOM_ENV_VAR", "not");

    ArrayList<String> capturedShellCommands = new ArrayList<>();

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            capturedShellCommands, new HashMap<>(), env, new HashMap<>());

    runner.run();

    // Find the "am instrument" command that RemoteAndroidTestRunner executed
    String instrumentCommand = null;
    for (String command : capturedShellCommands) {
      if (command.startsWith("am instrument")) {
        instrumentCommand = command;
        break;
      }
    }

    Assert.assertNotNull("Expected to find 'am instrument' command", instrumentCommand);
    Assert.assertTrue(instrumentCommand.contains("-e TEST_ONE foo"));
    Assert.assertTrue(instrumentCommand.contains("-e TEST_TWO bar"));
    Assert.assertFalse(instrumentCommand.contains("RANDOM_ENV_VAR"));
  }

  @Test
  public void perTestCoveragePullsArtifactsToWrapperDir() throws Throwable {
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Path hostCoverageDir = tmp.newFolder("wrapper_per_test_coverage").getPath();
    String deviceCoverageDir =
        "/sdcard/Android/data/com.example/files/Download/test_result/per_test_coverage/";

    Map<String, String> env = new HashMap<>();
    env.put(InstrumentationTestRunner.PER_TEST_COVERAGE_ENABLED_ENV, "true");
    env.put(InstrumentationTestRunner.PER_TEST_COVERAGE_DIR_ENV, hostCoverageDir.toString());

    Map<Path, byte[]> filesOnDevice = new HashMap<>();
    filesOnDevice.put(
        Paths.get(deviceCoverageDir, "manifest.jsonl"), "{\"test\":\"example\"}\n".getBytes());

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDeviceForSdk(
            "30", capturedShellCommands, filesOnDevice, env, new HashMap<>());
    runner.run();

    String instrumentCommand = findInstrumentationCommand(capturedShellCommands);
    Assert.assertTrue(
        instrumentCommand.contains(
            "-e " + InstrumentationTestRunner.PER_TEST_COVERAGE_DIR_ARG + " " + deviceCoverageDir));
    Assert.assertTrue(Files.exists(hostCoverageDir.resolve("manifest.jsonl")));
  }

  @Test
  public void perTestCoverageFallsBackToTestArtifactsDir() throws Throwable {
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Path artifactsDir = tmp.newFolder("test_artifacts").getPath();
    String deviceCoverageDir =
        "/sdcard/Android/data/com.example/files/Download/test_result/per_test_coverage/";

    Map<String, String> env = new HashMap<>();
    env.put(InstrumentationTestRunner.PER_TEST_COVERAGE_ENABLED_ENV, "true");
    env.put("TEST_RESULT_ARTIFACTS_DIR", artifactsDir.toString());

    Map<Path, byte[]> filesOnDevice = new HashMap<>();
    filesOnDevice.put(
        Paths.get(deviceCoverageDir, "manifest.jsonl"), "{\"test\":\"example\"}\n".getBytes());

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDeviceForSdk(
            "30", capturedShellCommands, filesOnDevice, env, new HashMap<>());
    runner.run();

    String instrumentCommand = findInstrumentationCommand(capturedShellCommands);
    Assert.assertTrue(
        instrumentCommand.contains(
            "-e " + InstrumentationTestRunner.PER_TEST_COVERAGE_DIR_ARG + " " + deviceCoverageDir));
    Assert.assertTrue(Files.exists(artifactsDir.resolve("per_test_coverage/manifest.jsonl")));
  }

  @Test
  public void perTestCoverageUsesLegacyStorageBeforeApi30() throws Throwable {
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Path hostCoverageDir = tmp.newFolder("legacy_per_test_coverage").getPath();
    String deviceCoverageDir = "/sdcard/test_result/per_test_coverage/com.example.test/";

    Map<String, String> env = new HashMap<>();
    env.put(InstrumentationTestRunner.PER_TEST_COVERAGE_ENABLED_ENV, "true");
    env.put(InstrumentationTestRunner.PER_TEST_COVERAGE_DIR_ENV, hostCoverageDir.toString());

    Map<Path, byte[]> filesOnDevice = new HashMap<>();
    filesOnDevice.put(
        Paths.get(deviceCoverageDir, "manifest.jsonl"), "{\"test\":\"example\"}\n".getBytes());

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDeviceForSdk(
            "27", capturedShellCommands, filesOnDevice, env, new HashMap<>());
    runner.run();

    String instrumentCommand = findInstrumentationCommand(capturedShellCommands);
    Assert.assertTrue(
        instrumentCommand.contains(
            "-e " + InstrumentationTestRunner.PER_TEST_COVERAGE_DIR_ARG + " " + deviceCoverageDir));
    Assert.assertTrue(Files.exists(hostCoverageDir.resolve("manifest.jsonl")));
  }

  @Test
  public void pullDirPullsContentsDirectlyToDestination() throws Throwable {
    // Set up a device directory structure with files
    Map<Path, byte[]> filesOnDevice = new HashMap<>();
    filesOnDevice.put(Paths.get("/device/source_dir/file1.txt"), "content1".getBytes());
    filesOnDevice.put(Paths.get("/device/source_dir/subdir/file2.txt"), "content2".getBytes());

    // Create a temporary destination directory
    Path destinationDir = tmp.newFolder("destination").getPath();

    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device"
    };

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    DeviceRunner.DeviceArgs deviceArgs = DeviceRunner.getDeviceArgs(args);
    argsParser.fromArgs(args);

    InstrumentationTestRunner runner =
        new InstrumentationTestRunner(
            deviceArgs,
            argsParser.packageName,
            argsParser.targetPackageName,
            argsParser.testRunner,
            argsParser.outputDirectory,
            argsParser.instrumentationApkPath,
            argsParser.apkUnderTestPath,
            argsParser.exopackageLocalPath,
            argsParser.apkUnderTestExopackageLocalPath,
            argsParser.exopackageSecondaryDexLocalPath,
            argsParser.attemptUninstallApkUnderTest,
            argsParser.attemptUninstallInstrumentationApk,
            argsParser.debug,
            argsParser.codeCoverage,
            argsParser.codeCoverageOutputFile,
            argsParser.isSelfInstrumenting,
            argsParser.extraInstrumentationArguments,
            argsParser.extraInstrumentationTestListener,
            argsParser.extraFilesToPull,
            argsParser.extraDirsToPull,
            argsParser.clearPackageData,
            argsParser.disableAnimations,
            argsParser.preTestSetupScript,
            argsParser.extraApksToInstall,
            argsParser.userId) {

          @Override
          protected AndroidDevice initializeAndroidDevice() {
            // Return TestAndroidDevice for tests
            return new com.facebook.buck.android.TestAndroidDevice() {
              @Override
              public String getSerialNumber() {
                return "test-device-123";
              }

              @Override
              public String getProperty(String property) throws Exception {
                if ("ro.build.version.sdk".equals(property)) {
                  return "28"; // Android 9
                }
                return null;
              }
            };
          }

          @Override
          public boolean directoryExists(String dirPath) throws Exception {
            // return true for the source directory being tested
            return dirPath.equals("/device/source_dir") || dirPath.equals("/device/source_dir/.");
          }

          @Override
          protected void transferFile(String operation, String source, String destination)
              throws Exception {
            // Simulate actual adb pull behavior:
            // "adb pull /device/source_dir /local/dest" creates /local/dest/source_dir/ (nested)
            // "adb pull /device/source_dir/. /local/dest" puts contents in /local/dest/ (correct)
            Path sourcePath = Paths.get(source);
            Path destPath = Paths.get(destination);

            if (operation.equals("pull")) {
              // Check if source ends with "/."
              boolean pullContents = source.endsWith("/.");
              if (pullContents) {
                // Remove "/." from source path for lookup
                sourcePath = Paths.get(source.substring(0, source.length() - 2));
              }

              for (Map.Entry<Path, byte[]> entry : filesOnDevice.entrySet()) {
                if (entry.getKey().startsWith(sourcePath)) {
                  Path relativePath = sourcePath.relativize(entry.getKey());
                  Path target;

                  if (pullContents) {
                    // With "/." - put contents directly in destination
                    target = destPath.resolve(relativePath);
                  } else {
                    // Without "/." - create nested directory (broken behavior)
                    String dirName = sourcePath.getFileName().toString();
                    target = destPath.resolve(dirName).resolve(relativePath);
                  }

                  File parent = target.getParent().toFile();
                  if (!parent.exists()) {
                    parent.mkdirs();
                  }
                  Files.write(target, entry.getValue());
                }
              }
            }
          }
        };

    runner.pullDir("/device/source_dir", destinationDir.toString());

    Path file1 = destinationDir.resolve("file1.txt");
    Path file2 = destinationDir.resolve("subdir/file2.txt");
    Path wrongFile1 = destinationDir.resolve("source_dir/file1.txt");

    Assert.assertTrue("file1.txt should be directly in destination", Files.exists(file1));
    Assert.assertTrue(
        "subdir/file2.txt should preserve subdirectory structure", Files.exists(file2));
    Assert.assertFalse(
        "source_dir should not be created as a nested directory", Files.exists(wrongFile1));

    Assert.assertEquals("content1", Files.readString(file1));
    Assert.assertEquals("content2", Files.readString(file2));
  }

  @Test
  public void parsesUserIdArgument() throws Throwable {
    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device",
      "--user",
      "10"
    };

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    argsParser.fromArgs(args);

    Assert.assertEquals(Integer.valueOf(10), argsParser.userId);
  }

  @Test
  public void userIdIsNullWhenNotProvided() throws Throwable {
    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device"
    };

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    argsParser.fromArgs(args);

    Assert.assertNull(argsParser.userId);
  }

  @Test
  public void addsUserInstrumentationArgWhenUserIdProvided() throws Throwable {
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            capturedShellCommands,
            new HashMap<>(),
            new HashMap<>(),
            new HashMap<>(),
            "--user",
            "10");
    runner.run();

    // Find the "am instrument" command that RemoteAndroidTestRunner executed
    String instrumentCommand = null;
    for (String command : capturedShellCommands) {
      if (command.startsWith("am instrument")) {
        instrumentCommand = command;
        break;
      }
    }

    Assert.assertNotNull("Expected to find 'am instrument' command", instrumentCommand);
    Assert.assertTrue(
        "Expected instrument command to contain user arg",
        instrumentCommand.contains("-e user 10"));
  }

  @Test
  public void doesNotAddUserInstrumentationArgWhenUserIdNotProvided() throws Throwable {
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithDevice(
            capturedShellCommands, new HashMap<>(), new HashMap<>(), new HashMap<>());
    runner.run();

    // Find the "am instrument" command that RemoteAndroidTestRunner executed
    String instrumentCommand = null;
    for (String command : capturedShellCommands) {
      if (command.startsWith("am instrument")) {
        instrumentCommand = command;
        break;
      }
    }

    Assert.assertNotNull("Expected to find 'am instrument' command", instrumentCommand);
    Assert.assertFalse(
        "Expected instrument command to not contain user arg", instrumentCommand.contains("user"));
  }

  @Test
  public void installsForAllUsersWhenRunningAsSecondaryUser() throws Throwable {
    List<String> installedWithUserTarget = new ArrayList<>();

    final TestAndroidDevice testAndroidDevice =
        new TestAndroidDevice() {
          @Override
          public boolean installApkOnDevice(
              File apk,
              boolean installViaSd,
              boolean quiet,
              boolean verifyTempWritable,
              boolean stagedInstallMode,
              String userId,
              boolean allowFastDeploy,
              String packageName) {
            installedWithUserTarget.add(apk.getPath() + ":" + userId);
            return true;
          }

          @Override
          public String getSerialNumber() {
            return "test-device-123";
          }

          @Override
          public String getProperty(String property) throws Exception {
            if ("ro.build.version.sdk".equals(property)) {
              return "28";
            }
            return null;
          }
        };

    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device",
      "--instrumentation-apk-path",
      "/path/to/test.apk",
      "--user",
      "10"
    };

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    DeviceRunner.DeviceArgs deviceArgs = DeviceRunner.getDeviceArgs(args);
    argsParser.fromArgs(args);

    List<String> capturedShellCommands = new ArrayList<>();
    IDevice device =
        new TestDevice() {
          @Override
          public void executeShellCommand(String command, IShellOutputReceiver receiver) {
            capturedShellCommands.add(command);
          }

          @Override
          public FileListingService getFileListingService() {
            return new FileListingService(this);
          }
        };

    InstrumentationTestRunner runner =
        new InstrumentationTestRunner(
            deviceArgs,
            argsParser.packageName,
            argsParser.targetPackageName,
            argsParser.testRunner,
            argsParser.outputDirectory,
            argsParser.instrumentationApkPath,
            argsParser.apkUnderTestPath,
            argsParser.exopackageLocalPath,
            argsParser.apkUnderTestExopackageLocalPath,
            argsParser.exopackageSecondaryDexLocalPath,
            argsParser.attemptUninstallApkUnderTest,
            argsParser.attemptUninstallInstrumentationApk,
            argsParser.debug,
            argsParser.codeCoverage,
            argsParser.codeCoverageOutputFile,
            argsParser.isSelfInstrumenting,
            argsParser.extraInstrumentationArguments,
            argsParser.extraInstrumentationTestListener,
            argsParser.extraFilesToPull,
            argsParser.extraDirsToPull,
            argsParser.clearPackageData,
            argsParser.disableAnimations,
            argsParser.preTestSetupScript,
            argsParser.extraApksToInstall,
            argsParser.userId) {
          @Override
          protected IDevice resolveIDevice(String serial) {
            return device;
          }

          @Override
          protected AndroidDevice initializeAndroidDevice() {
            return testAndroidDevice;
          }

          @Override
          protected String executeAdbShellCommand(String command) throws Exception {
            capturedShellCommands.add(command);
            return "";
          }

          @Override
          protected String getenv(String envVar) {
            return null;
          }

          @Override
          protected void installPackage(String path, String packageName) throws Throwable {
            // When running as secondary user (userId > 0), install for all users
            String userTarget = (argsParser.userId != null && argsParser.userId > 0) ? "all" : null;
            testAndroidDevice.installApkOnDevice(
                new File(path), false, false, true, false, userTarget, packageName);
          }
        };

    runner.run();

    // Verify the APK was installed with "all" user target
    Assert.assertEquals(1, installedWithUserTarget.size());
    Assert.assertTrue(installedWithUserTarget.get(0).endsWith(":all"));
  }

  @Test
  public void installsForCurrentUserOnlyWhenNotSecondaryUser() throws Throwable {
    List<String> installedWithUserTarget = new ArrayList<>();

    final TestAndroidDevice testAndroidDevice =
        new TestAndroidDevice() {
          @Override
          public boolean installApkOnDevice(
              File apk,
              boolean installViaSd,
              boolean quiet,
              boolean verifyTempWritable,
              boolean stagedInstallMode,
              String userId,
              boolean allowFastDeploy,
              String packageName) {
            installedWithUserTarget.add(apk.getPath() + ":" + userId);
            return true;
          }

          @Override
          public String getSerialNumber() {
            return "test-device-123";
          }

          @Override
          public String getProperty(String property) throws Exception {
            if ("ro.build.version.sdk".equals(property)) {
              return "28";
            }
            return null;
          }
        };

    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device",
      "--instrumentation-apk-path",
      "/path/to/test.apk"
      // Note: No --user argument
    };

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    DeviceRunner.DeviceArgs deviceArgs = DeviceRunner.getDeviceArgs(args);
    argsParser.fromArgs(args);

    List<String> capturedShellCommands = new ArrayList<>();
    IDevice device =
        new TestDevice() {
          @Override
          public void executeShellCommand(String command, IShellOutputReceiver receiver) {
            capturedShellCommands.add(command);
          }

          @Override
          public FileListingService getFileListingService() {
            return new FileListingService(this);
          }
        };

    InstrumentationTestRunner runner =
        new InstrumentationTestRunner(
            deviceArgs,
            argsParser.packageName,
            argsParser.targetPackageName,
            argsParser.testRunner,
            argsParser.outputDirectory,
            argsParser.instrumentationApkPath,
            argsParser.apkUnderTestPath,
            argsParser.exopackageLocalPath,
            argsParser.apkUnderTestExopackageLocalPath,
            argsParser.exopackageSecondaryDexLocalPath,
            argsParser.attemptUninstallApkUnderTest,
            argsParser.attemptUninstallInstrumentationApk,
            argsParser.debug,
            argsParser.codeCoverage,
            argsParser.codeCoverageOutputFile,
            argsParser.isSelfInstrumenting,
            argsParser.extraInstrumentationArguments,
            argsParser.extraInstrumentationTestListener,
            argsParser.extraFilesToPull,
            argsParser.extraDirsToPull,
            argsParser.clearPackageData,
            argsParser.disableAnimations,
            argsParser.preTestSetupScript,
            argsParser.extraApksToInstall,
            argsParser.userId) {
          @Override
          protected IDevice resolveIDevice(String serial) {
            return device;
          }

          @Override
          protected AndroidDevice initializeAndroidDevice() {
            return testAndroidDevice;
          }

          @Override
          protected String executeAdbShellCommand(String command) throws Exception {
            capturedShellCommands.add(command);
            return "";
          }

          @Override
          protected String getenv(String envVar) {
            return null;
          }

          @Override
          protected void installPackage(String path, String packageName) throws Throwable {
            // When not running as secondary user, userId is null
            String userTarget = (argsParser.userId != null && argsParser.userId > 0) ? "all" : null;
            testAndroidDevice.installApkOnDevice(
                new File(path), false, false, true, false, userTarget, packageName);
          }
        };

    runner.run();

    // Verify the APK was installed with null user target (current user only)
    Assert.assertEquals(1, installedWithUserTarget.size());
    Assert.assertTrue(installedWithUserTarget.get(0).endsWith(":null"));
  }

  @Test
  public void resolvesPathForSecondaryUserSdcard() throws Throwable {
    // Test that /sdcard paths are resolved to /data/media/{userId} for secondary users
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Map<String, String> extraDirsToPull = new HashMap<>();
    extraDirsToPull.put("/sdcard/test_output", "/tmp/local_output");

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithExtraDirsToPull(
            capturedShellCommands, extraDirsToPull, "--user", "10");
    runner.run();

    // Verify that the resolved path uses /data/media/10 instead of /sdcard
    boolean foundResolvedRmCommand = false;
    boolean foundResolvedMkdirCommand = false;
    for (String command : capturedShellCommands) {
      if (command.contains("rm -fr /data/media/10/test_output")) {
        foundResolvedRmCommand = true;
      }
      if (command.contains("mkdir -p /data/media/10/test_output")) {
        foundResolvedMkdirCommand = true;
      }
    }

    Assert.assertTrue(
        "Expected rm command to use resolved path /data/media/10/test_output",
        foundResolvedRmCommand);
    Assert.assertTrue(
        "Expected mkdir command to use resolved path /data/media/10/test_output",
        foundResolvedMkdirCommand);
  }

  @Test
  public void resolvesPathForSecondaryUserStorageEmulated0() throws Throwable {
    // Test that /storage/emulated/0 paths are resolved to /data/media/{userId}
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Map<String, String> extraDirsToPull = new HashMap<>();
    extraDirsToPull.put("/storage/emulated/0/test_output", "/tmp/local_output");

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithExtraDirsToPull(
            capturedShellCommands, extraDirsToPull, "--user", "10");
    runner.run();

    // Verify that the resolved path uses /data/media/10 instead of /storage/emulated/0
    boolean foundResolvedCommand = false;
    for (String command : capturedShellCommands) {
      if (command.contains("rm -fr /data/media/10/test_output")) {
        foundResolvedCommand = true;
        break;
      }
    }

    Assert.assertTrue(
        "Expected command to use resolved path /data/media/10/test_output", foundResolvedCommand);
  }

  @Test
  public void resolvesPathForSecondaryUserStorageEmulatedOtherUserId() throws Throwable {
    // Test that /storage/emulated/{other_user_id} paths are translated to current user's path
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Map<String, String> extraDirsToPull = new HashMap<>();
    // Path references another user (15), should be translated to current user (10)
    extraDirsToPull.put("/storage/emulated/15/test_output", "/tmp/local_output");

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithExtraDirsToPull(
            capturedShellCommands, extraDirsToPull, "--user", "10");
    runner.run();

    // Verify that the resolved path uses /data/media/10 (current user)
    boolean foundResolvedCommand = false;
    for (String command : capturedShellCommands) {
      if (command.contains("rm -fr /data/media/10/test_output")) {
        foundResolvedCommand = true;
        break;
      }
    }

    Assert.assertTrue(
        "Expected command to use resolved path /data/media/10/test_output", foundResolvedCommand);
  }

  @Test
  public void doesNotResolvePathForPrimaryUser() throws Throwable {
    // Test that paths are NOT modified for primary user (userId = 0 or null)
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Map<String, String> extraDirsToPull = new HashMap<>();
    extraDirsToPull.put("/sdcard/test_output", "/tmp/local_output");

    // No --user argument, so userId is null (primary user behavior)
    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithExtraDirsToPull(capturedShellCommands, extraDirsToPull);
    runner.run();

    // Verify that the original path /sdcard is used, not /data/media
    boolean foundOriginalPath = false;
    boolean foundResolvedPath = false;
    for (String command : capturedShellCommands) {
      if (command.contains("rm -fr /sdcard/test_output")) {
        foundOriginalPath = true;
      }
      if (command.contains("/data/media")) {
        foundResolvedPath = true;
      }
    }

    Assert.assertTrue(
        "Expected command to use original path /sdcard/test_output", foundOriginalPath);
    Assert.assertFalse("Expected command to NOT use /data/media path", foundResolvedPath);
  }

  @Test
  public void preservesNonUserStoragePathsUnchanged() throws Throwable {
    // Test that non-sdcard/non-storage paths are not modified
    ArrayList<String> capturedShellCommands = new ArrayList<>();
    Map<String, String> extraDirsToPull = new HashMap<>();
    extraDirsToPull.put("/data/local/tmp/test_output", "/tmp/local_output");

    InstrumentationTestRunner runner =
        createInstrumentationTestRunnerWithExtraDirsToPull(
            capturedShellCommands, extraDirsToPull, "--user", "10");
    runner.run();

    // Verify that /data/local/tmp path is preserved unchanged
    boolean foundOriginalPath = false;
    for (String command : capturedShellCommands) {
      if (command.contains("rm -fr /data/local/tmp/test_output")) {
        foundOriginalPath = true;
        break;
      }
    }

    Assert.assertTrue(
        "Expected command to preserve original path /data/local/tmp/test_output",
        foundOriginalPath);
  }

  private InstrumentationTestRunner createInstrumentationTestRunnerWithExtraDirsToPull(
      List<String> capturedShellCommands, Map<String, String> extraDirsToPull, String... extraArgs)
      throws Throwable {

    final AndroidDevice testAndroidDevice =
        new TestAndroidDevice() {
          @Override
          public String getSerialNumber() {
            return "test-device-123";
          }

          @Override
          public String getProperty(String property) throws Exception {
            if ("ro.build.version.sdk".equals(property)) {
              return "28"; // Android 9
            }
            return null;
          }
        };

    List<String> argsList = new ArrayList<>();
    argsList.addAll(
        Arrays.asList(
            "--target-package-name",
            "com.example",
            "--test-package-name",
            "com.example.test",
            "--test-runner",
            "com.example.test.TestRunner",
            "--adb-executable-path",
            "required_but_not_used",
            "--output",
            "/dev/null",
            "--auto-run-on-connected-device"));

    // Add extra dirs to pull
    for (Map.Entry<String, String> entry : extraDirsToPull.entrySet()) {
      argsList.add("--extra-dir-to-pull");
      argsList.add(entry.getKey() + "=" + entry.getValue());
    }

    // Add any extra args (like --user)
    argsList.addAll(Arrays.asList(extraArgs));

    String[] allArgs = argsList.toArray(new String[0]);

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    DeviceRunner.DeviceArgs deviceArgs = DeviceRunner.getDeviceArgs(allArgs);
    argsParser.fromArgs(allArgs);

    IDevice device =
        new TestDevice() {
          @Override
          public void executeShellCommand(String command, IShellOutputReceiver receiver) {
            capturedShellCommands.add(command);
          }

          @Override
          public FileListingService getFileListingService() {
            return new FileListingService(this);
          }
        };

    return new InstrumentationTestRunner(
        deviceArgs,
        argsParser.packageName,
        argsParser.targetPackageName,
        argsParser.testRunner,
        argsParser.outputDirectory,
        argsParser.instrumentationApkPath,
        argsParser.apkUnderTestPath,
        argsParser.exopackageLocalPath,
        argsParser.apkUnderTestExopackageLocalPath,
        argsParser.exopackageSecondaryDexLocalPath,
        argsParser.attemptUninstallApkUnderTest,
        argsParser.attemptUninstallInstrumentationApk,
        argsParser.debug,
        argsParser.codeCoverage,
        argsParser.codeCoverageOutputFile,
        argsParser.isSelfInstrumenting,
        argsParser.extraInstrumentationArguments,
        argsParser.extraInstrumentationTestListener,
        argsParser.extraFilesToPull,
        argsParser.extraDirsToPull,
        argsParser.clearPackageData,
        argsParser.disableAnimations,
        argsParser.preTestSetupScript,
        argsParser.extraApksToInstall,
        argsParser.userId) {
      @Override
      protected IDevice resolveIDevice(String serial) {
        return device;
      }

      @Override
      protected AndroidDevice initializeAndroidDevice() {
        return testAndroidDevice;
      }

      @Override
      protected String executeAdbShellCommand(String command) throws Exception {
        capturedShellCommands.add(command);
        return "";
      }

      @Override
      public boolean directoryExists(String dirPath) throws Exception {
        // Return false to simulate that directories don't exist yet
        return false;
      }

      @Override
      public void pullDir(String sourceDir, String destinationDir) throws Exception {
        // Mock implementation - do nothing
      }
    };
  }

  @Test
  public void installsSingleApkWhenNoApkUnderTest() throws Throwable {
    List<String> installedPackages = new ArrayList<>();
    List<String> capturedShellCommands = new ArrayList<>();
    Map<String, String> shellCommandMockResponses = new HashMap<>();

    final TestAndroidDevice testAndroidDevice =
        new TestAndroidDevice() {
          @Override
          public boolean installApkOnDevice(
              File apk,
              boolean installViaSd,
              boolean quiet,
              boolean verifyTempWritable,
              boolean stagedInstallMode,
              String userId,
              boolean allowFastDeploy,
              String packageName) {
            installedPackages.add(apk.getPath());
            return true;
          }

          @Override
          public String getSerialNumber() {
            return "test-device-123";
          }

          @Override
          public String getProperty(String property) throws Exception {
            if ("ro.build.version.sdk".equals(property)) {
              return "28"; // Android 9
            }
            return null;
          }
        };

    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device",
      "--instrumentation-apk-path",
      "/path/to/test.apk"
      // Note: No --apk-under-test-path
    };

    InstrumentationTestRunner.ArgsParser argsParser =
        new InstrumentationTestRunner.ArgsParser(new HashMap<>());
    DeviceRunner.DeviceArgs deviceArgs = DeviceRunner.getDeviceArgs(args);
    argsParser.fromArgs(args);

    // Create minimal IDevice only for RemoteAndroidTestRunner compatibility
    // RemoteAndroidTestRunner (ddmlib class) will execute "am instrument" via IDevice
    IDevice device =
        new TestDevice() {
          @Override
          public void executeShellCommand(String command, IShellOutputReceiver receiver) {
            // Capture commands executed by RemoteAndroidTestRunner
            capturedShellCommands.add(command);

            // Return mock responses if configured
            if (shellCommandMockResponses.containsKey(command)) {
              byte[] response = shellCommandMockResponses.get(command).getBytes();
              receiver.addOutput(response, 0, response.length);
            }
          }

          @Override
          public FileListingService getFileListingService() {
            return new FileListingService(this);
          }
        };

    InstrumentationTestRunner runner =
        new InstrumentationTestRunner(
            deviceArgs,
            argsParser.packageName,
            argsParser.targetPackageName,
            argsParser.testRunner,
            argsParser.outputDirectory,
            argsParser.instrumentationApkPath,
            argsParser.apkUnderTestPath,
            argsParser.exopackageLocalPath,
            argsParser.apkUnderTestExopackageLocalPath,
            argsParser.exopackageSecondaryDexLocalPath,
            argsParser.attemptUninstallApkUnderTest,
            argsParser.attemptUninstallInstrumentationApk,
            argsParser.debug,
            argsParser.codeCoverage,
            argsParser.codeCoverageOutputFile,
            argsParser.isSelfInstrumenting,
            argsParser.extraInstrumentationArguments,
            argsParser.extraInstrumentationTestListener,
            argsParser.extraFilesToPull,
            argsParser.extraDirsToPull,
            argsParser.clearPackageData,
            argsParser.disableAnimations,
            argsParser.preTestSetupScript,
            argsParser.extraApksToInstall,
            argsParser.userId) {
          @Override
          protected IDevice resolveIDevice(String serial) {
            // Return IDevice only for RemoteAndroidTestRunner compatibility
            return device;
          }

          @Override
          protected AndroidDevice initializeAndroidDevice() {
            // Initialize with the test's tracking AndroidDevice
            return testAndroidDevice;
          }

          @Override
          protected String executeAdbShellCommand(String command) throws Exception {
            // Pure AndroidDevice implementation - capture for verification
            capturedShellCommands.add(command);

            // Return configured mock response if available
            if (shellCommandMockResponses.containsKey(command)) {
              return shellCommandMockResponses.get(command);
            }

            // Default: return empty string
            return "";
          }

          @Override
          protected String getenv(String envVar) {
            return null;
          }

          @Override
          protected void installPackage(String path, String packageName) throws Throwable {
            testAndroidDevice.installApkOnDevice(
                new File(path), false, false, true, false, null, packageName);
          }
        };

    runner.run();

    // Verify only one APK was installed
    Assert.assertEquals(1, installedPackages.size());
    Assert.assertTrue(installedPackages.contains("/path/to/test.apk"));
  }

  private InstrumentationTestRunner createInstrumentationTestRunnerWithDevice(
      Map<Path, byte[]> filesOnDevice, Map<String, String> env, String... extraArgs)
      throws Throwable {
    List<String> capturedCommands = new ArrayList<>();
    return createInstrumentationTestRunnerWithDevice(
        capturedCommands, filesOnDevice, env, new HashMap<>(), extraArgs);
  }

  private InstrumentationTestRunner createInstrumentationTestRunnerWithDevice(
      Map<Path, byte[]> filesOnDevice,
      Map<String, String> env,
      Map<String, String> shellCommandMockResponses,
      String... extraArgs)
      throws Throwable {
    List<String> capturedCommands = new ArrayList<>();
    return createInstrumentationTestRunnerWithDevice(
        capturedCommands, filesOnDevice, env, shellCommandMockResponses, extraArgs);
  }

  private static String findInstrumentationCommand(List<String> capturedShellCommands) {
    for (String command : capturedShellCommands) {
      if (command.startsWith("am instrument")) {
        return command;
      }
    }

    Assert.fail("Expected to find 'am instrument' command");
    return "";
  }

  private InstrumentationTestRunner createInstrumentationTestRunnerWithDevice(
      List<String> capturedShellCommands,
      Map<Path, byte[]> filesOnDevice,
      Map<String, String> env,
      Map<String, String> shellCommandMockResponses,
      String... extraArgs)
      throws Throwable {
    return createInstrumentationTestRunnerWithDeviceForSdk(
        "28", capturedShellCommands, filesOnDevice, env, shellCommandMockResponses, extraArgs);
  }

  private InstrumentationTestRunner createInstrumentationTestRunnerWithDeviceForSdk(
      String sdkVersion,
      List<String> capturedShellCommands,
      Map<Path, byte[]> filesOnDevice,
      Map<String, String> env,
      Map<String, String> shellCommandMockResponses,
      String... extraArgs)
      throws Throwable {
    // Create a TestAndroidDevice as the primary device abstraction
    final AndroidDevice testAndroidDevice =
        new TestAndroidDevice() {
          @Override
          public String getSerialNumber() {
            return "test-device-123";
          }

          @Override
          public String getProperty(String property) throws Exception {
            if ("ro.build.version.sdk".equals(property)) {
              return sdkVersion;
            }
            return null;
          }
        };

    String[] args = {
      "--target-package-name",
      "com.example",
      "--test-package-name",
      "com.example.test",
      "--test-runner",
      "com.example.test.TestRunner",
      "--adb-executable-path",
      "required_but_not_used",
      "--output",
      "/dev/null",
      "--auto-run-on-connected-device"
    };

    String[] allArgs = new String[args.length + extraArgs.length];
    System.arraycopy(args, 0, allArgs, 0, args.length);
    System.arraycopy(extraArgs, 0, allArgs, args.length, extraArgs.length);

    InstrumentationTestRunner.ArgsParser argsParser = new InstrumentationTestRunner.ArgsParser(env);
    DeviceRunner.DeviceArgs deviceArgs = DeviceRunner.getDeviceArgs(args);
    argsParser.fromArgs(allArgs);

    // Create minimal IDevice only for RemoteAndroidTestRunner compatibility
    // RemoteAndroidTestRunner (ddmlib class) will execute "am instrument" via IDevice
    // We need to capture those commands too
    IDevice device =
        new TestDevice() {
          @Override
          public void executeShellCommand(String command, IShellOutputReceiver receiver) {
            // Capture commands executed by RemoteAndroidTestRunner
            capturedShellCommands.add(command);

            // Return mock responses if configured
            if (shellCommandMockResponses.containsKey(command)) {
              byte[] response = shellCommandMockResponses.get(command).getBytes();
              receiver.addOutput(response, 0, response.length);
            }
          }

          @Override
          public FileListingService getFileListingService() {
            return new FileListingService(this);
          }
        };

    InstrumentationTestRunner runner =
        new InstrumentationTestRunner(
            deviceArgs,
            argsParser.packageName,
            argsParser.targetPackageName,
            argsParser.testRunner,
            argsParser.outputDirectory,
            argsParser.instrumentationApkPath,
            argsParser.apkUnderTestPath,
            argsParser.exopackageLocalPath,
            argsParser.apkUnderTestExopackageLocalPath,
            argsParser.exopackageSecondaryDexLocalPath,
            argsParser.attemptUninstallApkUnderTest,
            argsParser.attemptUninstallInstrumentationApk,
            argsParser.debug,
            argsParser.codeCoverage,
            argsParser.codeCoverageOutputFile,
            argsParser.isSelfInstrumenting,
            argsParser.extraInstrumentationArguments,
            argsParser.extraInstrumentationTestListener,
            argsParser.extraFilesToPull,
            argsParser.extraDirsToPull,
            argsParser.clearPackageData,
            argsParser.disableAnimations,
            argsParser.preTestSetupScript,
            argsParser.extraApksToInstall,
            argsParser.userId) {

          @Override
          public Process exec(String command) throws IOException {
            if (execProcessOverride != null) {
              return execProcessOverride;
            }
            return new Process() {
              @Override
              public OutputStream getOutputStream() {
                return System.out;
              }

              @Override
              public InputStream getInputStream() {
                return new ByteArrayInputStream(new byte[0]);
              }

              @Override
              public InputStream getErrorStream() {
                return new ByteArrayInputStream(new byte[0]);
              }

              @Override
              public int waitFor() throws InterruptedException {
                return 0;
              }

              @Override
              public int exitValue() {
                return 0;
              }

              @Override
              public void destroy() {}
            };
          }

          @Override
          protected IDevice resolveIDevice(String serial) {
            // Return IDevice only for RemoteAndroidTestRunner compatibility
            return device;
          }

          @Override
          protected AndroidDevice initializeAndroidDevice() {
            // Use our TestAndroidDevice as the primary device abstraction
            return testAndroidDevice;
          }

          @Override
          protected String executeAdbShellCommand(String command) throws Exception {
            // Pure AndroidDevice implementation - NO IDevice usage!
            // Capture command for test verification
            capturedShellCommands.add(command);

            // Return configured mock response if available
            if (shellCommandMockResponses.containsKey(command)) {
              return shellCommandMockResponses.get(command);
            }

            // Default: return empty string
            return "";
          }

          @Override
          public boolean directoryExists(String dirPath) throws Exception {
            // Call our overridden executeAdbShellCommand to use mocked responses
            String output =
                executeAdbShellCommand(String.format("test -d %s && echo exists", dirPath));
            return output.contains("exists");
          }

          @Override
          public void pullDir(String sourceDir, String destinationDir) throws Exception {
            // Mock implementation that reads from the filesOnDevice map
            Path sourcePath = Paths.get(sourceDir);
            File destinationDirFile = new File(destinationDir);
            if (!destinationDirFile.exists()) {
              destinationDirFile.mkdirs();
            }
            for (Map.Entry<Path, byte[]> entry : filesOnDevice.entrySet()) {
              if (entry.getKey().startsWith(sourcePath)) {
                Path relativePath = sourcePath.relativize(entry.getKey());
                Path target = Paths.get(destinationDir).resolve(relativePath);
                File parent = target.getParent().toFile();
                if (!parent.exists()) {
                  parent.mkdirs();
                }
                Files.write(target, entry.getValue());
              }
            }
          }

          @Override
          public void pullFileWithSyncService(String remote, String local) throws Exception {
            for (Map.Entry<Path, byte[]> entry : filesOnDevice.entrySet()) {
              Path remote_path = Paths.get(remote);
              System.out.printf("remote: %s -> local: %s", remote, local);
              System.out.printf("entry: %s", entry.getKey().toString());
              if (entry.getKey().equals(remote_path)) {
                Path target = Paths.get(local);
                Files.write(target, entry.getValue());
              }
            }
          }

          @Override
          public void pushFileWithSyncService(String local, String remote) throws Exception {
            System.out.printf("mock: push file, local: %s -> remote: %s", local, remote);
          }

          @Override
          protected String getenv(String envVar) {
            return env.get(envVar);
          }

          @Override
          protected Map<String, String> getenv() {
            return env;
          }
        };
    if (argsParser.recordVideo) {
      runner.addReportLayer(new VideoRecordingReportLayer(runner));
    }
    runner.addReportLayer(new TombstonesReportLayer(runner, argsParser.collectTombstones));
    if (argsParser.collectPerfetto) {
      runner.addReportLayer(new PerfettoReportLayer(runner));
    }
    if (!argsParser.logExtractors.isEmpty()) {
      runner.addReportLayer(new LogExtractorReportLayer(runner, argsParser.logExtractors));
    }
    return runner;
  }
}
