/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex;

import static com.facebook.buck.android.dex.DexHeader.DEX_FIELD_IDS_SIZE_OFFSET;
import static com.facebook.buck.android.dex.DexHeader.DEX_METHOD_IDS_SIZE_OFFSET;
import static com.facebook.buck.android.dex.DexHeader.DEX_TYPE_IDS_SIZE_OFFSET;

import com.android.tools.r8.ByteDataView;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Objects;
import java.util.Set;

/** Classes and reference counts in an indivisible class-file dex output. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class DexUnitInfo {
  private static final int MIN_HEADER_SIZE = 112;

  private final ImmutableSet<String> descriptors;
  private final int methodRefs;
  private final int fieldRefs;
  private final int typeRefs;
  private final int byteSize;

  DexUnitInfo(ByteDataView data, Set<String> descriptors) {
    if (data.getLength() < MIN_HEADER_SIZE) {
      throw new IllegalArgumentException(
          "Truncated dex unit: expected at least "
              + MIN_HEADER_SIZE
              + " header bytes, got "
              + data.getLength());
    }
    this.descriptors = ImmutableSet.copyOf(descriptors);
    byteSize = data.getLength();
    ByteBuffer header =
        Objects.requireNonNull(
                ByteBuffer.wrap(data.getBuffer(), data.getOffset(), data.getLength()))
            .slice()
            .order(ByteOrder.LITTLE_ENDIAN);
    methodRefs = header.getInt(DEX_METHOD_IDS_SIZE_OFFSET);
    fieldRefs = header.getInt(DEX_FIELD_IDS_SIZE_OFFSET);
    typeRefs = header.getInt(DEX_TYPE_IDS_SIZE_OFFSET);
  }

  public ImmutableSet<String> getDescriptors() {
    return descriptors;
  }

  public int getByteSize() {
    return byteSize;
  }

  public int getMethodRefs() {
    return methodRefs;
  }

  public int getFieldRefs() {
    return fieldRefs;
  }

  public int getTypeRefs() {
    return typeRefs;
  }
}
