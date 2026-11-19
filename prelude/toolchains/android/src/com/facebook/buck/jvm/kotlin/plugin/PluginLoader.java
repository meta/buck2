/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.plugin;

import com.facebook.infer.annotation.Nullsafe;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.net.URISyntaxException;
import java.net.URL;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.util.Objects;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class PluginLoader {

  private static final String DEP_TRACKER_KOTLINC_PLUGIN_JAR_RESOURCE_PATH = "dep-tracker.jar";

  public static final File DEP_TRACKER_KOTLINC_PLUGIN_JAR_PATH = extractDepTrackerPlugin();

  private static File extractDepTrackerPlugin() {
    try {
      return doExtractDepTrackerPlugin();
    } catch (URISyntaxException | IOException | NullPointerException e) {
      throw new RuntimeException("Failed to extract kotlinc plugin jar", e);
    }
  }

  private static File doExtractDepTrackerPlugin() throws URISyntaxException, IOException {
    URL resource = PluginLoader.class.getResource(DEP_TRACKER_KOTLINC_PLUGIN_JAR_RESOURCE_PATH);
    if (resource == null) {
      throw new RuntimeException("Could not find dep-tracker kotlinc plugin jar");
    }
    if ("file".equals(resource.getProtocol())) {
      return new File(resource.toURI());
    }
    // TODO: This Extraction technique will be deprecated as we will provide dep-tracker dependency
    //   with a 3p buck rule, same as Kosabi and Nullsafe.
    try (InputStream in =
        PluginLoader.class.getResourceAsStream(DEP_TRACKER_KOTLINC_PLUGIN_JAR_RESOURCE_PATH)) {
      File plugin = File.createTempFile("dep-tracker", ".jar");
      plugin.deleteOnExit();
      Files.copy(
          Objects.requireNonNull(in),
          Paths.get(plugin.toURI()),
          StandardCopyOption.REPLACE_EXISTING);
      return plugin;
    }
  }
}
