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

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import java.nio.file.attribute.FileAttribute;
import java.nio.file.attribute.PosixFilePermission;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.Set;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class MorePosixFilePermissions {

  public static final FileAttribute<?> READ_ONLY_FILE_ATTRIBUTE =
      PosixFilePermissions.asFileAttribute(
          ImmutableSet.of(
              PosixFilePermission.OWNER_READ,
              PosixFilePermission.GROUP_READ,
              PosixFilePermission.OTHERS_READ));

  private MorePosixFilePermissions() {}

  private static final ImmutableList<PosixFilePermission> ORDERED_PERMISSIONS =
      ImmutableList.of(
          PosixFilePermission.OTHERS_EXECUTE,
          PosixFilePermission.OTHERS_WRITE,
          PosixFilePermission.OTHERS_READ,
          PosixFilePermission.GROUP_EXECUTE,
          PosixFilePermission.GROUP_WRITE,
          PosixFilePermission.GROUP_READ,
          PosixFilePermission.OWNER_EXECUTE,
          PosixFilePermission.OWNER_WRITE,
          PosixFilePermission.OWNER_READ);

  private static final ImmutableMap<PosixFilePermission, PosixFilePermission> READ_TO_EXECUTE_MAP =
      ImmutableMap.of(
          PosixFilePermission.OTHERS_READ, PosixFilePermission.OTHERS_EXECUTE,
          PosixFilePermission.GROUP_READ, PosixFilePermission.GROUP_EXECUTE,
          PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_EXECUTE);

  /** Convert a set of posix file permissions the unix bit representation (e.g. 0644). */
  public static long toMode(Set<PosixFilePermission> permissions) {
    long mode = 0;

    for (int index = 0; index < ORDERED_PERMISSIONS.size(); index++) {
      PosixFilePermission permission = ORDERED_PERMISSIONS.get(index);
      if (permissions.contains(permission)) {
        mode |= (1 << index);
      }
    }

    return mode;
  }

  /** Convert a unix bit representation (e.g. 0644) into a set of posix file permissions. */
  public static ImmutableSet<PosixFilePermission> fromMode(long mode) {
    ImmutableSet.Builder<PosixFilePermission> permissions = ImmutableSet.builder();

    for (int index = 0; index < ORDERED_PERMISSIONS.size(); index++) {
      if ((mode & (1 << index)) != 0) {
        permissions.add(ORDERED_PERMISSIONS.get(index));
      }
    }

    return permissions.build();
  }

  /**
   * Return a new set of permissions which include execute permission for each of the roles that
   * already have read permissions (e.g. 0606 =&gt; 0707).
   */
  public static ImmutableSet<PosixFilePermission> addExecutePermissionsIfReadable(
      Set<PosixFilePermission> permissions) {

    ImmutableSet.Builder<PosixFilePermission> newPermissions = ImmutableSet.builder();

    // The new permissions are a superset of the current ones.
    newPermissions.addAll(permissions);

    // If we see a read permission for the given role, add in the corresponding
    // execute permission.
    for (ImmutableMap.Entry<PosixFilePermission, PosixFilePermission> ent :
        READ_TO_EXECUTE_MAP.entrySet()) {
      if (permissions.contains(ent.getKey())) {
        newPermissions.add(ent.getValue());
      }
    }

    return newPermissions.build();
  }
}
