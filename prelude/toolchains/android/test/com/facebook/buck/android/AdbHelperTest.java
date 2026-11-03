/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.anyBoolean;
import static org.mockito.Mockito.eq;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.facebook.buck.android.device.TargetDeviceOptions;
import com.facebook.buck.android.exopackage.AdbCommandFailedException;
import com.facebook.buck.android.exopackage.AdbUtils;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.buck.android.exopackage.AndroidDeviceImpl;
import com.facebook.buck.android.exopackage.SetDebugAppMode;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.installer.android.IsolatedAndroidInstallerPrinter;
import com.facebook.buck.testutil.MoreAsserts;
import com.facebook.buck.testutil.TemporaryPaths;
import com.facebook.buck.testutil.TestLogSink;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.File;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Optional;
import java.util.logging.LogRecord;
import java.util.logging.Logger;
import java.util.stream.Collectors;
import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.ExpectedException;
import org.junit.runner.RunWith;
import org.mockito.Mock;
import org.mockito.Mockito;
import org.mockito.junit.MockitoJUnitRunner;

@RunWith(MockitoJUnitRunner.Silent.class)
public class AdbHelperTest {

  private static final Logger LOGGER = Logger.getLogger(AdbHelperTest.class.getName());

  @Rule public ExpectedException exceptionRule = ExpectedException.none();
  @Rule public TestLogSink testLogSink = new TestLogSink(AdbHelperTest.class);
  @Rule public final TemporaryPaths tmpDir = new TemporaryPaths();

  private TestAdbExecutionContext adbExecutionContext;
  private AdbHelper basicAdbHelper;

  @Mock private AdbUtils adbUtils;

  @Before
  public void setUp() {
    adbExecutionContext = new TestAdbExecutionContext();
    basicAdbHelper = createAdbHelper(createAdbOptions(), new TargetDeviceOptions());
  }

  @After
  public void tearDown() {
    AdbHelper.setDevicesSupplierForTests(Optional.empty());
  }

  private static AndroidDeviceImpl createRealDevice(String serial, String state) {

    AndroidDeviceImpl device = Mockito.mock(AndroidDeviceImpl.class);
    when(device.getSerialNumber()).thenReturn(serial);
    when(device.isEmulator()).thenReturn(false);
    when(device.isOnline()).thenReturn("device".equals(state));
    return device;
  }

  private static AndroidDeviceImpl createEmulator(String serial, String state) {
    AndroidDeviceImpl device = Mockito.mock(AndroidDeviceImpl.class);
    when(device.getSerialNumber()).thenReturn(serial);
    when(device.isEmulator()).thenReturn(true);
    when(device.isOnline()).thenReturn("device".equals(state));
    return device;
  }

  private AdbHelper createAdbHelper(
      AdbOptions adbOptions, TargetDeviceOptions targetDeviceOptions) {
    return createAdbHelper(adbExecutionContext, adbOptions, targetDeviceOptions);
  }

  private AdbHelper createAdbHelper(
      AdbExecutionContext adbExecutionContext,
      AdbOptions adbOptions,
      TargetDeviceOptions targetDeviceOptions) {
    return new AdbHelper(
        adbUtils,
        adbOptions,
        targetDeviceOptions,
        adbExecutionContext,
        new IsolatedAndroidInstallerPrinter(LOGGER),
        /* restartAdbOnFailure */ true,
        SetDebugAppMode.SET);
  }

  /** Verify that null is returned when no devices are present. */
  @Test
  public void testDeviceFilterNoDevices() {
    List<AndroidDevice> devices = new ArrayList<>();
    assertTrue(basicAdbHelper.filterDevices(devices).isEmpty());
  }

  /** Verify that non-online devices will not appear in result list. */
  @Test
  public void testDeviceFilterOnlineOnly() {
    List<AndroidDevice> devices =
        Arrays.asList(
            createEmulator("1", "offline"),
            createEmulator("2", "bootloader"),
            createEmulator("3", "recovery"),
            createRealDevice("4", "offline"),
            createRealDevice("5", "bootloader"),
            createRealDevice("6", "recovery"));

    assertTrue(basicAdbHelper.filterDevices(devices).isEmpty());
  }

