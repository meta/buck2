/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip.collect;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/**
 * A set of sources to store as entries in a zip file. A source entry can be either a file or an
 * entry from another zip file.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipEntrySourceCollection {

  private final ImmutableList<ZipEntrySource> sources;

  public ZipEntrySourceCollection(ImmutableList<ZipEntrySource> sources) {
    this.sources = sources;
  }

  public ImmutableList<ZipEntrySource> getSources() {
    return sources;
  }

  @Override
  public boolean equals(@Nullable Object o) {
    if (this == o) {
      return true;
    }
    if (o == null || getClass() != o.getClass()) {
      return false;
    }
    ZipEntrySourceCollection that = (ZipEntrySourceCollection) o;
    return Objects.equals(sources, that.sources);
  }

  @Override
  public int hashCode() {
    return Objects.hash(sources);
  }
}
