/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.support.exopackage;

import android.content.Context;
import android.util.Log;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Loads pre-dexed jars installed by the exopackage installer into our ClassLoader.
 *
 * <p>See http://android-developers.blogspot.com/2011/07/custom-class-loading-in-dalvik.html for an
 * explanation of how an app can load pre-dexed libraries. This class goes a step further and then
 * hacks the system class loader so these can be referenced by default. This logic has to run before
 * any class that references the code in these dexes is loaded.
 */
public class ExopackageDexLoader {

  private static final String TAG = "ExopackageDexLoader";

  private ExopackageDexLoader() {}

  /**
   * Load JARs installed by Buck's Exopackage installer and add them to the Application ClassLoader.
   *
   * @param context The application context.
   */
  public static void loadExopackageJars(Context context) {
    // Normal exopackage jars go to system CL
    File secondaryDirectory =
        new File("/data/local/tmp/exopackage/" + context.getPackageName() + "/secondary-dex");
    final List<File> dexJars = getJarFilesFromContainingDirectory(secondaryDirectory);
    File dexOptDir = context.getDir("exopackage_dex_opt", Context.MODE_PRIVATE);
    SystemClassLoaderAdder.installDexJars(context.getClassLoader(), dexOptDir, dexJars);
    cleanUpOldOdexFiles(dexOptDir, dexJars);
  }

  /** Find all .dex.jar files in the given directory */
  public static List<File> getJarFilesFromContainingDirectory(File containingDirectory) {
    List<File> dexJars = new ArrayList();
    try {
      final File metadataFile = new File(containingDirectory, "metadata.txt");
      if (!metadataFile.exists()) {
        return dexJars;
      }
      BufferedReader metadataReader =
          new BufferedReader(new InputStreamReader(new FileInputStream(metadataFile)));
      try {
        String line;
        while ((line = metadataReader.readLine()) != null) {
          int space = line.indexOf(' ');
          if (space == -1) {
            throw new RuntimeException("Found no space in line: " + line);
          }
          String baseName = line.substring(0, space);
          dexJars.add(new File(containingDirectory, baseName));
        }
      } finally {
        metadataReader.close();
      }
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
    return dexJars;
  }

  /**
   * Clean up any odex files that do not belong to the specified set of jars
   *
   * @param dexOptDir the directory to clean
   * @param dexJars odex files pertaining to these jars will be kept
   */
  public static void cleanUpOldOdexFiles(File dexOptDir, List<File> dexJars) {
    Set<String> expectedOdexSet = new HashSet<>();
    for (File file : dexJars) {
      expectedOdexSet.add(file.getName().replaceFirst("\\.jar$", ".dex"));
    }
    File[] odexes = dexOptDir.listFiles();
    if (odexes != null) {
      for (File odex : odexes) {
        if (!expectedOdexSet.contains(odex.getName())) {
          if (!odex.delete()) {
            Log.w(TAG, "Failed to delete stale odex: " + odex.getAbsolutePath());
          }
        }
      }
    }
  }
}
