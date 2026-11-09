/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.nio;

import com.facebook.buck.jvm.java.version.utils.JavaVersionUtils;
import com.facebook.infer.annotation.Nullsafe;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;
import org.jspecify.annotations.Nullable;

/** Class to unmap ByteBuffer on any Java version. Supports try with resources. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ByteBufferUnmapper {
  private static final Unmapper unmapper =
      JavaVersionUtils.getMajorVersion() < 9 ? new UnmapperBeforeJava9() : new UnmapperAfterJava9();

  private ByteBufferUnmapper() {}

  /** Static method to unmap a ByteBuffer. */
  public static void unmap(ByteBuffer buffer) {
    if (!buffer.isDirect()) {
      return;
    }
    try {
      unmapper.unmap(buffer);
    } catch (Exception e) {
      throw new Error(e);
    }
  }

  private interface Unmapper {
    void unmap(ByteBuffer buffer) throws Exception;
  }

  private static class UnmapperAfterJava9 implements Unmapper {
    private static final @Nullable Object unsafe;
    private static final Method invokeCleaner;

    static {
      try {
        Class<?> unsafeClass = Class.forName("sun.misc.Unsafe");
        Field unsafeField = unsafeClass.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        unsafe = unsafeField.get(null);
        invokeCleaner = unsafeClass.getMethod("invokeCleaner", ByteBuffer.class);
      } catch (Exception e) {
        throw new Error(e);
      }
    }

    @Override
    public void unmap(ByteBuffer buffer) throws Exception {
      invokeCleaner.invoke(unsafe, buffer);
    }
  }

  private static class UnmapperBeforeJava9 implements Unmapper {
    private static final Method clean;

    static {
      try {
        clean = Class.forName("sun.misc.Cleaner").getMethod("clean");
        clean.setAccessible(true);
      } catch (Exception e) {
        throw new Error(e);
      }
    }

    @Override
    public void unmap(ByteBuffer buffer) throws Exception {
      Method cleaner = buffer.getClass().getMethod("cleaner");
      cleaner.setAccessible(true);
      clean.invoke(cleaner.invoke(buffer));
    }
  }
}
