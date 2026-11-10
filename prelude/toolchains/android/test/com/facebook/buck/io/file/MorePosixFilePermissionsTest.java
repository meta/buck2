/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.io.file;

import static org.junit.Assert.assertEquals;

import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.Sets;
import java.nio.file.attribute.PosixFilePermission;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.HashSet;
import java.util.Objects;
import java.util.Set;
import org.junit.Test;

public class MorePosixFilePermissionsTest {

  static class Sample {

    public final Set<PosixFilePermission> permissions;
    public final long mode;

    public Sample(long mode, Set<PosixFilePermission> permissions) {
      this.permissions = Objects.requireNonNull(permissions);
      this.mode = mode;
    }
  }

  // Some examples of converting between unix modes and perm sets.
  private static final ImmutableList<Sample> CONVERSION_SAMPLES =
      ImmutableList.of(
          new Sample(040, Sets.newHashSet(PosixFilePermission.GROUP_READ)),
          new Sample(
              0600,
              Sets.newHashSet(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE)),
          new Sample(0777, Sets.newHashSet(PosixFilePermission.values())),
          new Sample(01, Sets.newHashSet(PosixFilePermission.OTHERS_EXECUTE)),
          new Sample(0, new HashSet<>()));

  // Some examples that test various cases of adding in execute perms for readers.
  private static final ImmutableMap<String, String> ADD_EXECUTE_PERM_SAMPLES =
      ImmutableMap.of(
          "rw-r--r--", "rwxr-xr-x",
          "rw----r--", "rwx---r-x",
          "------r--", "------r-x",
          "---------", "---------",
          "r-x------", "r-x------");

  @Test
  public void testConversionToAndFromDoesNotChangePermissions() {
    for (Sample sample : CONVERSION_SAMPLES) {
      assertEquals(
          sample.permissions,
          MorePosixFilePermissions.fromMode(MorePosixFilePermissions.toMode(sample.permissions)));
    }
  }

  @Test
  public void testConversionToAndFromDoesNotChangeMode() {
    for (Sample sample : CONVERSION_SAMPLES) {
      assertEquals(
          sample.mode,
          MorePosixFilePermissions.toMode(MorePosixFilePermissions.fromMode(sample.mode)));
    }
  }

  @Test
  public void testModeAndPermissionsAreEqualAfterConversionEitherWay() {
    for (Sample sample : CONVERSION_SAMPLES) {
      assertEquals(MorePosixFilePermissions.fromMode(sample.mode), sample.permissions);
      assertEquals(sample.mode, MorePosixFilePermissions.toMode(sample.permissions));
    }
  }

  @Test
  public void testAddingExecutePermissionsIfReadable() {
    for (ImmutableMap.Entry<String, String> ent : ADD_EXECUTE_PERM_SAMPLES.entrySet()) {
      assertEquals(
          MorePosixFilePermissions.addExecutePermissionsIfReadable(
              PosixFilePermissions.fromString(ent.getKey())),
          PosixFilePermissions.fromString(ent.getValue()));
    }
  }
}
