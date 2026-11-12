/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.json;

import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.JsonEncoding;
import com.fasterxml.jackson.core.JsonFactory;
import com.fasterxml.jackson.core.JsonGenerator;
import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Reads and writes JSON objects whose values are arrays of strings, such as {@code {"a.jar":
 * ["A.class"]}}. {@link #write} writes the same bytes as {@link ObjectMappers#WRITER} for such a
 * map, but neither method loads jackson-databind, which costs hundreds of classes in every one-shot
 * compiler daemon JVM.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class JsonStringListMaps {
  private static final JsonFactory FACTORY = new JsonFactory();

  static {
    // Like jackson-databind on a failed write, leave a truncated file rather than valid JSON.
    FACTORY.disable(JsonGenerator.Feature.AUTO_CLOSE_JSON_CONTENT);
  }

  private JsonStringListMaps() {}

  /** Writes {@code map} in its iteration order, with the {@code toString()} of keys and values. */
  public static void write(Path file, Map<?, ? extends Collection<?>> map) throws IOException {
    try (JsonGenerator generator = FACTORY.createGenerator(file.toFile(), JsonEncoding.UTF8)) {
      generator.writeStartObject();
      for (Map.Entry<?, ? extends Collection<?>> entry : map.entrySet()) {
        generator.writeFieldName(entry.getKey().toString());
        generator.writeStartArray();
        for (Object value : entry.getValue()) {
          generator.writeString(value.toString());
        }
        generator.writeEndArray();
      }
      generator.writeEndObject();
    }
  }

  /**
   * Reads an object of string arrays, in file order.
   *
   * @throws IOException if the file holds anything else, including a null array or element or a
   *     duplicate key
   */
  public static Map<String, List<String>> read(Path file) throws IOException {
    try (JsonParser parser = Objects.requireNonNull(FACTORY.createParser(file.toFile()))) {
      if (parser.nextToken() != JsonToken.START_OBJECT) {
        throw new IOException("Expected a JSON object of string arrays: " + file);
      }
      Map<String, List<String>> result = new LinkedHashMap<>();
      while (parser.nextToken() == JsonToken.FIELD_NAME) {
        String key = Objects.requireNonNull(parser.currentName());
        if (parser.nextToken() != JsonToken.START_ARRAY) {
          throw new IOException("Expected an array for " + key + ": " + file);
        }
        List<String> values = new ArrayList<>();
        for (JsonToken token = parser.nextToken();
            token != JsonToken.END_ARRAY;
            token = parser.nextToken()) {
          if (token == null || token == JsonToken.VALUE_NULL || !token.isScalarValue()) {
            throw new IOException("Expected a string in the array for " + key + ": " + file);
          }
          values.add(Objects.requireNonNull(parser.getText()));
        }
        if (result.put(key, values) != null) {
          throw new IOException("Duplicate key " + key + ": " + file);
        }
      }
      if (parser.currentToken() != JsonToken.END_OBJECT) {
        throw new IOException("Expected a JSON object of string arrays: " + file);
      }
      return result;
    }
  }
}
