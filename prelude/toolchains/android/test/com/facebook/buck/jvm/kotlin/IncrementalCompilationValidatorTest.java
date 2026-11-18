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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.File;
import java.nio.file.Path;
import java.nio.file.Paths;
import org.junit.Before;
import org.junit.Test;

public class IncrementalCompilationValidatorTest {

  private IncrementalCompilationValidator incrementalCompilationValidator;

  @Before
  public void setUp() {
    incrementalCompilationValidator = new IncrementalCompilationValidator();
  }

  @Test
  public void
      when_previousMetadataDigestIsNull_then_returnPreviousBuildConfigurationFileNotFoundReason() {
    ActionMetadata actionMetadata = mock(ActionMetadata.class);
    when(actionMetadata.getPreviousIncrementalConfigDigest()).thenReturn(null);

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, null, null, null);

    assertEquals(RebuildReason.NO_LAST_BUILD_CONFIGURATION, result);
  }

  @Test
  public void when_buildConfigurationChanged_then_returnBuildConfigurationChangedReason() {
    ActionMetadata actionMetadata = mock(ActionMetadata.class);
    when(actionMetadata.getPreviousIncrementalConfigDigest()).thenReturn("digest1");
    when(actionMetadata.getCurrentIncrementalConfigDigest()).thenReturn("digest2");

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, null, null, null);

    assertEquals(RebuildReason.BUILD_CONFIGURATION_CHANGED, result);
  }

  @Test
  public void when_contentBasedConfigChanged_then_returnBuildConfigurationChangedReason() {
    Path previousConfig = Paths.get("buck-out/__lib__/aaaaaaaaaaaaaaaa/incremental_config.json");
    Path currentConfig = Paths.get("buck-out/__lib__/bbbbbbbbbbbbbbbb/incremental_config.json");
    ActionMetadata actionMetadata =
        new ActionMetadata(
            currentConfig,
            ImmutableMap.of(previousConfig, "aaaaaaaaaaaaaaaa0123:155"),
            ImmutableMap.of(currentConfig, "bbbbbbbbbbbbbbbb4567:170"));

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, null, null, null);

    assertEquals(RebuildReason.BUILD_CONFIGURATION_CHANGED, result);
  }

  @Test
  public void when_contentBasedConfigUnchanged_then_noRebuild() {
    Path config = Paths.get("buck-out/__lib__/aaaaaaaaaaaaaaaa/incremental_config.json");
    ActionMetadata actionMetadata =
        new ActionMetadata(
            config,
            ImmutableMap.of(config, "aaaaaaaaaaaaaaaa0123:155"),
            ImmutableMap.of(config, "aaaaaaaaaaaaaaaa0123:155"));

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, null, null, null);

    assertNull(result);
  }

  @Test
  public void when_depFileNotExists_then_returnPreviousKotlinUsedClassesFileNotFoundReason() {
    ActionMetadata actionMetadata = mock(ActionMetadata.class);
    when(actionMetadata.getPreviousIncrementalConfigDigest()).thenReturn("digest");
    when(actionMetadata.getCurrentIncrementalConfigDigest()).thenReturn("digest");
    AbsPath depFile = mock(AbsPath.class);
    File mockFile = mock(File.class);
    when(depFile.toFile()).thenReturn(mockFile);
    when(mockFile.exists()).thenReturn(false);

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, depFile, null, null);

    assertEquals(RebuildReason.NO_LAST_DEP_FILE, result);
  }

  @Test
  public void when_usedJarsNotExists_then_returnNoLastUsedJarsReason() {
    ActionMetadata actionMetadata = mock(ActionMetadata.class);
    when(actionMetadata.getPreviousIncrementalConfigDigest()).thenReturn("digest");
    when(actionMetadata.getCurrentIncrementalConfigDigest()).thenReturn("digest");
    AbsPath usedJars = mock(AbsPath.class);
    File mockFile = mock(File.class);
    when(usedJars.toFile()).thenReturn(mockFile);
    when(mockFile.exists()).thenReturn(false);

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, null, usedJars, null);

    assertEquals(RebuildReason.NO_LAST_USED_JARS, result);
  }

  @Test
  public void when_jvmAbiGenWorkingDirNotExists_then_returnJvmAbiGenWorkingDirNotFoundReason() {
    ActionMetadata actionMetadata = mock(ActionMetadata.class);
    when(actionMetadata.getPreviousIncrementalConfigDigest()).thenReturn("digest");
    when(actionMetadata.getCurrentIncrementalConfigDigest()).thenReturn("digest");
    AbsPath jvmAbiGenWorkingDir = mock(AbsPath.class);
    File mockFile = mock(File.class);
    when(jvmAbiGenWorkingDir.toFile()).thenReturn(mockFile);
    when(mockFile.exists()).thenReturn(false);

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, null, null, jvmAbiGenWorkingDir);

    assertEquals(RebuildReason.NO_JVM_ABI_WORKING_DIR, result);
  }

  @Test
  public void when_kotlinCompilerPluginChanged_then_returnKotlinCompilerPluginChangedReason() {
    Path pluginPath = Paths.get("plugin.jar");
    RelPath pluginRelPath = RelPath.of(pluginPath);
    Path metadataPath = Paths.get("metadata.json");

    // Plugin digest changed from "oldDigest" to "newDigest"
    ActionMetadata actionMetadata =
        new ActionMetadata(
            metadataPath,
            ImmutableMap.of(metadataPath, "digest", pluginPath, "oldDigest"),
            ImmutableMap.of(metadataPath, "digest", pluginPath, "newDigest"));

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(pluginRelPath), actionMetadata, null, null, null);

    assertEquals(RebuildReason.KOTLIN_COMPILER_PLUGIN_CHANGED, result);
  }

  @Test
  public void when_kotlinCompilerPluginNotChanged_then_continueValidation() {
    Path pluginPath = Paths.get("plugin.jar");
    RelPath pluginRelPath = RelPath.of(pluginPath);
    Path metadataPath = Paths.get("metadata.json");

    // Plugin digest is the same in both previous and current
    ActionMetadata actionMetadata =
        new ActionMetadata(
            metadataPath,
            ImmutableMap.of(metadataPath, "digest", pluginPath, "sameDigest"),
            ImmutableMap.of(metadataPath, "digest", pluginPath, "sameDigest"));

    AbsPath depFile = mock(AbsPath.class);
    AbsPath usedJars = mock(AbsPath.class);
    AbsPath jvmAbiGenWorkingDir = mock(AbsPath.class);
    File mockFile1 = mock(File.class);
    File mockFile2 = mock(File.class);
    File mockFile3 = mock(File.class);
    when(depFile.toFile()).thenReturn(mockFile1);
    when(usedJars.toFile()).thenReturn(mockFile2);
    when(jvmAbiGenWorkingDir.toFile()).thenReturn(mockFile3);
    when(mockFile1.exists()).thenReturn(true);
    when(mockFile2.exists()).thenReturn(true);
    when(mockFile3.exists()).thenReturn(true);

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(pluginRelPath),
            actionMetadata,
            depFile,
            usedJars,
            jvmAbiGenWorkingDir);

    // Should return null since plugin hasn't changed and all files exist
    assertNull(result);
  }

  @Test
  public void when_allConditionsPass_then_returnNull() {
    Path metadataPath = Paths.get("metadata.json");
    ActionMetadata actionMetadata =
        new ActionMetadata(
            metadataPath,
            ImmutableMap.of(metadataPath, "digest"),
            ImmutableMap.of(metadataPath, "digest"));
    AbsPath depFile = mock(AbsPath.class);
    AbsPath usedJars = mock(AbsPath.class);
    AbsPath jvmAbiGenWorkingDir = mock(AbsPath.class);
    File mockFile1 = mock(File.class);
    File mockFile2 = mock(File.class);
    File mockFile3 = mock(File.class);
    when(depFile.toFile()).thenReturn(mockFile1);
    when(usedJars.toFile()).thenReturn(mockFile2);
    when(jvmAbiGenWorkingDir.toFile()).thenReturn(mockFile3);
    when(mockFile1.exists()).thenReturn(true);
    when(mockFile2.exists()).thenReturn(true);
    when(mockFile3.exists()).thenReturn(true);

    RebuildReason result =
        incrementalCompilationValidator.validate(
            ImmutableList.of(), actionMetadata, depFile, usedJars, jvmAbiGenWorkingDir);

    assertNull(result);
  }
}
