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

import static com.google.common.util.concurrent.MoreExecutors.listeningDecorator;

import com.facebook.buck.android.apex.ApexManifestProto.ApexManifest;
import com.facebook.buck.android.device.TargetDeviceOptions;
import com.facebook.buck.android.exopackage.AdbCommandFailedException;
import com.facebook.buck.android.exopackage.AdbUtils;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.buck.android.exopackage.AndroidDeviceInfo;
import com.facebook.buck.android.exopackage.AndroidDevicesHelper;
import com.facebook.buck.android.exopackage.AndroidIntent;
import com.facebook.buck.android.exopackage.ExopackageInstaller;
import com.facebook.buck.android.exopackage.InstallTimings;
import com.facebook.buck.android.exopackage.IsolatedExopackageInfo;
import com.facebook.buck.android.exopackage.SetDebugAppMode;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.util.MoreSuppliers;
import com.facebook.buck.util.Threads;
import com.facebook.buck.util.environment.EnvVariablesProvider;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Preconditions;
import com.google.common.base.Strings;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.Sets;
import com.google.common.util.concurrent.Futures;
import com.google.common.util.concurrent.ListenableFuture;
import com.google.common.util.concurrent.ListeningExecutorService;
import com.google.common.util.concurrent.MoreExecutors;
import com.google.common.util.concurrent.ThreadFactoryBuilder;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.Enumeration;
import java.util.HashSet;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CancellationException;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Function;
import java.util.function.Supplier;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import org.jetbrains.annotations.Nullable;

