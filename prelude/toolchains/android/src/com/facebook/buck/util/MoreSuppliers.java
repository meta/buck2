/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.JsonGenerator;
import com.fasterxml.jackson.databind.SerializerProvider;
import com.fasterxml.jackson.databind.annotation.JsonSerialize;
import com.fasterxml.jackson.databind.ser.std.StdSerializer;
import java.io.IOException;
import java.util.Objects;
import java.util.function.Supplier;

@Nullsafe(Nullsafe.Mode.LOCAL)
public final class MoreSuppliers {
  private MoreSuppliers() {}

  /**
   * Returns a supplier which caches the instance retrieved during the first call to {@code get()}
   * and returns that value on subsequent calls to {@code get()}.
   *
   * <p>Unlike Guava's {@link
   * com.google.common.base.Suppliers#memoize(com.google.common.base.Supplier)}, this version
   * removes the reference to the underlying Supplier once the value is computed. This frees up
   * memory used in lambda captures, at the cost of causing the supplier to be not Serializable.
   */
  public static <T> Supplier<T> memoize(Supplier<T> delegate) {
    return (delegate instanceof MemoizingSupplier) ? delegate : new MemoizingSupplier<T>(delegate);
  }

  @JsonSerialize(using = MemoizingSupplierSerializer.class)
  private static class MemoizingSupplier<T> extends Memoizer<T> implements Supplier<T> {
    private final Supplier<T> delegate;

    public MemoizingSupplier(Supplier<T> delegate) {
      this.delegate = Objects.requireNonNull(delegate);
    }

    @Override
    public T get() {
      return get(delegate);
    }
  }

  /**
   * Custom serializer for Jackson because buck deserializer creates {@link
   * com.google.common.base.Suppliers}.NonSerializableMemoizingSupplier from MemoizingSupplier and
   * it's useful in tests to have both serialized into the same thing with Jackson.
   */
  static class MemoizingSupplierSerializer<T> extends StdSerializer<MemoizingSupplier<T>> {

    @SuppressWarnings("unchecked")
    public MemoizingSupplierSerializer() {
      super((Class<MemoizingSupplier<T>>) (Class<?>) MemoizingSupplier.class);
    }

    @Override
    public void serialize(
        MemoizingSupplier<T> value, JsonGenerator gen, SerializerProvider provider)
        throws IOException {
      gen.writeStartObject();
      gen.writeObjectField("instance", value.get());
      gen.writeEndObject();
    }
  }
}
