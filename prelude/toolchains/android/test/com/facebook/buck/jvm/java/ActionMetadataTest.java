/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.Map;
import org.junit.Test;

public class ActionMetadataTest {

  @Test
  public void
      when_currentDigestContainsConfigFile_then_getCurrentIncrementalConfigDigestReturnsCorrectValue() {
    Path configFile = Paths.get("config.json");
    Map<Path, String> previousDigest = new HashMap<>();
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(configFile, "metadata-digest");
    currentDigest.put(Paths.get("other.file"), "other-digest");
    ActionMetadata metadata = new ActionMetadata(configFile, previousDigest, currentDigest);

    String currentIncrementalConfigDigest = metadata.getCurrentIncrementalConfigDigest();

    assertEquals("metadata-digest", currentIncrementalConfigDigest);
  }

  @Test
  public void when_digestsDoNotContainMetadataFile_then_incrementalConfigDigestReturnsNull() {
    Path configFile = Paths.get("config.json");
    Map<Path, String> previousDigest = new HashMap<>();
    Map<Path, String> currentDigest = new HashMap<>();
    ActionMetadata metadata = new ActionMetadata(configFile, previousDigest, currentDigest);

    String previousIncrementalConfigDigest = metadata.getPreviousIncrementalConfigDigest();
    String currentIncrementalConfigDigest = metadata.getCurrentIncrementalConfigDigest();

    assertNull(previousIncrementalConfigDigest);
    assertNull(currentIncrementalConfigDigest);
  }

  @Test
  public void when_contentBasedConfigPathChanged_then_previousConfigIsFoundWithoutContentHash() {
    Path previousConfig =
        Paths.get("buck-out/v2/art/fbcode/a/__lib__/aaaaaaaaaaaaaaaa/incremental_config.json");
    Path currentConfig =
        Paths.get("buck-out/v2/art/fbcode/a/__lib__/bbbbbbbbbbbbbbbb/incremental_config.json");
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(previousConfig, "aaaaaaaaaaaaaaaa0123:155");
    previousDigest.put(
        Paths.get("buck-out/v2/art/fbcode/a/__dep__/cccccccccccccccc/incremental_config.json"),
        "cccccccccccccccc0123:155");
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(currentConfig, "bbbbbbbbbbbbbbbb4567:170");
    ActionMetadata metadata = new ActionMetadata(currentConfig, previousDigest, currentDigest);

    assertEquals("aaaaaaaaaaaaaaaa0123:155", metadata.getPreviousIncrementalConfigDigest());
    assertEquals("bbbbbbbbbbbbbbbb4567:170", metadata.getCurrentIncrementalConfigDigest());
  }

  @Test
  public void when_previousConfigBelongsToAnotherOutput_then_previousDigestIsNull() {
    Path currentConfig =
        Paths.get("buck-out/v2/art/fbcode/a/__lib__/bbbbbbbbbbbbbbbb/incremental_config.json");
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(
        Paths.get("buck-out/v2/art/fbcode/a/__dep__/cccccccccccccccc/incremental_config.json"),
        "cccccccccccccccc0123:155");
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(currentConfig, "bbbbbbbbbbbbbbbb4567:170");
    ActionMetadata metadata = new ActionMetadata(currentConfig, previousDigest, currentDigest);

    assertNull(metadata.getPreviousIncrementalConfigDigest());
  }
}
