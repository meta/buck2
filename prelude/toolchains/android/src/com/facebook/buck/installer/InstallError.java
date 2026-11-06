/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.installer;

import com.facebook.infer.annotation.Nullsafe;
import java.util.Comparator;
import java.util.Set;
import java.util.stream.Collectors;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstallError {
  private final String message;
  private final Set<InstallErrorTag> tags;

  public InstallError(String message, InstallErrorTag installErrorTag) {
    this(message, Set.of(installErrorTag));
  }

  public InstallError(String message, Set<InstallErrorTag> installErrorTags) {
    this.message = message;
    this.tags = installErrorTags;
  }

  public String getMessage() {
    return String.format(
        "%s: %s",
        tags.stream().findFirst().map(InstallErrorTag::getName).orElse("UNKNOWN"), message);
  }

  public Set<InstallErrorTag> getTags() {
    return tags;
  }

  public com.facebook.buck.install.model.ErrorDetail toProtoModel() {

    InstallErrorCategory highestInstallErrorCategory =
        tags.stream()
            .map(InstallErrorTag::getErrorCategory)
            .min(
                Comparator.comparingInt(
                    token -> {
                      switch (token) {
                        case INFRA:
                          return 0;
                        case ENVIRONMENT:
                          return 1;
                        case USER:
                          return 2;
                        default:
                          return 3;
                      }
                    }))
            .orElse(InstallErrorCategory.UNKNOWN);
    return com.facebook.buck.install.model.ErrorDetail.newBuilder()
        // TODO(T213306104) Add a wiki link to guide users on troubleshooting installation errors.
        .setMessage(message)
        .addAllTags(tags.stream().map(InstallErrorTag::getName).collect(Collectors.toSet()))
        // Report highest priority category: Infra, then Environment, then User
        .setCategory(highestInstallErrorCategory.toProtoModel())
        .build();
  }

  @Override
  public String toString() {
    return "InstallError{" + "message='" + message + '\'' + ", tags=" + tags + '}';
  }
}