  /**
   * Verify that multi-install is not enabled and multiple devices pass the filter null is returned.
   * Also verify that if multiple devices are passing the filter and multi-install mode is enabled
   * they all appear in resulting list.
   */
  @Test
  public void testDeviceFilterMultipleDevices() {
    List<AndroidDevice> devices =
        Arrays.asList(
            createEmulator("1", "device"),
            createEmulator("2", "device"),
            createRealDevice("4", "device"),
            createRealDevice("5", "device"));

    List<AndroidDevice> filteredDevicesNoMultiInstall = basicAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevicesNoMultiInstall);
    assertEquals(devices.size(), filteredDevicesNoMultiInstall.size());

    AdbHelper myAdbHelper = createAdbHelper(createAdbOptions(true), new TargetDeviceOptions());
    List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevices);
    assertEquals(devices.size(), filteredDevices.size());
  }

  /** Verify that when emulator-only mode is enabled only emulators appear in result. */
  @Test
  public void testDeviceFilterEmulator() {
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("1"), anyBoolean()))
        .thenReturn("1");
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("2"), anyBoolean()))
        .thenReturn("0");
    AdbHelper myAdbHelper =
        createAdbHelper(createAdbOptions(), new TargetDeviceOptions(true, false, Optional.empty()));

    List<AndroidDevice> devices =
        Arrays.asList(createEmulator("1", "device"), createRealDevice("2", "device"));

    List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevices);
    assertEquals(1, filteredDevices.size());
    assertSame(devices.get(0), filteredDevices.get(0));
  }

  /** Verify that when real-device-only mode is enabled only real devices appear in result. */
  @Test
  public void testDeviceFilterRealDevices() {
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("1"), anyBoolean()))
        .thenReturn("0");
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("2"), anyBoolean()))
        .thenReturn("1");
    AdbHelper myAdbHelper =
        createAdbHelper(createAdbOptions(), new TargetDeviceOptions(false, true, Optional.empty()));

    List<AndroidDevice> devices =
        Arrays.asList(createRealDevice("1", "device"), createEmulator("2", "device"));

    List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevices);
    assertEquals(1, filteredDevices.size());
    assertSame(devices.get(0), filteredDevices.get(0));
  }

  /** Verify that filtering by serial number works. */
  @Test
  public void testDeviceFilterBySerial() {
    List<AndroidDevice> devices =
        Arrays.asList(
            createRealDevice("1", "device"),
            createEmulator("2", "device"),
            createRealDevice("3", "device"),
            createEmulator("4", "device"));

    for (AndroidDevice device : devices) {
      AdbHelper myAdbHelper =
          createAdbHelper(
              createAdbOptions(),
              new TargetDeviceOptions(false, false, Optional.of(device.getSerialNumber())));
      List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
      assertNotNull(filteredDevices);
      assertEquals(1, filteredDevices.size());
      assertSame(device, filteredDevices.get(0));
    }
  }

  /** Verify that filtering by environment variable works. */
  @Test
  public void whenSerialNumberSetInEnvironmentThenCorrectDeviceFound() {
    List<AndroidDevice> devices =
        Arrays.asList(
            createRealDevice("1", "device"),
            createEmulator("2", "device"),
            createRealDevice("3", "device"),
            createEmulator("4", "device"));

    for (AndroidDevice device : devices) {
      AdbHelper myAdbHelper =
          createAdbHelper(
              new TestAdbExecutionContext(
                  ImmutableMap.of(AdbHelper.SERIAL_NUMBER_ENV, device.getSerialNumber())),
              createAdbOptions(),
              new TargetDeviceOptions());
      List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
      assertNotNull(filteredDevices);
      assertEquals(1, filteredDevices.size());
      assertSame(device, filteredDevices.get(0));
    }
  }

  /** Verify that if no devices match filters empty list is returned. */
  @Test
  public void testDeviceFilterNoMatchingDevices() {
    when(adbUtils.executeAdbShellCommand(
            eq("getprop ro.kernel.qemu"), Mockito.anyString(), anyBoolean()))
        .thenReturn("1");
    List<AndroidDevice> devices =
        Arrays.asList(
            createRealDevice("1", "device"),
            createEmulator("2", "device"),
            createRealDevice("3", "device"),
            createEmulator("4", "device"));

    AdbHelper myAdbHelper =
        createAdbHelper(
            createAdbOptions(),
            new TargetDeviceOptions(false, false, Optional.of("invalid-serial")));
    List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
    assertTrue(filteredDevices.isEmpty());
  }

  /** Verify that different combinations of arguments work correctly. */
  @Test
  public void testDeviceFilterCombos() {
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("1"), anyBoolean()))
        .thenReturn("0");
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("2"), anyBoolean()))
        .thenReturn("0");
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("3"), anyBoolean()))
        .thenReturn("1");
    when(adbUtils.executeAdbShellCommand(eq("getprop ro.kernel.qemu"), eq("4"), anyBoolean()))
        .thenReturn("1");

    AndroidDeviceImpl realDevice1 = createRealDevice("1", "device");
    AndroidDeviceImpl realDevice2 = createRealDevice("2", "device");
    AndroidDeviceImpl emulator1 = createEmulator("3", "device");
    AndroidDeviceImpl emulator2 = createEmulator("4", "device");
    List<AndroidDevice> devices = Arrays.asList(realDevice1, emulator1, realDevice2, emulator2);

    AdbHelper myAdbHelper;
    // Filter by serial in "real device" mode with serial number for real device.
    myAdbHelper =
        createAdbHelper(
            createAdbOptions(),
            new TargetDeviceOptions(false, true, Optional.of(realDevice1.getSerialNumber())));
    List<AndroidDevice> filteredDevices = myAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevices);
    assertEquals(1, filteredDevices.size());
    assertSame(realDevice1, filteredDevices.get(0));

    // Filter by serial in "real device" mode with serial number for emulator.
    myAdbHelper =
        createAdbHelper(
            createAdbOptions(),
            new TargetDeviceOptions(false, true, Optional.of(emulator1.getSerialNumber())));
    filteredDevices = myAdbHelper.filterDevices(devices);
    assertTrue(filteredDevices.isEmpty());

    // Filter by serial in "emulator" mode with serial number for real device.
    myAdbHelper =
        createAdbHelper(
            createAdbOptions(),
            new TargetDeviceOptions(true, false, Optional.of(realDevice1.getSerialNumber())));
    filteredDevices = myAdbHelper.filterDevices(devices);
    assertTrue(filteredDevices.isEmpty());

    // Filter by serial in "real device" mode with serial number for emulator.
    myAdbHelper =
        createAdbHelper(
            createAdbOptions(),
            new TargetDeviceOptions(true, false, Optional.of(emulator1.getSerialNumber())));
    filteredDevices = myAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevices);
    assertEquals(1, filteredDevices.size());
    assertSame(emulator1, filteredDevices.get(0));

    // Filter in both "real device" mode and "emulator mode".
    myAdbHelper =
        createAdbHelper(
            createAdbOptions(true), new TargetDeviceOptions(true, true, Optional.empty()));
    filteredDevices = myAdbHelper.filterDevices(devices);
    assertNotNull(filteredDevices);
    assertEquals(devices.size(), filteredDevices.size());
    for (AndroidDevice device : devices) {
      assertTrue(filteredDevices.contains(device));
    }
  }

  private void assertLoggedToConsole(String... messages) {
    List<String> loggedMessages =
        testLogSink.getRecords().stream().map(LogRecord::getMessage).collect(Collectors.toList());
    MoreAsserts.assertListEquals(ImmutableList.copyOf(messages), loggedMessages);
  }

  @Test
  public void testGetDevicesShouldLogWhenMultipleDevices() {
    AdbHelper adbHelper =
        createAdbHelper(
            ImmutableList.of(
                createRealDevice("first", "device"), createRealDevice("second", "device")));

    assertEquals(2, adbHelper.getDevices(false).size());
    assertLoggedToConsole("Found 2 matching devices.\n");
  }

  @Test
  public void testGetDevicesShouldRespectQuietFlag() {
    AdbHelper adbHelper =
        createAdbHelper(
            ImmutableList.of(
                createRealDevice("first", "device"), createRealDevice("second", "device")));

    assertEquals(2, adbHelper.getDevices(true).size());
    assertTrue(testLogSink.getRecords().isEmpty());
  }

  @Test
  public void testAdbCallShouldFailWhenNoDevices() throws Exception {
    AdbHelper adbHelper = createAdbHelper(ImmutableList.of());

    exceptionRule.expect(RuntimeException.class);
    exceptionRule.expectMessage("Didn't find any attached Android devices/emulators.");
    adbHelper.adbCall("dummy", d -> true, true);
  }

  @Test
  public void testGetDevicesShouldRetryIfNoDevicesFound() {
    when(adbUtils.getDevices())
        .thenReturn(Collections.emptyList())
        .thenReturn(Collections.singletonList(createEmulator("some_devices", "device")));
    AdbHelper helper = createAdbHelper();

    assertEquals(1, helper.getDevices(true).size());
  }

  @Test
  public void testAdbCallShouldFailForMultipleDevices() throws Exception {
    exceptionRule.expect(RuntimeException.class);
    exceptionRule.expectMessage("2 devices match specified device filter");

    List<AndroidDevice> devices =
        Arrays.asList(createRealDevice("one", "device"), createRealDevice("two", "device"));
    when(adbUtils.getDevices()).thenReturn(devices);
    AdbHelper helper = createAdbHelper();
    helper.adbCall("test", d -> true, /* quiet= */ true);
  }

  @Test
  public void testAdbCallShouldFailForNoDevices() throws Exception {
    exceptionRule.expect(RuntimeException.class);
    exceptionRule.expectMessage("Didn't find any attached Android devices/emulators.");

    when(adbUtils.getDevices()).thenReturn(Collections.emptyList());
    AdbHelper helper = createAdbHelper();
    helper.adbCall("test", d -> true, /* quiet= */ true);
  }

  @Test
  public void testAdbCallShouldSucceedForNoDevicesWithIgnoreMissingDeviceFlag() throws Exception {
    AdbOptions options = createAdbOptions(false, true);
    when(adbUtils.getDevices()).thenReturn(Collections.emptyList());
    AdbHelper helper = createAdbHelper(options);
    helper.adbCall("test", d -> true, true);
  }

  @Test
  public void testAdbCallShouldSucceedForDevicesWithIgnoreMissingDeviceFlag() throws Exception {
    AdbOptions options = createAdbOptions(false, true);
    List<AndroidDevice> devices = Collections.singletonList(createRealDevice("one", "device"));
    when(adbUtils.getDevices()).thenReturn(devices);
    AdbHelper helper = createAdbHelper(options);
    helper.adbCall("test", d -> true, true);
  }

  @Test
  public void testPlainInstallSkipsAnApkTheDeviceAlreadyHas() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);

    installPlainApk(/* restartApp= */ false);

    verify(device, never())
        .installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            anyBoolean());
    verify(device, never()).stopPackage(anyString());
  }

  @Test
  public void testPlainInstallRestartsAnUnchangedAppWhenAnActivityWillBeStarted() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);

    installPlainApk(/* restartApp= */ true);

    verify(device, never())
        .installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            anyBoolean());
    verify(device).stopPackage("com.example.app");
  }

  @Test
  public void testPlainInstallInstallsAChangedApk() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ false);

    installPlainApk(/* restartApp= */ true);

    verify(device)
        .installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            /* verifyTempWritable= */ eq(true),
            /* stagedInstallMode= */ eq(false),
            /* userId= */ eq(null),
            /* allowFastDeploy= */ eq(true),
            eq("com.example.app"),
            eq(false));
    verify(device, never()).stopPackage(anyString());
  }

  @Test
  public void testPlainInstallInstallsWhenTheInstalledApkCannotBeRead() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);
    when(device.compareInstalledApk(eq("com.example.app"), any(File.class), eq(false)))
        .thenThrow(new AdbCommandFailedException("md5sum: base.apk: Permission denied"));

    installPlainApk(/* restartApp= */ false);

    verify(device)
        .installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            /* verifyTempWritable= */ eq(true),
            /* stagedInstallMode= */ eq(false),
            /* userId= */ eq(null),
            /* allowFastDeploy= */ eq(true),
            eq("com.example.app"),
            eq(false));
  }

  @Test
  public void testForceQueryableInstallsEvenWhenTheApkMatches() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);
    when(device.isPackageQueryable("com.example.app")).thenReturn(false);
    when(device.installApkOnDevice(
            any(File.class),
            eq(false),
            eq(true),
            eq(true),
            eq(false),
            eq(null),
            eq(true),
            eq("com.example.app"),
            eq(true)))
        .thenReturn(true);
    AdbOptions options =
        new AdbOptions("path_to_adb", 0, 0, false, false, false, false, "no", false, true);

    try (AdbHelper helper = createAdbHelper(options)) {
      installPlainApk(helper, /* restartApp= */ false);
    }

    verify(device)
        .installApkOnDevice(
            any(File.class),
            eq(false),
            eq(true),
            eq(true),
            eq(false),
            eq(null),
            eq(true),
            eq("com.example.app"),
            eq(true));
    verify(device).isPackageQueryable("com.example.app");
  }

  @Test
  public void testForceQueryableReusesAMatchingQueryableApkAndRestartsWhenRequested()
      throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);
    when(device.isPackageQueryable("com.example.app")).thenReturn(true);
    when(device.installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            eq(true)))
        .thenReturn(true);
    AdbOptions options =
        new AdbOptions("path_to_adb", 0, 0, false, false, false, false, "no", false, true);

    try (AdbHelper helper = createAdbHelper(options)) {
      installPlainApk(helper, /* restartApp= */ true);
    }

    verify(device).isPackageQueryable("com.example.app");
    verify(device).stopPackage("com.example.app");
    verify(device, never())
        .installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            anyBoolean());
  }

  @Test
  public void testForceQueryableStillInstallsChangedApkWithoutReadingVisibility() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ false);
    when(device.installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            eq(true)))
        .thenReturn(true);
    AdbOptions options =
        new AdbOptions("path_to_adb", 0, 0, false, false, false, false, "no", false, true);

    try (AdbHelper helper = createAdbHelper(options)) {
      installPlainApk(helper, /* restartApp= */ false);
    }

    verify(device, never()).isPackageQueryable(anyString());
    verify(device)
        .installApkOnDevice(
            any(File.class),
            eq(false),
            eq(true),
            eq(true),
            eq(false),
            eq(null),
            eq(true),
            eq("com.example.app"),
            eq(true));
  }

  @Test
  public void testForceQueryableInstallsWhenVisibilityCannotBeRead() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);
    when(device.isPackageQueryable("com.example.app"))
        .thenThrow(new AdbCommandFailedException("unreadable queryability"));
    when(device.installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            eq(true)))
        .thenReturn(true);
    AdbOptions options =
        new AdbOptions("path_to_adb", 0, 0, false, false, false, false, "no", false, true);

    try (AdbHelper helper = createAdbHelper(options)) {
      installPlainApk(helper, /* restartApp= */ false);
    }

    verify(device)
        .installApkOnDevice(
            any(File.class),
            eq(false),
            eq(true),
            eq(true),
            eq(false),
            eq(null),
            eq(true),
            eq("com.example.app"),
            eq(true));
  }

  @Test
  public void testPlainInstallDoesNotReadQueryabilityWithoutOptIn() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);
    installPlainApk(/* restartApp= */ false);
    verify(device, never()).isPackageQueryable(anyString());
  }

  @Test
  public void testPlainStagedInstallInstallsEvenWhenTheApkMatches() throws Exception {
    AndroidDeviceImpl device = deviceWithInstalledApk(/* sameApk= */ true);

    installPlainApk(
        createAdbHelper(
            createAdbOptions(
                /* multiInstallMode= */ false,
                /* stagedInstallMode= */ true,
                /* ignoreMissingDevices= */ false)),
        /* restartApp= */ false);

    verify(device)
        .installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            /* verifyTempWritable= */ eq(true),
            /* stagedInstallMode= */ eq(true),
            /* userId= */ eq(null),
            /* allowFastDeploy= */ eq(true),
            eq("com.example.app"),
            eq(false));
  }

  private File plainApk() throws Exception {
    File apk = tmpDir.getRoot().resolve("app.apk").toFile();
    if (!apk.exists()) {
      Files.write(apk.toPath(), new byte[] {1, 2, 3});
    }
    return apk;
  }

  /** A device whose installed apk for com.example.app is the local apk or a different one. */
  private AndroidDeviceImpl deviceWithInstalledApk(boolean sameApk) throws Exception {
    AndroidDeviceImpl device = createRealDevice("one", "device");
    when(device.setDebugAppPackageName("com.example.app")).thenReturn(true);
    when(device.compareInstalledApk(eq("com.example.app"), any(File.class), eq(false)))
        .thenReturn(
            sameApk ? AndroidDevice.InstalledApk.SAME : AndroidDevice.InstalledApk.DIFFERENT);
    when(device.installApkOnDevice(
            any(File.class),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            any(),
            anyBoolean(),
            anyString(),
            anyBoolean()))
        .thenReturn(true);
    when(adbUtils.getDevices()).thenReturn(Collections.singletonList(device));
    return device;
  }

  private void installPlainApk(boolean restartApp) throws Exception {
    installPlainApk(createAdbHelper(), restartApp);
  }

  private void installPlainApk(AdbHelper adbHelper, boolean restartApp) throws Exception {
    File apk = plainApk();
    adbHelper.installApk(
        new IsolatedApkInfo(
            tmpDir.getRoot().resolve("AndroidManifest.xml"), AbsPath.of(apk.toPath())),
        Optional.empty(),
        tmpDir.getRoot(),
        /* installViaSd= */ false,
        /* quiet= */ true,
        "//:app",
        "com.example.app",
        restartApp);
  }

  private AdbHelper createAdbHelper() {
    return createAdbHelper(createAdbOptions());
  }

  private AdbHelper createAdbHelper(AdbOptions options) {
    return new AdbHelper(
        adbUtils,
        options,
        new TargetDeviceOptions(),
        adbExecutionContext,
        new IsolatedAndroidInstallerPrinter(LOGGER),
        /* restartAdbOnFailure */ true,
        SetDebugAppMode.SET) {};
  }

  private AdbHelper createAdbHelper(List<AndroidDevice> deviceList) {
    AdbHelper.setDevicesSupplierForTests(
        Optional.of(
            () ->
                deviceList.stream()
                    .map(id -> new AndroidDeviceImpl(id.getSerialNumber(), adbUtils))
                    .collect(ImmutableList.toImmutableList())));

    return new AdbHelper(
        adbUtils,
        createAdbOptions(),
        new TargetDeviceOptions(),
        adbExecutionContext,
        new IsolatedAndroidInstallerPrinter(LOGGER),
        /* restartAdbOnFailure */ true,
        SetDebugAppMode.SET);
  }

  private static AdbOptions createAdbOptions() {
    return createAdbOptions(false);
  }

  private static AdbOptions createAdbOptions(boolean multiInstallMode) {
    return createAdbOptions(multiInstallMode, false, false);
  }

  private static AdbOptions createAdbOptions(
      boolean multiInstallMode, boolean ignoreMissingDevices) {
    return createAdbOptions(multiInstallMode, false, ignoreMissingDevices);
  }

  private static AdbOptions createAdbOptions(
      boolean multiInstallMode, boolean stagedInstallMode, boolean ignoreMissingDevices) {
    return new AdbOptions(
        "path_to_adb",
        0,
        0,
        multiInstallMode,
        stagedInstallMode,
        ignoreMissingDevices,
        false,
        "no",
        false);
  }
}