/** Helper for executing commands over ADB, especially for multiple devices. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class AdbHelper implements AndroidDevicesHelper {

  private static final Logger LOG = Logger.get(AdbHelper.class);

  /** Pattern that matches safe package names. (Must be a full string match). */
  public static final Pattern PACKAGE_NAME_PATTERN = Pattern.compile("[\\w.-]+");

  private static Optional<Supplier<ImmutableList<AndroidDevice>>> devicesSupplierForTests =
      Optional.empty();

  private static final Set<String> SUPPORTED_ABIS =
      ImmutableSet.of("armeabi-v7a", "arm64-v8a", "x86", "x86_64");

  public static final Path BUILD_METADATA_INSTALL_ROOT =
      Paths.get("/data/local/tmp/build_metadata/");

  /**
   * If this environment variable is set, the device with the specified serial number is targeted.
   * The -s option overrides this.
   */
  static final String SERIAL_NUMBER_ENV = "ANDROID_SERIAL";

  private static final String ABI_PROPERTY = "ro.product.cpu.abi";
  private static final String ABI_LIST_PROPERTY = "ro.product.cpu.abilist";

  static final int NUM_TRIES = 5;
  static final int RETRY_DELAY_MS = 1000;

  /**
   * The next port number to use for communicating with the agent on a device. This resets for every
   * instance of AdbHelper, but is incremented for every device on every call to adbCall().
   */
  private final AdbOptions options;

  private final TargetDeviceOptions deviceOptions;
  private final AdbExecutionContext adbExecutionContext;
  private final boolean restartAdbOnFailure;
  // Caches the list of android devices for this execution
  private final Supplier<GetDevicesResult> devicesSupplier;
  private final Supplier<ImmutableMap<String, ImmutableSet<String>>> deviceAbisSupplier;
  private final AndroidInstallPrinter androidPrinter;
  private final SetDebugAppMode setDebugAppMode;
  private final InstallTimings timings;

  @Nullable private ListeningExecutorService executorService = null;

  private final AdbUtils adbUtils;

  public AdbHelper(
      AdbUtils adbUtils,
      AdbOptions adbOptions,
      TargetDeviceOptions deviceOptions,
      AdbExecutionContext adbExecutionContext,
      AndroidInstallPrinter androidPrinter,
      boolean restartAdbOnFailure,
      SetDebugAppMode setDebugAppMode) {
    this(
        adbUtils,
        adbOptions,
        deviceOptions,
        adbExecutionContext,
        androidPrinter,
        restartAdbOnFailure,
        setDebugAppMode,
        InstallTimings.NONE);
  }

  public AdbHelper(
      AdbUtils adbUtils,
      AdbOptions adbOptions,
      TargetDeviceOptions deviceOptions,
      AdbExecutionContext adbExecutionContext,
      AndroidInstallPrinter androidPrinter,
      boolean restartAdbOnFailure,
      SetDebugAppMode setDebugAppMode,
      InstallTimings timings) {
    this.timings = timings;
    this.adbUtils = adbUtils;
    this.options = adbOptions;
    this.deviceOptions = deviceOptions;
    this.adbExecutionContext = adbExecutionContext;
    this.restartAdbOnFailure = restartAdbOnFailure;
    this.devicesSupplier = MoreSuppliers.memoize(this::getDevicesImpl);
    this.deviceAbisSupplier = MoreSuppliers.memoize(this::deviceAbisBySerialImpl);
    this.androidPrinter = androidPrinter;
    this.setDebugAppMode = setDebugAppMode;
  }

  @VisibleForTesting
  public static void setDevicesSupplierForTests(
      Optional<Supplier<ImmutableList<AndroidDevice>>> devicesSupplierForTests) {
    AdbHelper.devicesSupplierForTests = devicesSupplierForTests;
  }

  /**
   * Of the devices this helper resolved, those adb no longer reports.
   *
   * <p>The resolved set is fixed at its first use; this asks adb afresh on every call, so it is for
   * deciding once whether to go on rather than for polling. What it asks for is every serial adb
   * reports, not the filtered set: a superset can only shrink the answer, and a device that never
   * matched the filter was never resolved, so it can never be reported gone.
   */
  public ImmutableSet<String> departedSerials() {
    ImmutableSet<String> connected =
        adbUtils.getDevices().stream()
            .map(AndroidDevice::getSerialNumber)
            .collect(ImmutableSet.toImmutableSet());
    if (connected.isEmpty()) {
      // Adb answering with nothing is far more likely to be adb than every device at once. Reading
      // it as a mass departure would fail installs that a retry would have completed, and a device
      // that really has gone still fails the moment it is used.
      LOG.warn("adb reported no devices at all; not treating that as a disconnection");
      return ImmutableSet.of();
    }
    return devicesSupplier.get().devices.stream()
        .map(AndroidDevice::getSerialNumber)
        .filter(serial -> !connected.contains(serial))
        .collect(ImmutableSet.toImmutableSet());
  }

  @Override
  public ImmutableList<AndroidDevice> getDevices(boolean quiet) {
    GetDevicesResult result = devicesSupplier.get();
    if (!quiet && result.devices.size() > 1) {
      // Report if multiple devices are matching the filter.
      androidPrinter.printMessage("Found " + result.devices.size() + " matching devices.\n");
    }
    result.errorMessage.ifPresent(androidPrinter::printError);
    return result.devices;
  }

  /**
   * Execute an {@link AdbDeviceCallable} for all matching devices. This functions performs device
   * filtering based on three possible arguments:
   *
   * <p>-e (emulator-only) - only emulators are passing the filter -d (device-only) - only real
   * devices are passing the filter -s (serial) - only device/emulator with specific serial number
   * are passing the filter
   *
   * <p>If more than one device matches the filter this function will fail unless multi-install mode
   * is enabled (-x). This flag is used as a marker that user understands that multiple devices will
   * be used to install the apk if needed.
   */
  @SuppressWarnings("PMD.EmptyCatchBlock")
  @Override
  public synchronized void adbCall(String description, AdbDeviceCallable func, boolean quiet)
      throws InterruptedException {
    List<AndroidDevice> devices;

    try {
      GetDevicesResult result = devicesSupplier.get();
      if (!quiet && result.devices.size() > 1) {
        // Report if multiple devices are matching the filter.
        androidPrinter.printMessage("Found " + result.devices.size() + " matching devices.\n");
      }
      if (result.errorMessage.isPresent()) {
        throw new RuntimeException(result.errorMessage.get());
      } else if (result.devices.isEmpty()) {
        if (options.getIgnoreMissingDevice()) {
          androidPrinter.printMessage(
              "Proceeding anyway, even though we didn't find any attached Android"
                  + " devices/emulators.\n");
          return;
        }
        throw new RuntimeException("Didn't find any attached Android devices/emulators.");
      }
      devices = result.devices;
    } catch (Exception e) {
      throw new RuntimeException(e.toString());
    }

    // Start executions on all matching devices.
    List<ListenableFuture<Boolean>> futures = new ArrayList<>();
    for (AndroidDevice device : devices) {
      futures.add(
          getExecutorService()
              .submit(
                  () -> {
                    return func.apply(device);
                  }));
    }

    // Wait for all executions to complete or fail.
    List<Boolean> results;
    try {
      results = Futures.allAsList(futures).get();
    } catch (ExecutionException ex) {
      throw new RuntimeException(ex.getCause());
    } catch (InterruptedException e) {
      try {
        Futures.allAsList(futures).cancel(true);
      } catch (CancellationException ignored) {
        // Rethrow original InterruptedException instead.
      }
      Threads.interruptCurrentThread();
      throw e;
    }

    int successCount = 0;
    for (boolean result : results) {
      if (result) {
        successCount++;
      }
    }
    int failureCount = results.size() - successCount;

    // Report results.
    if (successCount > 0 && !quiet) {
      androidPrinter.printSuccess(
          String.format("Successfully ran %s on %d device(s)", description, successCount));
    }

    if (failureCount != 0) {
      throw new RuntimeException(
          String.format("Failed to %s on %d device(s).", description, failureCount));
    }
  }

  private synchronized ListeningExecutorService getExecutorService() {
    if (executorService != null) {
      return executorService;
    }
    int deviceCount;
    deviceCount = getDevices(true).size();
    int adbThreadCount = options.getAdbThreadCount();
    if (adbThreadCount <= 0) {
      adbThreadCount = deviceCount;
    }
    adbThreadCount = Math.min(deviceCount, adbThreadCount);
    executorService =
        listeningDecorator(
            Executors.newFixedThreadPool(
                adbThreadCount,
                new ThreadFactoryBuilder().setNameFormat(getClass().getSimpleName()).build()));
    return executorService;
  }

  public void installApk(
      IsolatedApkInfo isolatedApkInfo,
      Optional<IsolatedExopackageInfo> optionalIsolatedExopackageInfo,
      AbsPath rootPath,
      boolean installViaSd,
      boolean quiet,
      String fullyQualifiedName,
      String packageName,
      boolean restartApp)
      throws InterruptedException {
    Optional<String> buck2BuildUuid =
        Optional.ofNullable(EnvVariablesProvider.getSystemEnv().get("BUCK2_UUID"));

    AtomicBoolean success = new AtomicBoolean();

    if (optionalIsolatedExopackageInfo.isPresent()) {
      IsolatedExopackageInfo isolatedExopackageInfo = optionalIsolatedExopackageInfo.get();
      installApkExopackageWithRetries(
          rootPath,
          isolatedExopackageInfo,
          isolatedApkInfo,
          packageName,
          quiet,
          buck2BuildUuid,
          restartApp);
    } else {
      installApkDirectly(
          installViaSd,
          quiet,
          isolatedApkInfo,
          fullyQualifiedName,
          packageName,
          setDebugAppMode,
          buck2BuildUuid,
          restartApp);
    }
    success.set(true);
  }

  @Override
  public Set<AndroidDeviceInfo> getAndroidDeviceInfo(IsolatedApkInfo isolatedApkInfo)
      throws InterruptedException {
    Set<AndroidDeviceInfo> deviceInfos = new HashSet<>();

    adbCall(
        "Get device info",
        (device) -> {
          try {
            // Need to call both ro.product.cpu.abi and ro.product.cpu.abilist
            // as sticking to ro.product.cpu.abi helped fixing the issue of
            // exopackage install when the app was already installed in the device.
            String abi = device.getProperty(ABI_PROPERTY);
            Set<String> abiList =
                new HashSet<>(Arrays.asList(device.getProperty(ABI_LIST_PROPERTY).split(",")));
            String locale = getDeviceLocale(device);
            String buildFingerprint = device.getProperty("ro.build.fingerprint");
            String dpi = getDeviceDpi(device);
            String sdk = device.getProperty("ro.build.version.sdk");
            List<String> diskSpace = device.getDiskSpace();
            LOG.info(
                "Device disk size: %s, used: %s, available: %s",
                diskSpace.get(0), diskSpace.get(1), diskSpace.get(2));
            boolean isEmulator = device.isEmulator();

            AndroidDeviceInfo deviceInfo =
                new AndroidDeviceInfo(
                    locale,
                    abi,
                    abiList,
                    buildFingerprint,
                    dpi,
                    AndroidDeviceInfo.DensityClass.forPhysicalDensity(dpi),
                    sdk,
                    isEmulator,
                    AndroidDeviceInfo.transportOf(device.getSerialNumber()));
            LOG.info("Device info [%s]: %s", device.getSerialNumber(), deviceInfo);
            deviceInfos.add(deviceInfo);
          } catch (IncompatibleAbiException e) {
            throw e;
          } catch (Exception e) {
            // Don't log.
          }
          return true;
        },
        true);
    return deviceInfos;
  }

  /**
   * What each targeted device can run, by serial, read through the configured adb rather than
   * whatever `adb` is on the path. A device that will not say is left out rather than recorded as
   * running nothing, so callers can tell "cannot run this" from "would not answer".
   *
   * <p>Answered once per helper: these are boot-time properties of a fixed set of devices, and the
   * check that reads them is offered every time an artifact arrives.
   */
  public ImmutableMap<String, ImmutableSet<String>> deviceAbisBySerial() {
    return deviceAbisSupplier.get();
  }

  private ImmutableMap<String, ImmutableSet<String>> deviceAbisBySerialImpl() {
    ImmutableMap.Builder<String, ImmutableSet<String>> abis = ImmutableMap.builder();
    for (AndroidDevice device : getDevices(true)) {
      // Both properties, trimmed, blanks dropped: the list is comma separated and devices do put
      // spaces after the commas. Read separately so a device that answers one still contributes
      // it.
      ImmutableSet<String> forDevice =
          Stream.concat(
                  readProperty(device, ABI_PROPERTY).stream(),
                  readProperty(device, ABI_LIST_PROPERTY).stream()
                      .flatMap(list -> Arrays.stream(list.split(","))))
              .map(String::trim)
              .filter(abi -> !abi.isEmpty())
              .collect(ImmutableSet.toImmutableSet());
      if (!forDevice.isEmpty()) {
        abis.put(device.getSerialNumber(), forDevice);
      }
    }
    ImmutableMap<String, ImmutableSet<String>> bySerial = abis.build();
    if (bySerial.isEmpty()) {
      LOG.info("No targeted device would report an ABI");
    }
    return bySerial;
  }

  /** One property of one device, or empty if the device will not answer for it. */
  private Optional<String> readProperty(AndroidDevice device, String property) {
    try {
      return Optional.ofNullable(device.getProperty(property));
    } catch (Exception e) {
      LOG.warn(e, "Could not read %s of %s", property, device.getSerialNumber());
      return Optional.empty();
    }
  }

  public void throwIfIncompatibleAbi(
      AndroidDeviceInfo androidDeviceInfo, IsolatedApkInfo isolatedApkInfo)
      throws IncompatibleAbiException {
    Set<String> abis = getDeviceAbis(androidDeviceInfo.getAbi(), androidDeviceInfo.getAbiList());
    File apk = isolatedApkInfo.getApkPath().toFile();
    Set<String> apkAbis = getApkAbis(apk);
    if (!apkAbis.isEmpty() && Sets.intersection(abis, apkAbis).isEmpty()) {
      String errorMsg =
          String.format(
              "You are trying to install %s%s onto a device with the following CPU(s): %s. "
                  + "Please try again with a correct one. If you used an alias, "
                  + "some default aliases may have changed: https://fburl.com/workplace/p4ty6uvo.",
              apk.getName(),
              String.format(" (CPU(s): %s)", String.join(", ", apkAbis)),
              String.join(", ", abis));
      throw new IncompatibleAbiException(errorMsg);
    }
  }

  private Set<String> getDeviceAbis(String abi, Set<String> abiList) {
    return ImmutableSet.<String>builder().add(abi).addAll(abiList).build();
  }

  private Set<String> getApkAbis(File apk) {
    Set<String> abis = new HashSet<>();
    Pattern cpuPattern = Pattern.compile("lib/([^/]*)/");

    try (ZipFile zipFile = new ZipFile(apk)) {
      Enumeration<? extends ZipEntry> entries = zipFile.entries();
      while (entries.hasMoreElements()) {
        ZipEntry entry = entries.nextElement();
        Matcher m = cpuPattern.matcher(entry.getName());
        if (m.find()) {
          String abi = m.group(1);
          if (SUPPORTED_ABIS.contains(abi)) {
            abis.add(abi);
          }
        }
      }
    } catch (IOException e) {
      LOG.warn(e, "Failed to infer ABIs from apk: %s", apk);
    }

    return abis;
  }

  private static ImmutableMap<String, String> convertToMap(Set<AndroidDeviceInfo> infos) {
    ImmutableMap.Builder<String, String> map = ImmutableMap.builder();

    map.put("install_device_locales", toCommaList(infos, AndroidDeviceInfo::getLocale));
    map.put("install_device_abis", toCommaList(infos, AndroidDeviceInfo::getAbi));
    map.put(
        "install_device_build_fingerprint",
        toCommaList(infos, AndroidDeviceInfo::getBuildFingerprint));
    map.put("install_device_densities", toCommaList(infos, i -> i.getDensity().toString()));
    map.put("install_device_dpi", toCommaList(infos, AndroidDeviceInfo::getDotsPerInch));
    map.put("install_device_sdk", toCommaList(infos, AndroidDeviceInfo::getSdk));
    map.put("install_device_is_emulator", toCommaList(infos, i -> String.valueOf(i.isEmulator())));

    return map.build();
  }

  private static String toCommaList(
      Set<AndroidDeviceInfo> infos, Function<AndroidDeviceInfo, String> mapper) {
    return infos.stream().map(mapper).collect(Collectors.joining(","));
  }

  private static String getDeviceLocale(AndroidDevice device) throws Exception {
    // It's a bit tortuous to get the locale; there are 6 separate properties
    // we need to check to accurately record this.

    // First try "persist.sys" properties, which are the user's chosen language.
    String locale = device.getProperty("persist.sys.locale");
    // Try persist.sys.language + persist.sys.country
    if (Strings.isNullOrEmpty(locale)) {
      String language = device.getProperty("persist.sys.language");
      if (!Strings.isNullOrEmpty(language)) {
        String country = device.getProperty("persist.sys.country");
        if (!Strings.isNullOrEmpty(country)) {
          locale = language + "-" + country;
        }
      }
    }
    // Next try ro.product.locale properties which are the default system locale
    if (Strings.isNullOrEmpty(locale)) {
      locale = device.getProperty("ro.product.locale");
    }
    if (Strings.isNullOrEmpty(locale)) {
      String language = device.getProperty("ro.product.locale.language");
      String country = device.getProperty("ro.product.locale.region");

      // Default to en-US if all else fails
      if (Strings.isNullOrEmpty(language)) {
        language = "en";
      }
      if (Strings.isNullOrEmpty(country)) {
        country = "US-presumed";
      }
      locale = language + "-" + country;
    }
    return locale;
  }

  private static String getDeviceDpi(AndroidDevice device) throws Exception {
    // First, try getting it from a property
    String dpi = device.getProperty("ro.sf.lcd_density");
    if (Strings.isNullOrEmpty(dpi)) {
      // Try getting it from the WindowManager
      dpi = device.getProperty("density");
    }
    return dpi;
  }

  public void startActivityForIsolatedApk(
      IsolatedApkInfo isolatedApkInfo,
      String fullyQualifiedName,
      @Nullable String activity,
      @Nullable String intentUri,
      boolean waitForDebugger,
      boolean skipSetDebugApp)
      throws IOException {

    // Might need the package name and activities from the AndroidManifest.
    AndroidManifestReader reader =
        DefaultAndroidManifestReader.forPath(isolatedApkInfo.getManifestPath().getPath());

    final String packageName = reader.getPackage();

    final AndroidIntent intent;
    final String intentTargetNiceName;
    if (intentUri != null) {
      intent =
          new AndroidIntent(
              packageName,
              null,
              AndroidIntent.ACTION_VIEW,
              null,
              intentUri,
              null,
              waitForDebugger,
              skipSetDebugApp);
      intentTargetNiceName = intentUri;
    } else {
      if (activity == null) {
        // Get list of activities that show up in the launcher.
        List<String> launcherActivities = reader.getLauncherActivities();

        // Sanity check.
        if (launcherActivities.isEmpty()) {
          throw new RuntimeException("No launchable activities found.");
        } else if (launcherActivities.size() > 1) {
          throw new RuntimeException("Default activity is ambiguous.");
        }

        // Construct a component for the '-n' argument of 'adb shell am start'.
        activity = packageName + "/" + launcherActivities.get(0);
      } else if (!activity.contains("/")) {
        // If no package name was provided, assume the one in the manifest.
        activity = packageName + "/" + activity;
      }

      //  0x10200000 is FLAG_ACTIVITY_RESET_TASK_IF_NEEDED | FLAG_ACTIVITY_NEW_TASK; the
      // constant values are public ABI.  This way of invoking "am start" makes buck install -r
      // act just like the launcher, avoiding activity duplication on subsequent
      // launcher starts.
      intent =
          new AndroidIntent(
              packageName,
              activity,
              AndroidIntent.ACTION_MAIN,
              AndroidIntent.CATEGORY_LAUNCHER,
              null,
              "0x10200000",
              waitForDebugger,
              skipSetDebugApp);
      intentTargetNiceName = activity;
    }

    androidPrinter.printMessage(String.format("Starting activity %s...", intentTargetNiceName));

    try {
      adbCallOrThrow(
          "start activity",
          (device) -> {
            device.deviceStartIntent(intent);
            return true;
          },
          false);
    } catch (Exception e) {
    }
  }

  /**
   * Uninstall apk from all matching devices.
   *
   * @see #installApk(IsolatedApkInfo, Optional, AbsPath, boolean, boolean, String)
   */
  @Override
  public void uninstallApp(String packageName, boolean shouldKeepUserData)
      throws InterruptedException {
    Preconditions.checkArgument(AdbHelper.PACKAGE_NAME_PATTERN.matcher(packageName).matches());

    try {
      adbCall(
          "uninstall apk",
          (device) -> {
            device.uninstallApkFromDevice(packageName, shouldKeepUserData);
            return true;
          },
          false);
    } catch (RuntimeException e) {
      throw e;
    }
  }

  public static String tryToExtractPackageNameFromManifest(Path pathToManifest) {
    // Note that the file may not exist if AndroidManifest.xml is a generated file
    // and the rule has not been built yet.
    if (!Files.isRegularFile(pathToManifest)) {
      throw new RuntimeException(
          String.format(
              "Manifest file %s does not exist, so could not extract package name.",
              pathToManifest));
    }

    try {
      return DefaultAndroidManifestReader.forPath(pathToManifest).getPackage();
    } catch (IOException e) {
      throw new RuntimeException(
          String.format("Could not extract package name from %s", pathToManifest));
    }
  }

  /**
   * Returns list of devices that pass the filter. If there is an invalid combination or no devices
   * are left after filtering this function prints an error and returns null.
   */
  @VisibleForTesting
  List<AndroidDevice> filterDevices(List<AndroidDevice> allDevices) {
    if (allDevices.isEmpty()) {
      androidPrinter.printError("No devices are found.");
      return Collections.emptyList();
    }

    List<AndroidDevice> devices = new ArrayList<>();
    Optional<Boolean> emulatorsOnly = Optional.empty();
    if (deviceOptions.isEmulatorsOnlyModeEnabled() && options.isMultiInstallModeEnabled()) {
      emulatorsOnly = Optional.empty();
    } else if (deviceOptions.isEmulatorsOnlyModeEnabled()) {
      emulatorsOnly = Optional.of(true);
    } else if (deviceOptions.isRealDevicesOnlyModeEnabled()) {
      emulatorsOnly = Optional.of(false);
    }

    int onlineDevices = 0;
    for (AndroidDevice device : allDevices) {
      boolean passed = false;
      if (device.isOnline()) {
        LOG.info("Found online device: %s", device.getSerialNumber());
        onlineDevices++;

        boolean serialMatches = true;
        // -s option or $ANDROID_SERIAL could contain
        // "tcp:" prefixed serial number for TCP connected device.
        // It is valid but the following logic needs to remove it.
        if (deviceOptions.getSerialNumber().isPresent()
            || getEnvironment().containsKey(SERIAL_NUMBER_ENV)) {
          String expectedSerialNumber =
              deviceOptions
                  .getSerialNumber()
                  .orElse(getEnvironment().get(SERIAL_NUMBER_ENV))
                  .replaceFirst("^tcp:", "");
          serialMatches = expectedSerialNumber.equals(device.getSerialNumber());
          LOG.info(
              "Device: %s %s expected serial#: %s",
              device.getSerialNumber(),
              serialMatches ? "matches" : "does not match",
              expectedSerialNumber);
        }

        // Only devices of specific type are accepted:
        // either real devices only or emulators only.
        // All online devices match.
        boolean isDeviceEmulator = device.isEmulator();
        boolean deviceTypeMatches =
            emulatorsOnly.map(isEmulatorOnly -> (isEmulatorOnly == isDeviceEmulator)).orElse(true);
        LOG.info(
            "Device: %s %s expected device type. Actual: %s - expected: %s",
            device.getSerialNumber(),
            deviceTypeMatches ? "matches" : "does not match",
            isDeviceEmulator ? "emulator" : "real",
            emulatorsOnly.map(p -> p ? "emulator" : "real").orElse("any"));
        passed = serialMatches && deviceTypeMatches;
      } else {
        LOG.info("Found offline device: %s", device.getSerialNumber());
      }

      if (passed) {
        LOG.info("Device: %s passed filter", device.getSerialNumber());
        devices.add(device);
      } else {
        LOG.info("Device: %s did not pass filter", device.getSerialNumber());
      }
    }

    // Filtered out all devices.
    if (onlineDevices == 0) {
      androidPrinter.printError("No devices are found.");
      return Collections.emptyList();
    }

    if (devices.isEmpty()) {
      androidPrinter.printError(
          String.format(
              "Found %d connected device(s), but none of them matches specified filter.",
              onlineDevices));
      return Collections.emptyList();
    }

    return devices;
  }

  private ImmutableMap<String, String> getEnvironment() {
    return adbExecutionContext.getEnvironment();
  }

  private static class GetDevicesResult {

    private final ImmutableList<AndroidDevice> devices;
    private final Optional<String> errorMessage;

    private GetDevicesResult(ImmutableList<AndroidDevice> devices, Optional<String> errorMessage) {
      this.devices = devices;
      this.errorMessage = errorMessage;
    }

    static GetDevicesResult createSuccess(ImmutableList<AndroidDevice> devices) {
      return new GetDevicesResult(devices, Optional.empty());
    }

    static GetDevicesResult createFailure(String message) {
      return new GetDevicesResult(ImmutableList.of(), Optional.of(message));
    }
  }

  private GetDevicesResult getDevicesImpl() {
    if (devicesSupplierForTests.isPresent()) {
      return GetDevicesResult.createSuccess(devicesSupplierForTests.get().get());
    }

    // Build list of matching devices.
    List<AndroidDevice> devices = filterDevices(adbUtils.getDevices());
    // Found multiple devices but multi-install mode is not enabled.
    if (devices.size() > 1 && !options.isMultiInstallModeEnabled()) {
      return GetDevicesResult.createFailure(
          String.format(
              "%d devices match specified device filter (1 expected).\n"
                  + "Either disconnect other devices or enable multi-install mode (%s).",
              devices.size(), AdbOptions.MULTI_INSTALL_MODE_SHORT_ARG));
    }

    if (devices.isEmpty() && restartAdbOnFailure) {
      androidPrinter.printError("No devices found with adb, restarting adb-server.");
      adbUtils.restart();
      devices = filterDevices(adbUtils.getDevices());
    }

    return GetDevicesResult.createSuccess(
        devices.stream().collect(ImmutableList.toImmutableList()));
  }

  @Override
  public synchronized void close() {
    // getExecutorService() requires the context for lazy initialization, so explicitly check if it
    // has been initialized.
    if (executorService != null) {
      if (!MoreExecutors.shutdownAndAwaitTermination(executorService, 10, TimeUnit.MINUTES)) {
        throw new RuntimeException("Failed to shutdown ExecutorService.");
      }
      executorService = null;
    }
  }

  private void installApkExopackageWithRetries(
      AbsPath rootPath,
      IsolatedExopackageInfo isolatedExopackageInfo,
      IsolatedApkInfo isolatedApkInfo,
      String packageName,
      boolean quiet,
      Optional<String> buck2BuildUuid,
      boolean restartApp)
      throws InterruptedException {
    int attempt = 1;
    while (true) {
      try {
        installApkExopackage(
            rootPath,
            isolatedExopackageInfo,
            isolatedApkInfo,
            packageName,
            quiet,
            buck2BuildUuid,
            restartApp);
        LOG.info("Installing the APK using exopackage succeeded on attempt %d", attempt);
        return;
      } catch (RuntimeException e) {
        if (attempt >= NUM_TRIES) {
          LOG.error(e, "Failed to install the APK using exopackage after %d attempts.", NUM_TRIES);

          throw e;
        } else {
          // installation failed, so log the error and continue with the next retry
          LOG.warn(e, "Error installing APK using exopackage (attempt %d/%d).", attempt, NUM_TRIES);

          Thread.sleep(RETRY_DELAY_MS);
          ++attempt;
        }
      }
    }
  }

  /**
   * Pushes exopackage payloads to the matching devices ahead of the install itself.
   *
   * <p>Only pushes content. Metadata, collection of stale files and the apk all need the complete
   * artifact set, so they stay in {@link ExopackageInstaller#doInstall}, which lists the directory
   * again and skips whatever landed here.
   */
  public void streamExopackagePayloads(
      AbsPath rootPath, IsolatedExopackageInfo isolatedExopackageInfo, String packageName)
      throws InterruptedException {
    adbCall(
        "push exopackage files",
        device -> {
          new ExopackageInstaller(
                  isolatedExopackageInfo,
                  androidPrinter,
                  rootPath,
                  packageName,
                  device,
                  Optional.empty(),
                  timings)
              .streamPayloads();
          return true;
        },
        /* quiet= */ true);
  }

  private void installApkExopackage(
      AbsPath rootPath,
      IsolatedExopackageInfo isolatedExopackageInfo,
      IsolatedApkInfo isolatedApkInfo,
      String packageName,
      boolean quiet,
      Optional<String> buck2BuildUuid,
      boolean restartApp)
      throws InterruptedException {
    adbCall(
        "install exopackage apk",
        device -> {
          new ExopackageInstaller(
                  isolatedExopackageInfo,
                  androidPrinter,
                  rootPath,
                  packageName,
                  device,
                  buck2BuildUuid,
                  timings)
              .doInstall(isolatedApkInfo, setDebugAppMode, restartApp, options.isForceQueryable());
          return true;
        },
        quiet);
  }

  private void installApkDirectly(
      boolean installViaSd,
      boolean quiet,
      IsolatedApkInfo isolatedApkInfo,
      String buildTarget,
      String packageName,
      SetDebugAppMode setDebugAppMode,
      Optional<String> buck2BuildUuid,
      boolean restartApp)
      throws InterruptedException {
    File apk = isolatedApkInfo.getApkPath().toFile();
    if (setDebugAppMode == SetDebugAppMode.SET) {
      adbCall("set debug app", (device) -> device.setDebugAppPackageName(packageName), true);
    }
    adbCall(
        String.format("install apk %s", buildTarget),
        (device) -> {
          if (options.isApexModeEnabled()) {

            boolean restart = false;
            if (options.getRestartMode().equals("yes")) { // Restart
              restart = true;
            } else if (options.getRestartMode().equals("no")) { // Don't Restart
              restart = false;
            } else { // Auto - Only restart shell if rebootless is supported
              restart = isApexFileSupportsRebootlessUpdate(apk);
            }
            return device.installApexOnDevice(apk, quiet, restart, options.getWaitForDeviceReady());
          } else if (!options.isStagedInstallModeEnabled()
              && !shouldAppBeInstalled(
                  device, packageName, apk, androidPrinter, options.isForceQueryable())) {
            if (restartApp) {
              device.stopPackage(packageName);
            }
            return true;
          } else {
            return device.installApkOnDevice(
                apk,
                installViaSd,
                quiet,
                /* verifyTempWritable= */ true,
                options.isStagedInstallModeEnabled(),
                /* userId= */ null,
                /* allowFastDeploy= */ true,
                packageName,
                options.isForceQueryable());
          }
        },
        quiet);

    if (buck2BuildUuid.isPresent()) {
      adbCall(
          "install build_uuid.txt",
          (device) ->
              device.installBuildUuidFile(
                  BUILD_METADATA_INSTALL_ROOT, packageName, buck2BuildUuid.get()),
          true);
    }
  }

  /**
   * Whether {@code device} lacks {@code apk} as the installed apk of {@code packageName}. With
   * {@code forceQueryable}, also true when the installed package is not queryable or its
   * queryability cannot be read.
   */
  public static boolean shouldAppBeInstalled(
      AndroidDevice device,
      String packageName,
      File apk,
      AndroidInstallPrinter androidPrinter,
      boolean forceQueryable)
      throws Exception {
    AndroidDevice.InstalledApk installed;
    try {
      installed = device.compareInstalledApk(packageName, apk, /* exact= */ false);
    } catch (AdbCommandFailedException e) {
      LOG.info("Could not read the installed APK.  Must re-install: %s", e.getMessage());
      return true;
    }
    switch (installed) {
      case ABSENT:
        androidPrinter.printMessage("App not installed.  Installing now.");
        return true;
      case DIFFERENT:
        LOG.info("Installed APK differs.  Must re-install.");
        return true;
      default:
        if (forceQueryable) {
          try {
            if (!device.isPackageQueryable(packageName)) {
              LOG.info("Installed APK is not queryable.  Must re-install.");
              return true;
            }
          } catch (AdbCommandFailedException e) {
            LOG.info("Could not read package queryability.  Must re-install: %s", e.getMessage());
            return true;
          }
        }
        LOG.info("Installed APK matches.  No need to install.");
        return false;
    }
  }

  private boolean isApexFileSupportsRebootlessUpdate(File apexFile) {
    try {
      // Open APEX file as a zip archive
      try (ZipFile zipFile = new ZipFile(apexFile)) {
        // Find the apex_manifest.pb entry
        ZipEntry manifestEntry = zipFile.getEntry("apex_manifest.pb");

        if (manifestEntry == null) {
          LOG.warn("apex_manifest.pb not found in APEX file: %s", apexFile.getName());
          return false;
        }

        // Extract the manifest directly to byte array
        byte[] manifestBytes;
        try (java.io.InputStream inputStream =
                Objects.requireNonNull(zipFile.getInputStream(manifestEntry));
            java.io.ByteArrayOutputStream outputStream = new java.io.ByteArrayOutputStream()) {
          byte[] buffer = new byte[8192];
          int bytesRead;
          while ((bytesRead = inputStream.read(buffer)) != -1) {
            outputStream.write(buffer, 0, bytesRead);
          }
          manifestBytes = outputStream.toByteArray();
        }

        LOG.info(
            "Extracted apex_manifest.pb from %s (%d bytes)",
            apexFile.getName(), manifestBytes.length);

        // Parse manifestBytes to check for rebootless update support
        ApexManifest apexManifest = ApexManifest.parseFrom(manifestBytes);

        LOG.info(
            "supportsRebootlessUpdate: %s",
            apexManifest.getSupportsRebootlessUpdate() ? "true" : "false");

        return apexManifest.getSupportsRebootlessUpdate();
      }
    } catch (IOException e) {
      LOG.warn(e, "Failed to extract apex_manifest.pb from %s", apexFile.getName());
      return false;
    }
  }

  private static class IncompatibleAbiException extends InterruptedException {
    public IncompatibleAbiException(String message) {
      super(message);
    }
  }
}
