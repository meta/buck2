/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.IntBuffer;

/**
 * A ReferenceMapper is used to reassign ids in Android's .arsc/.xml files.
 *
 * <p>Android .arsc/.xml files include many resource references. These are ints of the form
 * 0xPPTTIIII encoding three things: package, type, index. The index part is an index into an array
 * of integers (or rather, into multiple arrays of integers).
 *
 * <p>A ReferenceMapper implements a method to update references and to rewrite those arrays that
 * they refer to.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface ReferenceMapper {
  /** Converts an id to its new value under this mapping. */
  int map(int id);

  /**
   * Given an IntBuffer with an entry for every value of the given type, rearranges the entries in
   * the buffer to match the id reassignment.
   */
  void rewrite(int type, IntBuffer buf);
}
