/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import com.facebook.buck.jvm.java.JavacPluginParams;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.Iterables;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.ObjectOutputStream;
import java.net.URISyntaxException;
import java.net.URL;
import java.nio.file.Paths;
import java.util.Base64;
import java.util.Map;
import java.util.Objects;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class AnnotationProcessorUtils {
  public static final String KSP_PROCESSOR_NAME_PREFIX = "KSP:";

  static ImmutableList<ResolvedJavacPluginProperties> getAnnotationProcessors(
      JavacPluginParams annotationProcessorParams) {
    return annotationProcessorParams.isEmpty()
        ? ImmutableList.of()
        : annotationProcessorParams.getPluginProperties().asList();
  }

  static String encodeOptions(Map<String, String> options) {
    try (ByteArrayOutputStream os = new ByteArrayOutputStream();
        ObjectOutputStream oos = new ObjectOutputStream(os)) {

      oos.writeInt(options.size());
      for (Map.Entry<String, String> entry : options.entrySet()) {
        oos.writeUTF(entry.getKey());
        oos.writeUTF(entry.getValue());
      }

      oos.flush();
      return Base64.getEncoder().encodeToString(os.toByteArray());
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Safely converts a URL to a File path. Use this instead of {@link URL#getFile} to ensure that
   * htmlencoded literals are not present in the file path.
   */
  public static String urlToFile(URL url) {
    try {
      return Paths.get(url.toURI()).toFile().getPath();
    } catch (URISyntaxException e) {
      // In case of error, fall back to the original implementation.
      return url.getFile();
    }
  }

  /**
   * KSP Annotation processors are defined like Java's, but their name starts with
   * KSP_PROCESSOR_NAME_PREFIX
   */
  public static boolean isKSPProcessor(ResolvedJavacPluginProperties prop) {
    return !prop.getProcessorNames().isEmpty()
        && Objects.requireNonNull(Iterables.getFirst(prop.getProcessorNames(), ""))
            .startsWith(KSP_PROCESSOR_NAME_PREFIX);
  }

  /** Java only processors that run on Javac instead of KotlinC for java files in mix modules */
  public static boolean isRunsOnJavaOnlyProcessor(ResolvedJavacPluginProperties prop) {
    return prop.getRunsOnJavaOnly();
  }
}
